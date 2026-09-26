//! C ABI adapter compiled inside the pinned fff-search crate. The upstream
//! matcher, scores, SIMD path storage, query parser and highlights are unchanged.
use crate::{
    score::{fuzzy_match_and_score_files, fuzzy_match_byte_offsets_for_page},
    simd_path::{
        ArenaPtr, ChunkedPathStore, ChunkedPathStoreBuilder, ChunkedString, MAX_PATH_CHUNKS,
    },
    types::{FileItem, PaginationArgs, ScoringContext},
};
use fff_query_parser::{Constraint, FuzzyQuery, MixedSearchConfig, QueryParser};
use memmap2::{Mmap, MmapOptions};
use std::{
    ffi::{CStr, c_char},
    fs::File,
    ops::Range,
    panic::{AssertUnwindSafe, catch_unwind},
    ptr, slice,
    sync::Arc,
};

pub struct CrawlerFFFIndex {
    paths: Vec<ChunkedString>,
    storage: CrawlerFFFStorage,
}
pub struct CrawlerFFFCache {
    mapping: Arc<Mmap>,
}
enum CacheIndices {
    Mapped(Range<usize>),
    Owned(Vec<u32>),
}
enum CrawlerFFFStorage {
    Built(ChunkedPathStore),
    Mapped {
        mapping: Arc<Mmap>,
        chunks: Range<usize>,
        indices: CacheIndices,
    },
}
impl CrawlerFFFStorage {
    fn arena(&self) -> ArenaPtr {
        match self {
            Self::Built(store) => store.as_arena_ptr(),
            Self::Mapped {
                mapping,
                chunks,
                indices,
            } => {
                let indices = match indices {
                    CacheIndices::Mapped(range) => mapping[range.clone()].as_ptr().cast(),
                    CacheIndices::Owned(values) => values.as_ptr(),
                };
                // FFF/neo_frizbee load chunks through unaligned byte pointers.
                // Typed u32 indices are borrowed only after checking alignment.
                ArenaPtr::new(mapping[chunks.clone()].as_ptr(), indices)
            }
        }
    }
    fn storage_bytes(&self) -> usize {
        match self {
            Self::Built(store) => store.heap_bytes(),
            Self::Mapped {
                chunks, indices, ..
            } => {
                chunks.len()
                    + match indices {
                        CacheIndices::Mapped(range) => range.len(),
                        CacheIndices::Owned(values) => values.len() * 4,
                    }
            }
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn crawler_fff_cache_open(path: *const c_char) -> *mut CrawlerFFFCache {
    catch_unwind(AssertUnwindSafe(|| {
        if path.is_null() {
            return ptr::null_mut();
        }
        let Ok(path) = (unsafe { CStr::from_ptr(path) }).to_str() else {
            return ptr::null_mut();
        };
        let Ok(file) = File::open(path) else {
            return ptr::null_mut();
        };
        // App cache writes use atomic replacement, never truncate this inode.
        let Ok(mapping) = (unsafe { MmapOptions::new().map(&file) }) else {
            return ptr::null_mut();
        };
        Box::into_raw(Box::new(CrawlerFFFCache {
            mapping: Arc::new(mapping),
        }))
    }))
    .unwrap_or(ptr::null_mut())
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn crawler_fff_cache_view(cache: *const CrawlerFFFCache) -> CrawlerFFFBytes {
    if cache.is_null() {
        return CrawlerFFFBytes {
            data: ptr::null(),
            count: 0,
        };
    }
    let mapping = &unsafe { &*cache }.mapping;
    CrawlerFFFBytes {
        data: mapping.as_ptr(),
        count: mapping.len(),
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn crawler_fff_cache_destroy(cache: *mut CrawlerFFFCache) {
    if !cache.is_null() {
        drop(unsafe { Box::from_raw(cache) });
    }
}
#[repr(C)]
pub struct CrawlerFFFBytes {
    data: *const u8,
    count: usize,
}
#[repr(C)]
pub struct CrawlerFFFHit {
    index: u32,
    score: i32,
    range_offset: u32,
    range_count: u32,
}
#[repr(C)]
pub struct CrawlerFFFRange {
    start: u32,
    end: u32,
}
#[repr(C)]
pub struct CrawlerFFFResult {
    hits: *const CrawlerFFFHit,
    hit_count: usize,
    ranges: *const CrawlerFFFRange,
    range_count: usize,
    matching_indices: *const u32,
    total: usize,
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn crawler_fff_create(
    paths: *const u8,
    byte_count: usize,
    count: usize,
) -> *mut CrawlerFFFIndex {
    catch_unwind(AssertUnwindSafe(|| {
        if (paths.is_null() && byte_count != 0) || count > u32::MAX as usize {
            return ptr::null_mut();
        }
        let bytes = if byte_count == 0 {
            &[]
        } else {
            unsafe { slice::from_raw_parts(paths, byte_count) }
        };
        let mut builder = ChunkedPathStoreBuilder::new(count);
        let mut paths = Vec::with_capacity(count);
        for raw in bytes.split(|b| *b == 0).take(count) {
            let Ok(path) = std::str::from_utf8(raw) else {
                return ptr::null_mut();
            };
            // FFF's chunk resolver uses a fixed maximum macOS path buffer.
            if path.is_empty() || path.len() >= crate::constants::PATH_BUF_SIZE {
                return ptr::null_mut();
            }
            let filename_offset = path.rfind('/').map_or(0, |i| i + 1) as u16;
            paths.push(builder.add_file_immediate(path, filename_offset));
        }
        if paths.len() != count {
            return ptr::null_mut();
        }
        Box::into_raw(Box::new(CrawlerFFFIndex {
            paths,
            storage: CrawlerFFFStorage::Built(builder.finish()),
        }))
    }))
    .unwrap_or(ptr::null_mut())
}

// Persist FFF's exact SIMD chunks and path references, rather than rebuilding
// them from folded strings at every launch. No upstream scoring data changes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn crawler_fff_serialize(
    index: *const CrawlerFFFIndex,
) -> *mut CrawlerFFFBytes {
    catch_unwind(AssertUnwindSafe(|| {
        if index.is_null() {
            return ptr::null_mut();
        }
        let index = unsafe { &*index };
        let indices_count: usize = index.paths.iter().map(|path| path.chunk_count()).sum();
        let chunk_bytes = index.storage.storage_bytes() - indices_count * 4;
        let arena = index.storage.arena();
        let mut bytes =
            Vec::with_capacity(16 + index.paths.len() * 8 + chunk_bytes + indices_count * 4);
        bytes.extend_from_slice(b"CFI1");
        bytes.extend_from_slice(&(index.paths.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&((chunk_bytes / 16) as u32).to_le_bytes());
        bytes.extend_from_slice(&(indices_count as u32).to_le_bytes());
        let mut offset = 0u32;
        for path in &index.paths {
            bytes.extend_from_slice(&offset.to_le_bytes());
            bytes.extend_from_slice(&path.byte_len.to_le_bytes());
            bytes.extend_from_slice(&path.filename_offset.to_le_bytes());
            offset += path.chunk_count() as u32;
        }
        if chunk_bytes != 0 {
            bytes.extend_from_slice(unsafe { slice::from_raw_parts(arena.as_ptr(), chunk_bytes) });
        }
        let mut pointers = [ptr::null(); MAX_PATH_CHUNKS];
        for path in &index.paths {
            for &chunk in path.resolve_ptrs(arena, &mut pointers) {
                let id = unsafe { chunk.offset_from(arena.as_ptr()) } as usize / 16;
                bytes.extend_from_slice(&(id as u32).to_le_bytes());
            }
        }
        let bytes = bytes.into_boxed_slice();
        Box::into_raw(Box::new(CrawlerFFFBytes {
            count: bytes.len(),
            data: Box::into_raw(bytes).cast(),
        }))
    }))
    .unwrap_or(ptr::null_mut())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn crawler_fff_free_bytes(bytes: *mut CrawlerFFFBytes) {
    if !bytes.is_null() {
        let bytes = unsafe { Box::from_raw(bytes) };
        drop(unsafe {
            Box::from_raw(ptr::slice_from_raw_parts_mut(
                bytes.data as *mut u8,
                bytes.count,
            ))
        });
    }
}

// A lossy hint orders cache loading only. It never excludes FFF candidates.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn crawler_fff_signature(
    index: *const CrawlerFFFIndex,
) -> *mut CrawlerFFFBytes {
    catch_unwind(AssertUnwindSafe(|| {
        if index.is_null() {
            return ptr::null_mut();
        }
        let index = unsafe { &*index };
        let arena = index.storage.arena();
        let mut bits = vec![0u8; 8192];
        let mut buf = [0u8; crate::constants::PATH_BUF_SIZE];
        for path in &index.paths {
            for triple in path.read_to_buf(arena, &mut buf).as_bytes().windows(3) {
                let value = (u32::from(triple[0]) << 16)
                    | (u32::from(triple[1]) << 8)
                    | u32::from(triple[2]);
                let bit = (value.wrapping_mul(2654435761) >> 16) as usize;
                bits[bit / 8] |= 1 << (bit % 8);
            }
        }
        let bits = bits.into_boxed_slice();
        Box::into_raw(Box::new(CrawlerFFFBytes {
            count: bits.len(),
            data: Box::into_raw(bits).cast(),
        }))
    }))
    .unwrap_or(ptr::null_mut())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn crawler_fff_validate_path(path: *const u8, count: usize) -> u8 {
    if path.is_null() || count == 0 {
        return 0;
    }
    let bytes = unsafe { slice::from_raw_parts(path, count) };
    if bytes[0] == b'/'
        || bytes.contains(&0)
        || bytes
            .split(|b| *b == b'/')
            .any(|part| part == b"." || part == b"..")
        || std::str::from_utf8(bytes).is_err()
    {
        return 0;
    }
    if bytes.is_ascii() { 1 } else { 2 }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn crawler_fff_restore_mapped(
    cache: *const CrawlerFFFCache,
    offset: usize,
    length: usize,
    expected_count: usize,
) -> *mut CrawlerFFFIndex {
    catch_unwind(AssertUnwindSafe(|| {
        if cache.is_null() || length < 16 || expected_count == 0 || expected_count > 2048 {
            return ptr::null_mut();
        }
        let mapping = &unsafe { &*cache }.mapping;
        let Some(end) = offset.checked_add(length) else {
            return ptr::null_mut();
        };
        let Some(bytes) = mapping.get(offset..end) else {
            return ptr::null_mut();
        };
        if &bytes[..4] != b"CFI1" {
            return ptr::null_mut();
        }
        let number = |at| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        let count = number(4);
        let chunks_count = number(8);
        let indices_count = number(12);
        if count != expected_count
            || chunks_count > indices_count
            || indices_count > count * MAX_PATH_CHUNKS
            || length != 16 + count * 8 + chunks_count * 16 + indices_count * 4
        {
            return ptr::null_mut();
        }
        let chunk_start = 16 + count * 8;
        let indices_start = chunk_start + chunks_count * 16;
        let index_at = |i| number(indices_start + i * 4);
        if (0..indices_count).any(|i| index_at(i) >= chunks_count) {
            return ptr::null_mut();
        }
        let mut paths = Vec::with_capacity(count);
        let mut next = 0usize;
        let mut path_buf = [0u8; crate::constants::PATH_BUF_SIZE];
        for i in 0..count {
            let at = 16 + i * 8;
            let start = number(at);
            let size = u16::from_le_bytes(bytes[at + 4..at + 6].try_into().unwrap()) as usize;
            let filename = u16::from_le_bytes(bytes[at + 6..at + 8].try_into().unwrap()) as usize;
            let needed = size.div_ceil(16);
            if start != next
                || size == 0
                || size >= path_buf.len()
                || filename > size
                || needed > indices_count - next
            {
                return ptr::null_mut();
            }
            for n in 0..needed {
                let take = (size - n * 16).min(16);
                let chunk = chunk_start + index_at(start + n) * 16;
                path_buf[n * 16..n * 16 + take].copy_from_slice(&bytes[chunk..chunk + take]);
            }
            let Ok(path) = std::str::from_utf8(&path_buf[..size]) else {
                return ptr::null_mut();
            };
            if path.contains('\0') || path.rfind('/').map_or(0, |i| i + 1) != filename {
                return ptr::null_mut();
            }
            paths.push(ChunkedString::new(
                start as u32,
                size as u16,
                filename as u16,
            ));
            next += needed;
        }
        if next != indices_count {
            return ptr::null_mut();
        }
        let indices_range = offset + indices_start..end;
        let indices = if cfg!(target_endian = "little")
            && mapping[indices_range.clone()]
                .as_ptr()
                .align_offset(std::mem::align_of::<u32>())
                == 0
        {
            CacheIndices::Mapped(indices_range)
        } else {
            // Older unaligned snapshots remain valid; copy their index table only.
            CacheIndices::Owned((0..indices_count).map(|i| index_at(i) as u32).collect())
        };
        Box::into_raw(Box::new(CrawlerFFFIndex {
            paths,
            storage: CrawlerFFFStorage::Mapped {
                mapping: Arc::clone(mapping),
                chunks: offset + chunk_start..offset + indices_start,
                indices,
            },
        }))
    }))
    .unwrap_or(ptr::null_mut())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn crawler_fff_destroy(index: *mut CrawlerFFFIndex) {
    if !index.is_null() {
        drop(unsafe { Box::from_raw(index) });
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn crawler_fff_search(
    index: *const CrawlerFFFIndex,
    query: *const c_char,
    candidates: *const u32,
    frecency: *const i16,
    count: usize,
    limit: usize,
    literal_words: u8,
) -> *mut CrawlerFFFResult {
    catch_unwind(AssertUnwindSafe(|| {
        if index.is_null() || query.is_null() || count == 0 {
            return ptr::null_mut();
        }
        let index = unsafe { &*index };
        let Ok(text) = (unsafe { CStr::from_ptr(query) }).to_str() else {
            return ptr::null_mut();
        };
        if candidates.is_null() && count != index.paths.len() {
            return ptr::null_mut();
        }
        let ids =
            (!candidates.is_null()).then(|| unsafe { slice::from_raw_parts(candidates, count) });
        let boosts =
            (!frecency.is_null()).then(|| unsafe { slice::from_raw_parts(frecency, count) });
        // Keep only compact path references resident. Content/metadata fields
        // are unused by filename search; materialize bounded scoring inputs here.
        let mut files = Vec::with_capacity(count);
        for position in 0..count {
            let id = ids.map_or(position as u32, |ids| ids[position]);
            let Some(path) = index.paths.get(id as usize) else {
                return ptr::null_mut();
            };
            let mut file = FileItem::new_raw(path.filename_offset, 0, 0, None, false);
            file.set_path(path.clone());
            file.access_frecency_score = boosts.map_or(0, |boosts| boosts[position]);
            files.push(file);
        }
        let parser = QueryParser::new(MixedSearchConfig);
        let mut query = parser.parse(text);
        // Computer-wide natural phrases require every typed term. Use FFF's
        // own literal constraints (any order) before its normal scoring;
        // independent typo allowances otherwise make short words match almost
        // the entire Mac. Single-token abbreviations retain FFF fuzzy matching.
        if literal_words != 0
            && let FuzzyQuery::Parts(parts) = &query.fuzzy_query
        {
            query
                .constraints
                .extend(parts.iter().copied().map(Constraint::Text));
        }
        // FFF intentionally doesn't fuzzy-score one-byte needles. Its literal
        // constraint keeps a single typed letter useful instead of ranking every file.
        if text.len() == 1 {
            query.fuzzy_query = FuzzyQuery::Empty;
            query.constraints.push(Constraint::Text(text));
        }
        let effective = match &query.fuzzy_query {
            FuzzyQuery::Text(t) => *t,
            FuzzyQuery::Parts(p) if !p.is_empty() => p[0],
            _ => text,
        };
        let max_typos = (effective.len() as u16 / 4).clamp(2, 6);
        let context = ScoringContext {
            query: &query,
            project_path: None,
            current_file: None,
            max_typos,
            max_threads: 1,
            last_same_query_match: None,
            combo_boost_score_multiplier: 0,
            min_combo_count: 0,
            // Retain every match for accurate scope counts. Each snapshot is bounded.
            pagination: PaginationArgs {
                offset: 0,
                limit: 0,
            },
        };
        let arena = index.storage.arena();
        let (mut items, mut scores, total) =
            fuzzy_match_and_score_files(&files, &context, files.len(), arena, arena);
        let matching: Box<[u32]> = items
            .iter()
            .map(|item| {
                let position = unsafe { ptr::from_ref(*item).offset_from(files.as_ptr()) } as usize;
                ids.map_or(position as u32, |ids| ids[position])
            })
            .collect();
        items.truncate(limit);
        scores.truncate(limit);
        let offsets = fuzzy_match_byte_offsets_for_page(&query, &items, max_typos, arena, arena);
        let mut ranges = Vec::new();
        let hits: Box<[CrawlerFFFHit]> = items
            .iter()
            .zip(scores)
            .zip(offsets)
            .map(|((item, score), offsets)| {
                let position = unsafe { ptr::from_ref(*item).offset_from(files.as_ptr()) } as usize;
                let range_offset = ranges.len() as u32;
                let range_count = offsets.len() as u32;
                ranges.extend(
                    offsets
                        .into_iter()
                        .map(|(start, end)| CrawlerFFFRange { start, end }),
                );
                CrawlerFFFHit {
                    index: ids.map_or(position as u32, |ids| ids[position]),
                    score: score.total,
                    range_offset,
                    range_count,
                }
            })
            .collect();
        let ranges = ranges.into_boxed_slice();
        let result = CrawlerFFFResult {
            hit_count: hits.len(),
            hits: Box::into_raw(hits) as *const CrawlerFFFHit,
            range_count: ranges.len(),
            ranges: Box::into_raw(ranges) as *const CrawlerFFFRange,
            matching_indices: Box::into_raw(matching) as *const u32,
            total,
        };
        Box::into_raw(Box::new(result))
    }))
    .unwrap_or(ptr::null_mut())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn crawler_fff_free_result(result: *mut CrawlerFFFResult) {
    if result.is_null() {
        return;
    }
    let r = unsafe { Box::from_raw(result) };
    unsafe {
        drop(Box::from_raw(ptr::slice_from_raw_parts_mut(
            r.hits as *mut CrawlerFFFHit,
            r.hit_count,
        )));
        drop(Box::from_raw(ptr::slice_from_raw_parts_mut(
            r.ranges as *mut CrawlerFFFRange,
            r.range_count,
        )));
        drop(Box::from_raw(ptr::slice_from_raw_parts_mut(
            r.matching_indices as *mut u32,
            r.total,
        )));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn crawler_fff_revision() -> *const c_char {
    c"c188e7a8f90e4ee4aabe18f93d5869dfb4289f12".as_ptr()
}
