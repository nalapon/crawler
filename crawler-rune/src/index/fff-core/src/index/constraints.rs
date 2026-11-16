//! Constraint-based prefiltering for search queries.

use fff_query_parser::{Constraint, GitStatusFilter};
use smallvec::SmallVec;

use crate::git::is_modified_status;
use crate::simd_path::ArenaPtr;
use crate::simd_string_utils::memmem::find_case_insensitive_short;

const PAR_THRESHOLD: usize = 10_000;

pub(crate) trait Constrainable {
    fn write_file_name(&self, arena: ArenaPtr, out: &mut String);
    fn git_status(&self) -> Option<git2::Status>;
    fn write_relative_path(&self, arena: ArenaPtr, out: &mut String);
    fn is_overflow(&self) -> bool;

    fn has_extension(&self, arena: ArenaPtr, extension: &str, scratch: &mut String) -> bool {
        self.write_file_name(arena, scratch);
        file_has_extension(scratch, extension)
    }
}

/// Stored/canonical paths use `/`; also accept `\` so a Windows user typing
/// a native separator in a query still matches.
#[inline]
fn is_path_sep(b: u8) -> bool {
    b == b'/' || b == b'\\'
}

#[inline]
fn path_slice_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).all(|(x, y)| {
        if is_path_sep(*x) && is_path_sep(*y) {
            true
        } else {
            x.eq_ignore_ascii_case(y)
        }
    })
}

/// Path ends with suffix at a path-separator boundary (case-insensitive).
#[inline]
pub fn path_ends_with_suffix(path: &str, suffix: &str) -> bool {
    let path_bytes = path.as_bytes();
    let suffix_bytes = suffix.as_bytes();
    if path_bytes.len() < suffix_bytes.len() {
        return false;
    }

    let start = path.len() - suffix.len();

    // Multi-byte UTF-8 may put `start` inside a char.
    if !path.is_char_boundary(start) {
        return false;
    }

    if !path_slice_eq(&path_bytes[start..], suffix_bytes) {
        return false;
    }

    // Exact or preceded by a separator. Scan backward past any multi-byte
    // continuation bytes to find the preceding ASCII byte.
    if start == 0 {
        return true;
    }
    let mut i = start;
    while i > 0 {
        i -= 1;
        if path_bytes[i] < 128 {
            return is_path_sep(path_bytes[i]);
        }
    }
    false
}

#[inline]
pub fn file_has_extension(file_name: &str, ext: &str) -> bool {
    let name_bytes = file_name.as_bytes();
    let ext_bytes = ext.as_bytes();
    if name_bytes.len() <= ext_bytes.len() + 1 {
        return false;
    }
    let start = name_bytes.len() - ext_bytes.len() - 1;
    if start > 0 && !file_name.is_char_boundary(start) {
        return false;
    }
    name_bytes.get(start) == Some(&b'.') && name_bytes[start + 1..].eq_ignore_ascii_case(ext_bytes)
}

/// Matches multi-segment queries like `libswscale/aarch64`.
#[inline]
pub fn path_contains_segment(path: &str, segment: &str) -> bool {
    let path_bytes = path.as_bytes();
    let segment_bytes = segment.as_bytes();
    let segment_len = segment_bytes.len();

    if path_bytes.len() > segment_len
        && is_path_sep(path_bytes[segment_len])
        && path.is_char_boundary(segment_len)
        && path_slice_eq(&path_bytes[..segment_len], segment_bytes)
    {
        return true;
    }

    if path_bytes.len() < segment_len + 2 {
        return false;
    }

    for i in 0..path_bytes.len().saturating_sub(segment_len + 1) {
        if is_path_sep(path_bytes[i]) {
            let start = i + 1;
            let end = start + segment_len;
            if end < path_bytes.len()
                && is_path_sep(path_bytes[end])
                && path.is_char_boundary(start)
                && path.is_char_boundary(end)
                && path_slice_eq(&path_bytes[start..end], segment_bytes)
            {
                return true;
            }
        }
    }
    false
}

/// Returns `None` if no constraints are present, `Some(filtered)` otherwise.
///
/// Constraint semantics:
/// - All `Extension` constraints OR together (file matches if ANY extension hits).
///   They're split out up front so the per-item loop reads the OR predicate as a
///   single short-circuit check, not as N AND-merged sub-constraints.
/// - Every other constraint kind ANDs (file matches only if ALL hold). They're
///   evaluated in order with short-circuit on first failure.
pub(crate) fn apply_constraints<'a, T: Constrainable + Sync>(
    items: &'a [T],
    constraints: &[Constraint<'_>],
    base_arena: ArenaPtr,
    overflow_arena: ArenaPtr,
) -> Option<Vec<&'a T>> {
    if constraints.is_empty() {
        return None;
    }
    let plan = ConstraintPlan::build(constraints, items, base_arena, overflow_arena);
    Some(plan.run(items, base_arena, overflow_arena))
}

#[cfg(feature = "zlob")]
pub(crate) type GlobPattern = zlob::ZlobPattern;
#[cfg(all(not(feature = "zlob"), feature = "ripgrep"))]
pub(crate) type GlobPattern = globset::GlobMatcher;

/// `PERIOD` on top of `RECOMMENDED`: we filter an index that already contains
/// dotfiles, so `**/x` must cross `.config`/`.pi` components. Without it zlob
/// applies fnmatch's leading-dot rule and diverges from the globset backend.
#[cfg(feature = "zlob")]
const GLOB_FLAGS: zlob::ZlobFlags = zlob::ZlobFlags::RECOMMENDED.union(zlob::ZlobFlags::PERIOD);

/// How `Constraint::Glob` is evaluated for each item.
enum GlobStrategy {
    /// No Glob constraint present.
    None,
    /// Pure-glob workload (no Extension filter to reject items first).
    /// Batch all paths through zlob/globset once; per-item check is a Vec<bool> lookup.
    Prepass(Vec<Vec<bool>>),
    /// Mixed workload (Extension filter present). Compile patterns up front, then
    /// only run them on items that survive the cheap Extension OR check.
    /// `None` slot = compile failure -> never matches; preserves index alignment.
    Inline(Vec<Option<GlobPattern>>),
}

/// Bundles preprocessed constraints for the per-item evaluator.
pub(crate) struct ConstraintPlan<'q, 'c> {
    /// OR semantics — file passes if ANY extension matches. Empty = no ext filter.
    extensions: SmallVec<[&'q str; 8]>,
    /// AND semantics — file passes only if ALL match.
    rest: SmallVec<[&'c Constraint<'q>; 8]>,
    glob: GlobStrategy,
}

pub(crate) struct ConstraintsBuffers {
    fname: String,
    path: String,
}

impl ConstraintsBuffers {
    pub(crate) fn new() -> Self {
        Self {
            fname: String::with_capacity(64),
            path: String::with_capacity(64),
        }
    }
}

impl<'q, 'c> ConstraintPlan<'q, 'c> {
    pub(crate) fn build<T: Constrainable>(
        constraints: &'c [Constraint<'q>],
        items: &[T],
        base_arena: ArenaPtr,
        overflow_arena: ArenaPtr,
    ) -> Self {
        let mut extensions = SmallVec::new();
        let mut rest: SmallVec<[&'c Constraint<'q>; 8]> = SmallVec::new();
        for c in constraints {
            match c {
                Constraint::Extension(ext) => extensions.push(*ext),
                _ => rest.push(c),
            }
        }
        let has_pre_filter = !extensions.is_empty() || rest.iter().any(|&c| !is_glob_node(c));
        let glob = build_glob_strategy(&rest, has_pre_filter, items, base_arena, overflow_arena);

        Self {
            extensions,
            rest,
            glob,
        }
    }

    fn run<'a, T: Constrainable + Sync>(
        &self,
        items: &'a [T],
        base_arean: ArenaPtr,
        overflow_arena: ArenaPtr,
    ) -> Vec<&'a T> {
        if items.len() >= PAR_THRESHOLD {
            use rayon::prelude::*;
            items
                .par_iter()
                .enumerate()
                .map_init(ConstraintsBuffers::new, |scratch, (i, item)| {
                    self.matches(item, i, base_arean, overflow_arena, scratch)
                        .then_some(item)
                })
                .flatten()
                .collect()
        } else {
            let mut scratch = ConstraintsBuffers::new();
            items
                .iter()
                .enumerate()
                .filter_map(|(i, item)| {
                    self.matches(item, i, base_arean, overflow_arena, &mut scratch)
                        .then_some(item)
                })
                .collect()
        }
    }

    #[inline]
    pub(crate) fn matches<T: Constrainable>(
        &self,
        item: &T,
        index: usize,
        base_arena: ArenaPtr,
        overflow_arena: ArenaPtr,
        scratch: &mut ConstraintsBuffers,
    ) -> bool {
        let arena = if item.is_overflow() {
            overflow_arena
        } else {
            base_arena
        };

        if !self.passes_extensions(item, arena, scratch) {
            return false;
        }

        let mut glob_idx = 0;
        self.rest.iter().all(|c| {
            evaluate(
                item,
                index,
                c,
                &self.glob,
                &mut glob_idx,
                false,
                arena,
                scratch,
            )
        })
    }

    #[inline]
    fn passes_extensions<T: Constrainable>(
        &self,
        item: &T,
        arena: ArenaPtr,
        scratch: &mut ConstraintsBuffers,
    ) -> bool {
        if self.extensions.is_empty() {
            return true;
        }
        self.extensions
            .iter()
            .any(|ext| item.has_extension(arena, ext, &mut scratch.fname))
    }
}

#[inline]
#[allow(clippy::too_many_arguments)]
fn evaluate<T: Constrainable>(
    item: &T,
    index: usize,
    constraint: &Constraint<'_>,
    glob: &GlobStrategy,
    glob_idx: &mut usize,
    negate: bool,
    arena: ArenaPtr,
    scratch: &mut ConstraintsBuffers,
) -> bool {
    let raw = match constraint {
        Constraint::Glob(_) => {
            let m = match glob {
                GlobStrategy::None => true,
                GlobStrategy::Prepass(masks) => masks
                    .get(*glob_idx)
                    .and_then(|mask| mask.get(index).copied())
                    .unwrap_or(false),
                GlobStrategy::Inline(patterns) => {
                    item.write_relative_path(arena, &mut scratch.path);
                    patterns
                        .get(*glob_idx)
                        .and_then(|p| p.as_ref())
                        .map(|p| compiled_matches(p, &scratch.path))
                        .unwrap_or(false)
                }
            };
            *glob_idx += 1;
            m
        }
        // Reachable only via `Not(Extension(_))` — bare extensions are split out
        // up front and handled in `passes_extensions`.
        Constraint::Extension(ext) => item.has_extension(arena, ext, &mut scratch.fname),
        Constraint::PathSegment(segment) => {
            item.write_relative_path(arena, &mut scratch.path);
            path_contains_segment(&scratch.path, segment)
        }
        Constraint::FilePath(suffix) => {
            item.write_relative_path(arena, &mut scratch.path);
            path_ends_with_suffix(&scratch.path, suffix)
        }
        Constraint::Text(text) => {
            // Only meaningful under negation (used as exclude filter).
            item.write_relative_path(arena, &mut scratch.path);
            find_case_insensitive_short(scratch.path.as_bytes(), text.as_bytes()).is_some()
        }
        Constraint::GitStatus(filter) => matches_git_status(item.git_status(), filter),
        Constraint::Not(inner) => {
            return evaluate(item, index, inner, glob, glob_idx, !negate, arena, scratch);
        }
        // Pass-throughs — handled at higher levels.
        Constraint::Parts(_) | Constraint::Exclude(_) | Constraint::FileType(_) => true,
    };
    if negate { !raw } else { raw }
}

#[inline]
fn matches_git_status(status: Option<git2::Status>, filter: &GitStatusFilter) -> bool {
    match (status, filter) {
        (Some(s), GitStatusFilter::Modified) => is_modified_status(s),
        (Some(s), GitStatusFilter::Untracked) => s.contains(git2::Status::WT_NEW),
        (Some(s), GitStatusFilter::Staged) => s.intersects(
            git2::Status::INDEX_NEW
                | git2::Status::INDEX_MODIFIED
                | git2::Status::INDEX_DELETED
                | git2::Status::INDEX_RENAMED
                | git2::Status::INDEX_TYPECHANGE,
        ),
        (Some(s), GitStatusFilter::Unmodified) => s.is_empty(),
        (None, GitStatusFilter::Unmodified) => true,
        (None, _) => false,
    }
}

#[inline]
#[cfg(feature = "zlob")]
pub(crate) fn compiled_matches(p: &GlobPattern, path: &str) -> bool {
    p.matches_default(path)
}

#[inline]
#[cfg(all(not(feature = "zlob"), feature = "ripgrep"))]
pub(crate) fn compiled_matches(p: &GlobPattern, path: &str) -> bool {
    p.is_match(path)
}

/// Append indices (into `rels`) of paths matching `p`, in input order.
/// zlob backend: ONE FFI call for the whole batch.
#[cfg(feature = "zlob")]
pub(crate) fn glob_matches_into(p: &GlobPattern, rels: &[&str], out: &mut Vec<usize>) {
    match p.match_indices(rels, p.flags()) {
        Ok(ix) => out.extend_from_slice(ix.as_slice()),
        Err(e) => {
            tracing::warn!(?e, "zlob batch match failed, falling back to per-path");
            out.extend((0..rels.len()).filter(|&i| p.matches_default(rels[i])));
        }
    }
}

#[cfg(all(not(feature = "zlob"), feature = "ripgrep"))]
pub(crate) fn glob_matches_into(p: &GlobPattern, rels: &[&str], out: &mut Vec<usize>) {
    out.extend((0..rels.len()).filter(|&i| p.is_match(rels[i])));
}

/// Decide between batch prepass and inline compiled patterns.
///
/// `has_pre_filter` = true when something cheaper than glob can reject items first
/// (extensions OR non-glob constraints in `rest`). In that case inline pays glob
/// cost only on survivors and beats prepass on every workload we benched. Pure-glob
/// (no pre-filter) takes prepass — single batched zlob call beats N inline matches.
fn build_glob_strategy<T: Constrainable>(
    rest: &[&Constraint<'_>],
    has_pre_filter: bool,
    items: &[T],
    arena: ArenaPtr,
    overflow_arena: ArenaPtr,
) -> GlobStrategy {
    if !contains_glob(rest) {
        return GlobStrategy::None;
    }
    if has_pre_filter {
        return GlobStrategy::Inline(compile_globs(rest));
    }
    let buf = PathBuffer::collect(items, arena, overflow_arena);
    let path_refs = buf.as_strs();
    GlobStrategy::Prepass(precompute_masks(rest, &path_refs))
}

/// `Glob` or `Not(Glob)` — the constraint kinds whose evaluation goes through
/// the GlobStrategy. Everything else can pre-reject items before glob runs.
fn is_glob_node(c: &Constraint<'_>) -> bool {
    match c {
        Constraint::Glob(_) => true,
        Constraint::Not(inner) => is_glob_node(inner),
        _ => false,
    }
}

fn contains_glob(rest: &[&Constraint<'_>]) -> bool {
    rest.iter().any(|c| is_glob_node(c))
}

/// Contiguous byte buffer holding every item's `relative_path`. Single allocation
/// instead of N `String`s. On Windows the in-place pass folds `\\` -> `/` so the
/// glob library sees a canonical separator.
struct PathBuffer {
    bytes: Vec<u8>,
    offsets: Vec<(usize, usize)>,
}

impl PathBuffer {
    fn collect<T: Constrainable>(items: &[T], arena: ArenaPtr, overflow_arena: ArenaPtr) -> Self {
        let mut bytes = Vec::<u8>::new();
        let mut offsets = Vec::with_capacity(items.len());
        let mut tmp = String::with_capacity(64);
        for item in items {
            let item_arena = if item.is_overflow() {
                overflow_arena
            } else {
                arena
            };
            let start = bytes.len();
            item.write_relative_path(item_arena, &mut tmp);
            bytes.extend_from_slice(tmp.as_bytes());
            offsets.push((start, bytes.len() - start));
        }
        Self { bytes, offsets }
    }

    fn as_strs(&self) -> Vec<&str> {
        self.offsets
            .iter()
            .map(|&(off, len)| unsafe {
                std::str::from_utf8_unchecked(&self.bytes[off..off + len])
            })
            .collect()
    }
}

fn precompute_masks(rest: &[&Constraint<'_>], paths: &[&str]) -> Vec<Vec<bool>> {
    let mut out = Vec::new();
    for c in rest {
        walk_globs(c, &mut |pattern| {
            out.push(match_glob_pattern(pattern, paths))
        });
    }
    out
}

fn compile_globs(rest: &[&Constraint<'_>]) -> Vec<Option<GlobPattern>> {
    let mut out = Vec::new();
    for c in rest {
        walk_globs(c, &mut |pattern| out.push(compile_one(pattern)));
    }
    out
}

/// Visit every Glob (including ones nested under Not) in constraint walk order.
/// Order matters: `glob_idx` in the per-item evaluator increments by one per Glob node.
fn walk_globs<F: FnMut(&str)>(c: &Constraint<'_>, f: &mut F) {
    match c {
        Constraint::Glob(p) => f(p),
        Constraint::Not(inner) => walk_globs(inner, f),
        _ => {}
    }
}

#[cfg(feature = "zlob")]
pub(crate) fn compile_one(pattern: &str) -> Option<GlobPattern> {
    zlob::ZlobPattern::compile(pattern, GLOB_FLAGS).ok()
}

#[cfg(all(not(feature = "zlob"), feature = "ripgrep"))]
pub(crate) fn compile_one(pattern: &str) -> Option<GlobPattern> {
    globset::Glob::new(pattern)
        .ok()
        .map(|g| g.compile_matcher())
}

/// Build a `paths.len()`-sized bitmap. Vec<bool> beats AHashSet ~2× in the per-item
/// filter loop — no hashing, plain array indexing, sequential prefetcher-friendly.
#[cfg(feature = "zlob")]
fn match_glob_pattern(pattern: &str, paths: &[&str]) -> Vec<bool> {
    let mut mask = vec![false; paths.len()];
    let Ok(hits) = zlob::zlob_match_paths_indices(pattern, paths, GLOB_FLAGS) else {
        return mask;
    };
    for i in hits.to_iter() {
        if i < mask.len() {
            mask[i] = true;
        }
    }
    mask
}

#[cfg(all(not(feature = "zlob"), feature = "ripgrep"))]
fn match_glob_pattern(pattern: &str, paths: &[&str]) -> Vec<bool> {
    let mut mask = vec![false; paths.len()];
    let Ok(glob) = globset::Glob::new(pattern) else {
        return mask;
    };
    let matcher = glob.compile_matcher();
    if paths.len() >= PAR_THRESHOLD {
        use rayon::prelude::*;
        mask.par_iter_mut()
            .zip(paths.par_iter())
            .for_each(|(slot, p)| *slot = matcher.is_match(p));
    } else {
        for (slot, p) in mask.iter_mut().zip(paths.iter()) {
            *slot = matcher.is_match(p);
        }
    }
    mask
}

