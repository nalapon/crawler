use crate::{
    git::is_modified_status,
    index::constraints::apply_constraints,
    match_offsets::char_indices_to_byte_offsets,
    path_utils::DirectoryDistance,
    simd_path::{ArenaPtr, MAX_PATH_CHUNKS},
    sort_buffer::sort_with_buffer,
    types::{DirItem, FileItem, Score, ScoringContext},
};
use fff_query_parser::{FFFQuery, FuzzyQuery};
use neo_frizbee::Scoring;
use rayon::prelude::*;
use smallvec::SmallVec;
use std::{borrow::Cow, path::MAIN_SEPARATOR};

enum FileItems<'a> {
    All(&'a [FileItem]),
    Filtered(Vec<&'a FileItem>),
}

impl<'a> FileItems<'a> {
    #[inline]
    fn len(&self) -> usize {
        match self {
            FileItems::All(s) => s.len(),
            FileItems::Filtered(v) => v.len(),
        }
    }

    #[inline]
    fn index(&self, index: usize) -> &'a FileItem {
        match self {
            FileItems::All(s) => &s[index],
            FileItems::Filtered(v) => v[index],
        }
    }
}

/// Resolve a FileItem's chunked path into frizbee's pointer buffer.
/// Returns `Some((chunk_count, byte_len))` or `None` for deleted files.
#[inline]
fn resolve_file_chunks(
    file: &FileItem,
    arena: ArenaPtr,
    buf: &mut [*const u8; MAX_PATH_CHUNKS],
) -> Option<(usize, u16)> {
    if file.is_deleted() {
        return None;
    }
    let ptrs = file.path.resolve_ptrs(arena, buf);
    Some((ptrs.len(), file.path.byte_len))
}

#[inline]
fn match_fuzzy_parts(
    fuzzy_parts: &[&str],
    working_files: &FileItems<'_>,
    options: &neo_frizbee::Config,
    max_threads: usize,
    arena: ArenaPtr,
) -> Vec<neo_frizbee::Match> {
    let valid_parts: SmallVec<[&str; 4]> = fuzzy_parts
        .iter()
        .copied()
        .filter(|p| p.len() >= 2)
        .collect();

    if valid_parts.is_empty() {
        tracing::debug!("match_fuzzy_parts: no valid parts after filtering, returning empty");
        return vec![];
    }

    let reverse_pair = valid_parts.len() == 2 && valid_parts[1].len() > valid_parts[0].len();
    let first_part = usize::from(reverse_pair);
    let mut first_options = *options;
    if reverse_pair {
        first_options.max_typos = options
            .max_typos
            .map(|t| t.min(valid_parts[1].len() as u16));
    }

    // Narrow two-part queries with the longer part before scoring the shorter part.
    let first_part_matches = match_file_range(
        valid_parts[first_part],
        working_files.len(),
        &|index, buf: &mut [*const u8; MAX_PATH_CHUNKS]| {
            resolve_file_chunks(working_files.index(index as usize), arena, buf)
        },
        &first_options,
        max_threads,
    );

    if valid_parts.len() == 1 {
        return first_part_matches;
    }

    let total_parts = valid_parts.len() as u32;
    let mut matches = first_part_matches;
    for (part_index, part) in valid_parts
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != first_part)
    {
        let mut part_options = *options;
        if part_index > 0 {
            part_options.max_typos = options.max_typos.map(|t| t.min(part.len() as u16));
        }

        // Match only the files that survived the previous round, addressed
        // through the previous matches without collecting a subset.
        let survivors = &matches;
        let sub_matches = match_file_range(
            part,
            survivors.len(),
            &|index, buf: &mut [*const u8; MAX_PATH_CHUNKS]| {
                let file = working_files.index(survivors[index as usize].index as usize);
                resolve_file_chunks(file, arena, buf)
            },
            &part_options,
            max_threads,
        );

        if sub_matches.is_empty() {
            return vec![]; // break early! 
        }

        // Map sub_matches back to original indices, average scores across all parts.
        matches = sub_matches
            .into_iter()
            .map(|sm| {
                let prev = &matches[sm.index as usize];
                let sum = (prev.score as u32).saturating_add(sm.score as u32);
                let avg = sum / total_parts;

                neo_frizbee::Match {
                    index: prev.index,
                    score: avg.min(u16::MAX as u32) as u16,
                    end_col: if reverse_pair {
                        sm.end_col
                    } else {
                        prev.end_col
                    },
                    exact: prev.exact && sm.exact,
                }
            })
            .collect();
    }

    matches
}

/// Match + score across base and overflow files, each with their own arena.
#[tracing::instrument(skip_all, level = tracing::Level::DEBUG)]
pub fn fuzzy_match_and_score_files<'a>(
    files: &'a [FileItem],
    context: &ScoringContext,
    base_count: usize,
    base_arena: ArenaPtr,
    overflow_arena: ArenaPtr,
) -> (Vec<&'a FileItem>, Vec<Score>, usize) {
    if fuzzy_parts(context.query).is_none() {
        return rank_by_frecency(files, context, base_count, base_arena, overflow_arena);
    }

    // Process overflow files first: newly added files (created after the
    // initial scan) live in the overflow arena and are more likely to be
    // relevant to the current search. Exact ties are broken by index order.
    let results = if files.len() > base_count {
        let mut results = match_and_score_in_arena(&files[base_count..], context, overflow_arena);

        results.extend(match_and_score_in_arena(
            &files[..base_count],
            context,
            base_arena,
        ));

        results
    } else {
        match_and_score_in_arena(files, context, base_arena)
    };

    sort_and_paginate(results, context, |score| score.total)
}

/// Visit every scored match in a single arena without sorting or building a page.
/// Crawler needs all matches for scope counts and applies its own top-K order.
pub fn visit_match_scores<'a>(
    files: &'a [FileItem],
    context: &ScoringContext,
    arena: ArenaPtr,
    mut visit: impl FnMut(&'a FileItem, i32),
) {
    if fuzzy_parts(context.query).is_none() {
        if let Some(working_files) = filter_by_constraints(files, context, arena) {
            let mut totals = Vec::new();
            frecency_totals(&working_files, context, arena, &mut totals);
            for (file, total) in totals {
                visit(file, total);
            }
        }
    } else {
        for (file, score) in match_and_score_in_arena(files, context, arena) {
            visit(file, score.total);
        }
    }
}

// Ranks on a 16-byte (file, total) pair and builds the full Score only for the page.
fn rank_by_frecency<'a>(
    files: &'a [FileItem],
    context: &ScoringContext,
    base_count: usize,
    base_arena: ArenaPtr,
    overflow_arena: ArenaPtr,
) -> (Vec<&'a FileItem>, Vec<Score>, usize) {
    let base_count = base_count.min(files.len());
    let mut totals: Vec<(&FileItem, i32)> = Vec::new();
    for (slice, arena) in [
        (&files[base_count..], overflow_arena),
        (&files[..base_count], base_arena),
    ] {
        if slice.is_empty() {
            continue;
        }
        let Some(working_files) = filter_by_constraints(slice, context, arena) else {
            continue;
        };
        frecency_totals(&working_files, context, arena, &mut totals);
    }

    let (items, _, total_matched) = sort_and_paginate(totals, context, |total| *total);
    let scores = items
        .iter()
        .map(|file| {
            let arena = if file.is_overflow() {
                overflow_arena
            } else {
                base_arena
            };
            frecency_score(file, context, arena)
        })
        .collect();
    (items, scores, total_matched)
}

fn fuzzy_parts<'q>(query: &'q FFFQuery<'q>) -> Option<&'q [&'q str]> {
    match &query.fuzzy_query {
        FuzzyQuery::Text(t) if t.len() >= 2 => Some(std::slice::from_ref(t)),
        FuzzyQuery::Parts(parts) if !parts.is_empty() => Some(parts.as_slice()),
        _ => None,
    }
}

// None when constraints exclude every file.
fn filter_by_constraints<'a>(
    files: &'a [FileItem],
    context: &ScoringContext,
    arena: ArenaPtr,
) -> Option<FileItems<'a>> {
    let constraints = &context.query.constraints;
    if constraints.is_empty() {
        return Some(FileItems::All(files));
    }
    match apply_constraints(files, constraints, arena, arena) {
        Some(filtered) if !filtered.is_empty() => Some(FileItems::Filtered(filtered)),
        Some(_) => None,
        None => Some(FileItems::All(files)),
    }
}

pub fn fuzzy_match_byte_offsets_for_page<'q>(
    query: &'q FFFQuery<'q>,
    items: &[&FileItem],
    max_typos: u16,
    base_arena: ArenaPtr,
    overflow_arena: ArenaPtr,
) -> Vec<SmallVec<[(u32, u32); 4]>> {
    let parts: SmallVec<[&str; 4]> = match &query.fuzzy_query {
        FuzzyQuery::Text(text) if text.len() >= 2 => smallvec::smallvec![*text],
        FuzzyQuery::Parts(parts) => parts.iter().copied().filter(|p| p.len() >= 2).collect(),
        _ => SmallVec::new(),
    };

    let mut ranges_by_item = vec![SmallVec::new(); items.len()];
    if parts.is_empty() || items.is_empty() {
        return ranges_by_item;
    }

    let mut paths = String::with_capacity(items.iter().map(|item| item.relative_path_len()).sum());
    for item in items {
        let arena = if item.is_overflow() {
            overflow_arena
        } else {
            base_arena
        };
        item.path.append_to_string(arena, &mut paths);
    }

    let mut start = 0;
    let path_strs: Vec<&str> = items
        .iter()
        .map(|item| {
            let end = start + item.relative_path_len();
            let path = &paths[start..end];
            start = end;
            path
        })
        .collect();

    let has_uppercase = parts
        .iter()
        .any(|part| part.chars().any(|ch| ch.is_uppercase()));
    let config = neo_frizbee::Config {
        max_typos: Some(max_typos),
        sort: neo_frizbee::SortStrategy::Unsorted,
        scoring: Scoring {
            capitalization_bonus: if has_uppercase { 8 } else { 0 },
            matching_case_bonus: if has_uppercase { 4 } else { 0 },
            ..Default::default()
        },
        ..Default::default()
    };

    for (idx, part) in parts.iter().copied().enumerate() {
        let mut part_config = config;
        if idx > 0 {
            part_config.max_typos = config.max_typos.map(|t| t.min(part.len() as u16));
        }

        let mut matcher = neo_frizbee::Matcher::new(part, &part_config);
        for mut matched in matcher.match_list_indices(&path_strs) {
            let item_idx = matched.index as usize;
            let Some(path) = path_strs.get(item_idx) else {
                continue;
            };

            matched.indices.sort_unstable();
            ranges_by_item[item_idx].extend(char_indices_to_byte_offsets(path, &matched.indices));
        }
    }

    for ranges in &mut ranges_by_item {
        *ranges = merge_byte_offsets(std::mem::take(ranges));
    }

    ranges_by_item
}

fn merge_byte_offsets(mut ranges: SmallVec<[(u32, u32); 4]>) -> SmallVec<[(u32, u32); 4]> {
    if ranges.len() <= 1 {
        return ranges;
    }

    ranges.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut merged = 0;
    for index in 0..ranges.len() {
        let (start, end) = ranges[index];
        if end <= start {
            continue;
        }

        if merged > 0 && start <= ranges[merged - 1].1 {
            let last = &mut ranges[merged - 1];
            last.1 = last.1.max(end);
            continue;
        }

        ranges[merged] = (start, end);
        merged += 1;
    }

    ranges.truncate(merged);
    ranges
}

/// Resolve a DirItem's chunked path into frizbee's pointer buffer.
#[inline]
fn resolve_dir_chunks(
    dir: &DirItem,
    arena: ArenaPtr,
    overflow_arena: ArenaPtr,
    buf: &mut [*const u8; MAX_PATH_CHUNKS],
) -> Option<(usize, u16)> {
    let arena = if dir.is_overflow() {
        overflow_arena
    } else {
        arena
    };
    let ptrs = dir.path.resolve_ptrs(arena, buf);
    Some((ptrs.len(), dir.path.byte_len))
}

/// Run SIMD fuzzy match across directory items, narrowing the candidate set
/// through each query part — same pipeline as file matching.
fn match_fuzzy_parts_dirs(
    fuzzy_parts: &[&str],
    working_dirs: &[&DirItem],
    options: &neo_frizbee::Config,
    max_threads: usize,
    arena: ArenaPtr,
    overflow_arena: ArenaPtr,
) -> Vec<neo_frizbee::Match> {
    let valid_parts: SmallVec<[&str; 4]> = fuzzy_parts
        .iter()
        .copied()
        .filter(|p| p.len() >= 2)
        .collect();

    if valid_parts.is_empty() {
        return vec![];
    }

    let first_part_matches = neo_frizbee::match_range_parallel_resolved(
        valid_parts[0],
        working_dirs.len(),
        &|index, buf: &mut [*const u8; MAX_PATH_CHUNKS]| {
            resolve_dir_chunks(working_dirs[index as usize], arena, overflow_arena, buf)
        },
        options,
        max_threads,
    );

    if valid_parts.len() == 1 {
        return first_part_matches;
    }

    let mut matches = first_part_matches;
    let total_parts = valid_parts.len() as u32;
    for part in valid_parts[1..].iter() {
        let mut part_options = *options;
        part_options.max_typos = options.max_typos.map(|t| t.min(part.len() as u16));

        // Match only the dirs that survived the previous round, addressed
        // through the previous matches without collecting a subset.
        let survivors = &matches;
        let sub_matches = neo_frizbee::match_range_parallel_resolved(
            part,
            survivors.len(),
            &|index, buf: &mut [*const u8; MAX_PATH_CHUNKS]| {
                let dir = working_dirs[survivors[index as usize].index as usize];
                resolve_dir_chunks(dir, arena, overflow_arena, buf)
            },
            &part_options,
            max_threads,
        );

        if sub_matches.is_empty() {
            return vec![];
        }

        // Map sub_matches back to original indices, average scores across all parts.
        matches = sub_matches
            .into_iter()
            .map(|sm| {
                let prev = &matches[sm.index as usize];
                let sum = (prev.score as u32).saturating_add(sm.score as u32);
                let avg = sum / total_parts;

                neo_frizbee::Match {
                    index: prev.index,
                    score: avg.min(u16::MAX as u32) as u16,
                    end_col: prev.end_col, // keep first part's position for dirname bonus
                    exact: prev.exact && sm.exact,
                }
            })
            .collect();
    }

    matches
}

/// Match + score directories against a fuzzy query.
/// Scoring: base_score, frecency_boost, distance_penalty, dirname_bonus.
#[tracing::instrument(skip_all, level = tracing::Level::DEBUG)]
pub(crate) fn fuzzy_match_and_score_dirs<'a>(
    dirs: &'a [DirItem],
    context: &ScoringContext,
    arena: ArenaPtr,
    overflow_arena: ArenaPtr,
) -> (Vec<&'a DirItem>, Vec<Score>, usize) {
    if dirs.is_empty() {
        return (vec![], vec![], 0);
    }

    let parsed_query = context.query;
    // Ghost dirs (all files tombstoned) never surface in search results.
    let working_dirs: Vec<&DirItem> = if parsed_query.constraints.is_empty() {
        dirs.iter().filter(|d| !d.is_deleted()).collect()
    } else {
        match apply_constraints(dirs, &parsed_query.constraints, arena, overflow_arena) {
            Some(filtered) if !filtered.is_empty() => {
                filtered.into_iter().filter(|d| !d.is_deleted()).collect()
            }
            Some(_) => return (vec![], vec![], 0),
            None => dirs.iter().filter(|d| !d.is_deleted()).collect(),
        }
    };

    let fuzzy_parts: &[&str] = match &parsed_query.fuzzy_query {
        FuzzyQuery::Text(t) if t.len() >= 2 => std::slice::from_ref(t),
        FuzzyQuery::Parts(parts) if !parts.is_empty() => parts.as_slice(),
        _ => {
            return score_dirs_by_frecency(&working_dirs, context);
        }
    };

    let valid_parts: SmallVec<[&str; 4]> = fuzzy_parts
        .iter()
        .copied()
        .filter(|p| p.len() >= 2)
        .collect();

    if valid_parts.is_empty() {
        return score_dirs_by_frecency(&working_dirs, context);
    }

    let has_uppercase = valid_parts
        .iter()
        .any(|p| p.chars().any(|c| c.is_uppercase()));

    let options = neo_frizbee::Config {
        max_typos: Some(context.max_typos),
        sort: neo_frizbee::SortStrategy::Unsorted,
        scoring: Scoring {
            capitalization_bonus: if has_uppercase { 8 } else { 0 },
            matching_case_bonus: if has_uppercase { 4 } else { 0 },
            ..Default::default()
        },
        ..Default::default()
    };

    let path_matches = match_fuzzy_parts_dirs(
        fuzzy_parts,
        &working_dirs,
        &options,
        context.max_threads,
        arena,
        overflow_arena,
    );

    let main_needle = valid_parts[0].as_bytes();
    let main_needle_len = main_needle.len() as u16;

    let mut dir_buf = String::with_capacity(64);
    let mut dirname_buf = String::with_capacity(32);
    let distance = context.current_file.map(DirectoryDistance::new);

    let results: Vec<(&DirItem, Score)> = path_matches
        .into_iter()
        .map(|path_match| {
            let dir = working_dirs[path_match.index as usize];
            let dir_arena = if dir.is_overflow() {
                overflow_arena
            } else {
                arena
            };
            let base_score = path_match.score as i32;
            let frecency_boost = base_score.saturating_mul(dir.max_access_frecency()) / 100;

            // Distance penalty from current file's directory.
            let distance_penalty = if let Some(distance) = &distance {
                dir.path.write_to_string(dir_arena, &mut dir_buf);
                distance.penalty(&dir_buf)
            } else {
                0
            };

            // Dirname bonus: if the match is in the last path segment.
            let last_seg_offset = dir.last_segment_offset();
            let match_start_approx = path_match.end_col.saturating_sub(main_needle_len - 1);
            let is_dirname_match = match_start_approx >= last_seg_offset;

            dir.write_dir_name(dir_arena, &mut dirname_buf);
            let dirname_len = dirname_buf.len();
            let is_exact_dirname = is_dirname_match
                && main_needle_len as usize == dirname_len
                && main_needle.eq_ignore_ascii_case(dirname_buf.as_bytes());

            let filename_bonus = if is_exact_dirname {
                base_score / 5 * 2 // 40% bonus for exact dirname match
            } else if is_dirname_match {
                base_score / 6 // ~16% bonus for fuzzy dirname match
            } else {
                0
            };

            let total = base_score
                .saturating_add(frecency_boost)
                .saturating_add(distance_penalty)
                .saturating_add(filename_bonus);

            let score = Score {
                total,
                base_score,
                filename_bonus,
                special_filename_bonus: 0,
                frecency_boost,
                git_status_boost: 0,
                git_recency_boost: 0,
                distance_penalty,
                current_file_penalty: 0,
                combo_match_boost: 0,
                path_alignment_bonus: 0,
                exact_match: is_exact_dirname || path_match.exact,
                match_type: if is_exact_dirname {
                    "exact_dirname"
                } else if is_dirname_match {
                    "fuzzy_dirname"
                } else if path_match.exact {
                    "exact_path"
                } else {
                    "fuzzy_path"
                },
            };

            (dir, score)
        })
        .collect();

    sort_and_paginate_dirs(results, context)
}

fn score_dirs_by_frecency<'a>(
    dirs: &[&'a DirItem],
    context: &ScoringContext,
) -> (Vec<&'a DirItem>, Vec<Score>, usize) {
    let results: Vec<(&DirItem, Score)> = dirs
        .iter()
        .map(|&dir| {
            let score = Score {
                total: dir.max_access_frecency(),
                frecency_boost: dir.max_access_frecency(),
                match_type: "frecency",
                ..Default::default()
            };

            (dir, score)
        })
        .collect();

    sort_and_paginate_dirs(results, context)
}

/// Sort dir results by total score (descending) and apply pagination.
fn sort_and_paginate_dirs<'a>(
    mut results: Vec<(&'a DirItem, Score)>,
    context: &ScoringContext,
) -> (Vec<&'a DirItem>, Vec<Score>, usize) {
    let total_matched = results.len();
    if total_matched == 0 {
        return (vec![], vec![], 0);
    }

    let offset = context.pagination.offset;
    let limit = if context.pagination.limit > 0 {
        context.pagination.limit
    } else {
        total_matched
    };

    if offset >= total_matched {
        return (vec![], vec![], total_matched);
    }

    let items_needed = offset.saturating_add(limit).min(total_matched);
    let use_partial_sort = items_needed < total_matched / 2 && total_matched > 100;

    let compare = |a: &(&DirItem, Score), b: &(&DirItem, Score)| {
        b.1.total
            .cmp(&a.1.total)
            .then_with(|| std::ptr::from_ref(a.0).cmp(&std::ptr::from_ref(b.0)))
    };
    if use_partial_sort {
        results.select_nth_unstable_by(items_needed - 1, compare);
        results.truncate(items_needed);
    }

    sort_with_buffer(&mut results, compare);

    let (items, scores): (Vec<&DirItem>, Vec<Score>) =
        results.into_iter().skip(offset).take(limit).unzip();
    (items, scores, total_matched)
}

fn match_and_score_in_arena<'a>(
    files: &'a [FileItem],
    context: &ScoringContext,
    arena: ArenaPtr,
) -> Vec<(&'a FileItem, Score)> {
    if context.current_file.is_some() {
        match_and_score_in_arena_inner::<true>(files, context, arena)
    } else {
        match_and_score_in_arena_inner::<false>(files, context, arena)
    }
}

fn match_and_score_in_arena_inner<'a, const WITH_CURRENT_FILE: bool>(
    files: &'a [FileItem],
    context: &ScoringContext,
    arena: ArenaPtr,
) -> Vec<(&'a FileItem, Score)> {
    if files.is_empty() {
        return vec![];
    }

    let Some(working_files) = filter_by_constraints(files, context, arena) else {
        return vec![];
    };
    let Some(fuzzy_parts) = fuzzy_parts(context.query) else {
        // Frecency-only queries are ranked by `rank_by_frecency` before reaching here.
        return frecency_scores(&working_files, context, arena);
    };
    let has_uppercase = fuzzy_parts
        .iter()
        .any(|p| p.chars().any(|c| c.is_uppercase()));
    // Users type `/` regardless of platform. Checking the OS separator alone
    // would miss forward-slash queries on Windows.
    let query_contains_path_separator = fuzzy_parts
        .iter()
        .any(|p| p.contains('/') || p.contains(MAIN_SEPARATOR));

    let options = neo_frizbee::Config {
        max_typos: Some(context.max_typos),
        sort: neo_frizbee::SortStrategy::Unsorted,
        scoring: Scoring {
            capitalization_bonus: if has_uppercase { 8 } else { 0 },
            matching_case_bonus: if has_uppercase { 4 } else { 0 },
            ..Default::default()
        },
        ..Default::default()
    };

    let path_matches = match_fuzzy_parts(
        fuzzy_parts,
        &working_files,
        &options,
        context.max_threads,
        arena,
    );

    let main_needle = fuzzy_parts[0].as_bytes(); // safe
    let main_needle_len = main_needle.len() as u16;

    let mut fallback_indices: Vec<u32> = Vec::new();
    let filename_fallback_matches = if query_contains_path_separator || path_matches.len() > 15_000
    {
        vec![]
    } else {
        let mut fallback_filenames: Vec<Cow<'_, str>> = Vec::new();

        for (i, path_match) in path_matches.iter().enumerate() {
            let file = working_files.index(path_match.index as usize);
            let filename_start = file.filename_offset_in_relative_path() as u16;
            let match_start_approx = path_match.end_col.saturating_sub(main_needle_len - 1);

            if match_start_approx < filename_start {
                fallback_indices.push(i as u32);
                fallback_filenames.push(file.path.filename_cow(arena));
            }
        }

        if fallback_filenames.is_empty() {
            vec![]
        } else {
            // Match on `&str` so frizbee reuses the instantiation its index
            // resolver already emits instead of a separate `Cow<str>` copy.
            let filename_strs: Vec<&str> = fallback_filenames.iter().map(Cow::as_ref).collect();
            neo_frizbee::Matcher::new(fuzzy_parts[0], &options).match_list_parallel(
                &filename_strs,
                if path_matches.len() > 4096 {
                    context.max_threads.div_ceil(2048)
                } else {
                    1
                },
            )
        }
    };

    // path-match index -> position in filename_fallback_matches (u32::MAX = none)
    let mut fallback_by_match: Vec<u32> = Vec::new();
    if !filename_fallback_matches.is_empty() {
        fallback_by_match.resize(path_matches.len(), u32::MAX);
        for (position, m) in filename_fallback_matches.iter().enumerate() {
            fallback_by_match[fallback_indices[m.index as usize] as usize] = position as u32;
        }
    }

    let distance = context
        .current_file
        .filter(|_| WITH_CURRENT_FILE)
        .map(DirectoryDistance::new);
    let combo_path = context
        .last_same_query_match
        .as_ref()
        .map(|entry| entry.file_path.to_string_lossy());

    let score_match =
        |bufs: &mut ScoreBuffers, (match_idx, path_match): (usize, &neo_frizbee::Match)| {
            let ScoreBuffers {
                dir_buf,
                fname_buf,
                last_dir_penalty,
            } = bufs;
            let file_idx = path_match.index as usize;
            let file = working_files.index(file_idx);

            let base_score = path_match.score as i32;
            let frecency_boost = base_score.saturating_mul(file.total_frecency_score()) / 100;

            let git_status_boost = if file.git_status.is_some_and(is_modified_status) {
                base_score * 15 / 100
            } else {
                0
            };
            let git_recency_boost = file.git_recency_score as i32;

            let distance_penalty = if WITH_CURRENT_FILE && let Some(distance) = &distance {
                if let Some((parent, penalty)) = *last_dir_penalty
                    && parent == file.parent_dir_index
                    && parent != u32::MAX
                {
                    penalty
                } else {
                    file.write_dir_str(arena, dir_buf);
                    let penalty = distance.penalty(dir_buf);
                    *last_dir_penalty = Some((file.parent_dir_index, penalty));
                    penalty
                }
            } else {
                0
            };

            let filename_start = file.filename_offset_in_relative_path() as u16;
            let match_start_approx = path_match.end_col.saturating_sub(main_needle_len - 1);

            let end_col_filename_match = match_start_approx >= filename_start;
            let simd_filename_match = if !end_col_filename_match {
                fallback_by_match
                    .get(match_idx)
                    .and_then(|&position| filename_fallback_matches.get(position as usize))
            } else {
                None
            };

            let is_filename_match = end_col_filename_match || simd_filename_match.is_some();
            let fname_len = file.path.byte_len as usize - file.path.filename_offset as usize;

            let is_exact_filename = simd_filename_match.is_some_and(|m| m.exact)
                || (end_col_filename_match && main_needle_len as usize == fname_len && {
                    file.write_file_name_from_arena(arena, fname_buf);
                    main_needle.eq_ignore_ascii_case(fname_buf.as_bytes())
                });

            let mut has_special_filename_bonus = false;
            let filename_bonus = if is_exact_filename {
                base_score / 5 * 2 // 40% bonus for exact filename match
            } else if is_filename_match {
                // 16% bonus for fuzzy filename match that landed in the filename region.
                // For fallback matches (where the path match landed in a directory segment),
                // scale the bonus by the quality of the filename match — a contiguous match
                // like "rename" in "rename.ts" gets the full bonus, while a scattered
                // subsequence like r-e-n-a-m-e in "generateSessionName.ts" gets much less.
                let max_bonus = (base_score / 6).min(30);
                if let Some(fm) = simd_filename_match {
                    let max_possible = main_needle_len as i32 * 16;
                    let quality = (fm.score as i32).min(max_possible);
                    max_bonus * quality / max_possible
                } else {
                    max_bonus
                }
            } else if !is_filename_match && (5..=11).contains(&fname_len) {
                file.write_file_name_from_arena(arena, fname_buf);
                // 5% bonus for special file but not as much as file name to avoid situations
                // when you have /user_service/server.rs and /user_service/server/mod.rs
                if is_special_entry_point_file(fname_buf) {
                    has_special_filename_bonus = true;
                    base_score * 5 / 100
                } else {
                    0
                }
            } else {
                0
            };

            let current_file_penalty = if WITH_CURRENT_FILE {
                calculate_current_file_penalty(file, base_score / 4, context, arena)
            } else {
                0
            };
            let combo_match_boost = {
                let last_same_query_match = context.last_same_query_match.as_ref().filter(|_| {
                    combo_path
                        .as_ref()
                        .and_then(|path| {
                            path.len()
                                .checked_sub(file.relative_path_len())
                                .and_then(|start| path.get(start..))
                        })
                        .is_some_and(|suffix| file.relative_path_eq(arena, suffix))
                });

                match last_same_query_match {
                    Some(_) if context.min_combo_count == 0 => 1000,
                    Some(combo_match) if combo_match.open_count >= context.min_combo_count => {
                        combo_match.open_count as i32 * context.combo_boost_score_multiplier
                    }
                    Some(combo_match) => combo_match.open_count as i32 * 5,
                    _ => 0,
                }
            };

            let path_alignment_bonus = if query_contains_path_separator && main_needle.len() > 10 {
                let common_suffix = file
                    .path
                    .common_suffix_len_ignore_ascii_case(arena, main_needle);

                let needle_len = main_needle.len();
                if common_suffix > 10 && needle_len > 0 {
                    let coverage = common_suffix * 100 / needle_len;
                    if coverage >= 30 {
                        base_score * coverage as i32 / 100
                    } else {
                        0
                    }
                } else {
                    0
                }
            } else {
                0
            };

            let total = base_score
                .saturating_add(frecency_boost)
                .saturating_add(git_status_boost)
                .saturating_add(git_recency_boost)
                .saturating_add(distance_penalty)
                .saturating_add(filename_bonus)
                .saturating_add(current_file_penalty)
                .saturating_add(combo_match_boost)
                .saturating_add(path_alignment_bonus);

            let score = Score {
                total,
                base_score,
                current_file_penalty,
                filename_bonus,
                special_filename_bonus: if has_special_filename_bonus {
                    filename_bonus
                } else {
                    0
                },
                frecency_boost,
                git_status_boost,
                git_recency_boost,
                distance_penalty,
                combo_match_boost,
                path_alignment_bonus,
                exact_match: is_exact_filename || path_match.exact,
                match_type: if is_exact_filename {
                    "exact_filename"
                } else if is_filename_match {
                    "fuzzy_filename"
                } else if path_match.exact {
                    "exact_path"
                } else {
                    "fuzzy_path"
                },
            };

            (file, score)
        };

    // Scoring is a pure per-match function; only the scratch buffers are per-thread.
    let results: Vec<_> =
        if path_matches.len() >= PARALLEL_SCORING_MIN_MATCHES && context.max_threads > 1 {
            path_matches
                .par_iter()
                .enumerate()
                .with_min_len((path_matches.len() / context.max_threads).max(4096))
                .map_init(ScoreBuffers::new, score_match)
                .collect()
        } else {
            let mut bufs = ScoreBuffers::new();
            path_matches
                .iter()
                .enumerate()
                .map(|item| score_match(&mut bufs, item))
                .collect()
        };

    results
}

const PARALLEL_SCORING_MIN_MATCHES: usize = 8192;

struct ScoreBuffers {
    dir_buf: String,
    fname_buf: String,
    last_dir_penalty: Option<(u32, i32)>,
}

impl ScoreBuffers {
    fn new() -> Self {
        Self {
            dir_buf: String::with_capacity(64),
            fname_buf: String::with_capacity(32),
            last_dir_penalty: None,
        }
    }
}

fn is_special_entry_point_file(filename: &str) -> bool {
    matches!(
        filename,
        "mod.rs"
            | "lib.rs"
            | "main.rs"
            | "index.js"
            | "index.jsx"
            | "index.ts"
            | "index.tsx"
            | "index.mjs"
            | "index.cjs"
            | "index.vue"
            | "__init__.py"
            | "__main__.py"
            | "main.go"
            | "main.c"
            | "index.php"
            | "main.rb"
            | "index.rb"
    )
}

fn frecency_total(file: &FileItem, context: &ScoringContext, arena: ArenaPtr) -> i32 {
    frecency_score(file, context, arena).total
}

fn frecency_score(file: &FileItem, context: &ScoringContext, arena: ArenaPtr) -> Score {
    let frecency_boost = file.access_frecency_score as i32
        + (file.modification_frecency_score as i32).saturating_mul(4);

    // Give modified/dirty files a boost even in frecency-only mode
    let git_status_boost = if file.git_status.is_some_and(is_modified_status) {
        frecency_boost * 15 / 100
    } else {
        0
    };
    let git_recency_boost = file.git_recency_score as i32;
    let current_file_penalty = calculate_current_file_penalty(file, frecency_boost, context, arena);
    let total = frecency_boost
        .saturating_add(git_status_boost)
        .saturating_add(git_recency_boost)
        .saturating_add(current_file_penalty);

    Score {
        total,
        base_score: 0,
        filename_bonus: 0,
        distance_penalty: 0,
        special_filename_bonus: 0,
        combo_match_boost: 0,
        path_alignment_bonus: 0,
        current_file_penalty,
        frecency_boost,
        git_status_boost,
        git_recency_boost,
        exact_match: false,
        match_type: "frecency",
    }
}

fn frecency_totals<'a>(
    files: &FileItems<'a>,
    context: &ScoringContext,
    arena: ArenaPtr,
    out: &mut Vec<(&'a FileItem, i32)>,
) {
    let total = |file: &'a FileItem| (file, frecency_total(file, context, arena));
    match files {
        // Small indexes cannot amortize Rayon scheduling for this cheap scoring pass.
        FileItems::All(s) if s.len() >= 32_768 && context.max_threads > 1 => out.par_extend(
            s.par_iter()
                .with_min_len(4096)
                .filter(|f| !f.is_deleted())
                .map(total),
        ),
        FileItems::All(s) => out.extend(s.iter().filter(|f| !f.is_deleted()).map(total)),
        FileItems::Filtered(v) => {
            out.extend(v.iter().copied().filter(|f| !f.is_deleted()).map(total))
        }
    }
}

fn frecency_scores<'a>(
    files: &FileItems<'a>,
    context: &ScoringContext,
    arena: ArenaPtr,
) -> Vec<(&'a FileItem, Score)> {
    let mut totals = Vec::new();
    frecency_totals(files, context, arena, &mut totals);
    totals
        .into_iter()
        .map(|(file, _)| (file, frecency_score(file, context, arena)))
        .collect()
}

#[inline]
fn calculate_current_file_penalty(
    file: &FileItem,
    base_score: i32,
    context: &ScoringContext,
    arena: ArenaPtr,
) -> i32 {
    let mut penalty = 0i32;

    if let Some(current) = context.current_file
        && file.relative_path_eq(arena, current)
    {
        penalty -= base_score;
    }

    penalty
}

/// Sorts elements by total score (descending) and returns the requested page.
/// Always returns results in descending order (best scores first).
/// The UI layer handles rendering order based on prompt position.
#[tracing::instrument(skip_all, level = tracing::Level::DEBUG)]
fn sort_and_paginate<'a, S>(
    mut results: Vec<(&'a FileItem, S)>,
    context: &ScoringContext,
    total: impl Fn(&S) -> i32,
) -> (Vec<&'a FileItem>, Vec<S>, usize) {
    let total_matched = results.len();

    if total_matched == 0 {
        return (vec![], vec![], 0);
    }

    let offset = context.pagination.offset;
    let limit = if context.pagination.limit > 0 {
        context.pagination.limit
    } else {
        total_matched
    };

    // Check if offset is out of bounds
    if offset >= total_matched {
        tracing::warn!(
            offset = offset,
            total_matched = total_matched,
            "Pagination: offset >= total_matched, returning empty"
        );

        return (vec![], vec![], total_matched);
    }

    let items_needed = offset.saturating_add(limit).min(total_matched);
    let compare = |a: &(&FileItem, S), b: &(&FileItem, S)| {
        total(&b.1)
            .cmp(&total(&a.1))
            .then_with(|| b.0.modified.cmp(&a.0.modified))
            // Total order: parallel matching yields scheduling-dependent input order.
            .then_with(|| std::ptr::from_ref(a.0).cmp(&std::ptr::from_ref(b.0)))
    };
    // Use partial sort if we need less than half the results and dataset is large
    if items_needed < total_matched / 2 && total_matched > 100 {
        results.select_nth_unstable_by(items_needed - 1, compare);
        results.truncate(items_needed);
    }

    // select nth does not sort the results, we have to sort accordingly anyway
    sort_with_buffer(&mut results, compare);

    let (items, scores): (Vec<&FileItem>, Vec<S>) =
        results.into_iter().skip(offset).take(limit).unzip();
    (items, scores, total_matched)
}

fn match_file_range<F>(
    needle: &str,
    len: usize,
    resolve: &F,
    options: &neo_frizbee::Config,
    max_threads: usize,
) -> Vec<neo_frizbee::Match>
where
    F: Fn(u32, &mut [*const u8; MAX_PATH_CHUNKS]) -> Option<(usize, u16)> + Sync,
{
    // The rayon path below flattens per-worker results and never sorts, so
    // any sorted strategy stays on frizbee's own k-merge implementation.
    let unsorted = matches!(options.sort, neo_frizbee::SortStrategy::Unsorted);
    if len < 32_768 || !unsorted {
        return neo_frizbee::match_range_parallel_resolved(
            needle,
            len,
            resolve,
            options,
            max_threads,
        );
    }
    assert!(
        len <= u32::MAX as usize,
        "too many files for fuzzy matching"
    );
    let mut matcher = neo_frizbee::Matcher::new(needle, options);
    let max_threads = if max_threads == 0 {
        std::thread::available_parallelism()
            .map(|n| n.get().saturating_sub(2))
            .unwrap_or(1)
    } else {
        max_threads
    };
    let workers = max_threads.max(1).min(len.div_ceil(2000));
    if workers <= 1 {
        return matcher.match_range_resolved(len, resolve);
    }
    let chunk_size = len.div_ceil(workers * 4).clamp(2048, 16_384);
    let next = std::sync::atomic::AtomicUsize::new(0);
    (0..workers)
        .into_par_iter()
        .map(|_| {
            let mut matcher = matcher.clone();
            let mut matches = Vec::new();
            loop {
                let start = next.fetch_add(chunk_size, std::sync::atomic::Ordering::Relaxed);
                if start >= len {
                    break;
                }
                let end = (start + chunk_size).min(len);
                matcher.match_range_resolved_into(start as u32..end as u32, resolve, &mut matches);
            }
            matches
        })
        .collect::<Vec<_>>()
        .into_iter()
        .flatten()
        .collect()
}





