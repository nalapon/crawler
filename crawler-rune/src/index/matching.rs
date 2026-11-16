use fff_query_parser::{Constraint, FuzzyQuery, MixedSearchConfig, QueryParser};
use fff_search::{
    FileItem, PaginationArgs, ScoringContext,
    crawler::{fuzzy_match_and_score_files, fuzzy_match_byte_offsets_for_page, set_path},
};
use rayon::prelude::*;
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use unicode_normalization::UnicodeNormalization;
pub type Boosts = HashMap<String, i16>;
pub fn boosts(values: &[(String, i16)], root: &Path) -> Boosts {
    values
        .iter()
        .filter_map(|(path, value)| {
            let relative = Path::new(path).strip_prefix(root).ok()?.to_str()?;
            Some((relative.nfc().collect(), *value))
        })
        .collect()
}
use super::{
    Hit, Query, Response, Scope,
    snapshot::{self, Snapshot},
};

#[derive(Clone)]
pub struct Ranked {
    pub batch: Arc<Snapshot>,
    pub row: usize,
    pub score: i32,
}
#[derive(Default)]
pub struct Matches {
    pub hits: Vec<Ranked>,
    pub counts: BTreeMap<PathBuf, usize>,
    pub total: usize,
}
impl Matches {
    pub fn merge(&mut self, other: Self) {
        self.total += other.total;
        for (path, count) in other.counts {
            *self.counts.entry(path).or_default() += count;
        }
        self.hits.extend(other.hits);
        self.hits.sort_unstable_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.batch.path(a.row).len().cmp(&b.batch.path(b.row).len()))
                .then_with(|| a.batch.path(a.row).cmp(b.batch.path(b.row)))
        });
        self.hits.truncate(100);
    }
    pub fn response(
        &self,
        query: &Query,
        root: &Path,
        id: u64,
        complete: bool,
        literal: bool,
    ) -> Response {
        let hits = self
            .hits
            .iter()
            .map(|hit| {
                let path = hit.batch.path(hit.row);
                let offsets = highlights(query, hit, literal);
                let split = path.rfind('/').map_or(0, |at| at + 1);
                let absolute = root.join(path);
                let root_bytes = absolute.as_os_str().as_encoded_bytes().len() - path.len();
                let parent_bytes = absolute
                    .parent()
                    .map_or(0, |p| p.as_os_str().as_encoded_bytes().len());
                let mut name_highlights = Vec::new();
                let mut dir_highlights = Vec::new();
                for range in snapshot::original_ranges(path, &offsets) {
                    if range.start < split {
                        let start = range.start + root_bytes;
                        let end = (range.end.min(split) + root_bytes).min(parent_bytes);
                        if start < end {
                            dir_highlights.push(start..end);
                        }
                    }
                    if range.end > split {
                        name_highlights.push(range.start.max(split) - split..range.end - split);
                    }
                }
                Hit {
                    path: absolute,
                    is_dir: hit.batch.directory(hit.row),
                    score: i64::from(hit.score),
                    name_highlights,
                    dir_highlights,
                }
            })
            .collect();
        Response {
            id,
            hits,
            scope_counts: self.counts.iter().map(|(k, v)| (k.clone(), *v)).collect(),
            total: self.total,
            complete,
        }
    }
}
fn parsed<'a>(
    parser: &QueryParser<MixedSearchConfig>,
    text: &'a str,
    literal: bool,
) -> fff_query_parser::FFFQuery<'a> {
    let mut query = parser.parse(text);
    if literal && let FuzzyQuery::Parts(parts) = &query.fuzzy_query {
        query
            .constraints
            .extend(parts.iter().copied().map(Constraint::Text));
    }
    if text.len() == 1 {
        query.fuzzy_query = FuzzyQuery::Empty;
        query.constraints.push(Constraint::Text(text));
    }
    query
}
fn context<'a>(query: &'a fff_query_parser::FFFQuery<'a>, text: &str) -> ScoringContext<'a> {
    let effective = match &query.fuzzy_query {
        FuzzyQuery::Text(t) => *t,
        FuzzyQuery::Parts(p) if !p.is_empty() => p[0],
        _ => text,
    };
    ScoringContext {
        query,
        project_path: None,
        current_file: None,
        max_typos: (effective.len() as u16 / 4).clamp(2, 6),
        max_threads: 1,
        last_same_query_match: None,
        combo_boost_score_multiplier: 0,
        min_combo_count: 0,
        pagination: PaginationArgs {
            offset: 0,
            limit: 0,
        },
    }
}
pub fn batch(
    batch: &Arc<Snapshot>,
    query: &Query,
    root: &Path,
    exclude: Option<&Path>,
    boosts: &Boosts,
    literal: bool,
) -> Matches {
    let mut files = Vec::with_capacity(batch.len());
    let mut rows = Vec::with_capacity(batch.len());
    let scope = match &query.scope {
        Scope::Mac => None,
        Scope::Under(p) => p.strip_prefix(root).ok().and_then(Path::to_str),
    };
    if matches!(query.scope, Scope::Under(_)) && scope.is_none() {
        return Matches::default();
    }
    let excluded = exclude
        .and_then(|p| p.strip_prefix(root).ok())
        .and_then(Path::to_str);
    for row in 0..batch.len() {
        let path = batch.path(row);
        if scope.is_some_and(|s| !s.is_empty() && !descendant(path, s))
            || excluded.is_some_and(|s| descendant(path, s))
        {
            continue;
        }
        let prepared = &batch.paths[row];
        let mut file = FileItem::new_raw(prepared.filename_offset, 0, 0, None, false);
        set_path(&mut file, prepared.clone());
        file.access_frecency_score = if path.is_ascii() {
            boosts.get(path).copied()
        } else {
            boosts.get(&path.nfc().collect::<String>()).copied()
        }
        .unwrap_or(0);
        files.push(file);
        rows.push(row);
    }
    if files.is_empty() {
        return Matches::default();
    }
    let parser = QueryParser::new(MixedSearchConfig);
    let parsed = parsed(&parser, &query.text, literal);
    let context = context(&parsed, &query.text);
    let arena = batch.arena();
    let (items, scores, total) =
        fuzzy_match_and_score_files(&files, &context, files.len(), arena, arena);
    let mut result = Matches {
        total,
        ..Matches::default()
    };
    for (position, (item, score)) in items.into_iter().zip(scores).enumerate() {
        // The matcher returns references into this exact, unmoved files allocation.
        let index = (std::ptr::from_ref(item) as usize - files.as_ptr() as usize)
            / std::mem::size_of::<FileItem>();
        let row = rows[index];
        let path = batch.path(row);
        let relative = scope
            .filter(|s| !s.is_empty())
            .and_then(|s| relative_to(path, s))
            .unwrap_or(path)
            .trim_start_matches('/');
        if let Some((child, _)) = relative.split_once('/') {
            let base = match &query.scope {
                Scope::Mac => root,
                Scope::Under(p) => p,
            };
            *result.counts.entry(base.join(child)).or_default() += 1;
        }
        if position < 100 {
            result.hits.push(Ranked {
                batch: Arc::clone(batch),
                row,
                score: score.total,
            });
        }
    }
    result
}
pub fn descendant(path: &str, scope: &str) -> bool {
    relative_to(path, scope).is_some()
}
fn relative_to<'a>(path: &'a str, scope: &str) -> Option<&'a str> {
    let scope = scope.trim_end_matches('/');
    if scope.is_empty() {
        return Some(path);
    }
    if let Some(rest) = path.strip_prefix(scope).and_then(|s| s.strip_prefix('/')) {
        return Some(rest);
    }
    if path.is_ascii() && scope.is_ascii() {
        return None;
    }
    let mut components = path.split('/');
    let mut at = 0;
    for part in scope.split('/') {
        let actual = components.next()?;
        if !snapshot::equal(actual, part) {
            return None;
        }
        at += actual.len() + 1;
    }
    path.get(at..)
}
fn highlights(query: &Query, hit: &Ranked, literal: bool) -> Vec<(u32, u32)> {
    let parser = QueryParser::new(MixedSearchConfig);
    let parsed = parsed(&parser, &query.text, literal);
    let context = context(&parsed, &query.text);
    let prepared = &hit.batch.paths[hit.row];
    let mut file = FileItem::new_raw(prepared.filename_offset, 0, 0, None, false);
    set_path(&mut file, prepared.clone());
    fuzzy_match_byte_offsets_for_page(
        &parsed,
        &[&file],
        context.max_typos,
        hit.batch.arena(),
        hit.batch.arena(),
    )
    .into_iter()
    .flatten()
    .collect()
}

pub fn group(
    batches: &[Arc<Snapshot>],
    query: &Query,
    root: &Path,
    exclude: Option<&Path>,
    boosts: &Boosts,
    literal: bool,
    generation: &AtomicU64,
    id: u64,
) -> Matches {
    batches
        .par_iter()
        .map(|snapshot| {
            if generation.load(Ordering::Relaxed) != id {
                return Matches::default();
            }
            batch(snapshot, query, root, exclude, boosts, literal)
        })
        .reduce(Matches::default, |mut a, b| {
            a.merge(b);
            a
        })
}
