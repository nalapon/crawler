//! Immutable bounded path snapshots. Original names and FFF's prepared chunks share a mapping.
use fff_search::crawler::{
    ArenaPtr, ChunkedPathStore, ChunkedPathStoreBuilder, ChunkedString, MAX_PATH_CHUNKS,
};
use memmap2::Mmap;
use objc2_foundation::{NSLocale, NSString, NSStringCompareOptions};
use std::{
    ops::Range,
    sync::{Arc, OnceLock},
};
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

pub const BATCH: usize = 2048;
const _: () = assert!(BATCH < u16::MAX as usize);
pub const REVISION: &str = "c188e7a8f90e4ee4aabe18f93d5869dfb4289f12";

pub fn folded(text: &str) -> String {
    if text.is_ascii() {
        return text.to_ascii_lowercase();
    }
    // Foundation's ICU folding with a fixed locale, for non-ASCII paths and queries. Caches
    // saved with this folding stay valid.
    objc2::rc::autoreleasepool(|_| {
        let locale = NSLocale::localeWithLocaleIdentifier(&NSString::from_str("en_US_POSIX"));
        NSString::from_str(text)
            .stringByFoldingWithOptions_locale(
                NSStringCompareOptions::CaseInsensitiveSearch
                    | NSStringCompareOptions::DiacriticInsensitiveSearch,
                Some(&locale),
            )
            .to_string()
    })
}
pub fn equal(a: &str, b: &str) -> bool {
    a == b || (!a.is_ascii() && !b.is_ascii() && a.nfc().eq(b.nfc()))
}

struct Entry {
    start: u32,
    length: u16,
    flags: u8,
}
impl Entry {
    fn range(&self) -> Range<usize> {
        self.start as usize..self.start as usize + usize::from(self.length)
    }
}
/// Row offsets of the lexicographically first and last original paths.
#[derive(Default)]
struct Bounds {
    first: u16,
    last: u16,
}
impl Bounds {
    fn include(&mut self, entries: &[Entry], bytes: &[u8], path: &str) {
        if entries.is_empty() {
            return;
        }
        let row = entries.len() as u16;
        if path.as_bytes() < &bytes[entries[self.first as usize].range()] {
            self.first = row;
        }
        if path.as_bytes() > &bytes[entries[self.last as usize].range()] {
            self.last = row;
        }
    }
}
enum Originals {
    Owned(Vec<u8>),
    Mapped(Arc<Mmap>),
}
enum Prepared {
    Owned(ChunkedPathStore),
    Mapped {
        map: Arc<Mmap>,
        chunks: Range<usize>,
        encoded_block: Option<Range<usize>>,
        indices: Range<usize>,
    },
}
pub struct Snapshot {
    original_bytes: usize,
    original_start: usize,
    originals: Originals,
    entries: Vec<Entry>,
    // Built only when a filesystem change needs a subtree lookup. Two bytes per path.
    sorted_rows: OnceLock<Vec<u16>>,
    bounds: Bounds,
    pub paths: Vec<ChunkedString>,
    prepared: Prepared,
}
impl Snapshot {
    pub fn build(rows: &[(String, bool)]) -> Self {
        assert!(rows.len() <= BATCH, "bounded snapshot");
        let mut originals = Vec::new();
        let mut bounds = Bounds::default();
        let mut entries = Vec::new();
        let mut builder = ChunkedPathStoreBuilder::new(rows.len());
        let mut paths = Vec::new();
        for (path, directory) in rows {
            let normalized = folded(path);
            if normalized.is_empty()
                || normalized.len() >= fff_search::constants::PATH_BUF_SIZE
                || path.len() > u16::MAX as usize
            {
                continue;
            }
            let start = u32::try_from(originals.len()).expect("bounded snapshot offsets");
            bounds.include(&entries, &originals, path);
            originals.extend_from_slice(path.as_bytes());
            entries.push(Entry {
                start,
                length: path.len() as u16,
                flags: u8::from(*directory) | (u8::from(path.is_ascii()) << 1),
            });
            paths.push(builder.add_file_immediate(
                &normalized,
                normalized.rfind('/').map_or(0, |n| n + 1) as u16,
            ));
        }
        Self {
            original_bytes: originals.len(),
            original_start: 0,
            originals: Originals::Owned(originals),
            entries,
            sorted_rows: OnceLock::new(),
            bounds,
            paths,
            prepared: Prepared::Owned(builder.finish()),
        }
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn path(&self, row: usize) -> &str {
        let bytes: &[u8] = match &self.originals {
            Originals::Owned(v) => v,
            Originals::Mapped(v) => v,
        };
        let entry = &self.entries[row];
        let start = self.original_start + entry.start as usize;
        // SAFETY: build copies a str; decode validates every original once. The private
        // entry offsets and immutable backing allocation preserve those boundaries.
        unsafe { std::str::from_utf8_unchecked(&bytes[start..start + usize::from(entry.length)]) }
    }
    /// A cheap exclusion check for an exact path or its slash-delimited descendants.
    pub fn may_overlap_prefix(&self, prefix: &str) -> bool {
        if self.len() == 0 {
            return false;
        }
        let first = self.path(self.bounds.first as usize);
        let last = self.path(self.bounds.last as usize);
        last >= prefix
            && (first <= prefix
                || first
                    .strip_prefix(prefix)
                    .and_then(|tail| tail.as_bytes().first())
                    .is_some_and(|b| *b <= b'/'))
    }
    /// Return the exact path and all descendants for each changed prefix. Batches remain
    /// in their original order for matching and persistence.
    pub fn affected_rows(&self, prefixes: &foldhash::HashSet<String>) -> Vec<u16> {
        if self.len() == 0 {
            return Vec::new();
        }
        if prefixes.contains("") {
            return (0..self.len() as u16).collect();
        }
        let mut candidates = prefixes
            .iter()
            .filter(|prefix| self.may_overlap_prefix(prefix))
            .peekable();
        if candidates.peek().is_none() {
            return Vec::new();
        }
        if prefixes.len() > self.len() / 16 {
            return (0..self.len())
                .filter(|&r| super::discovery::ancestors_contain(self.path(r), prefixes))
                .map(|r| r as u16)
                .collect();
        }
        let sorted = self.sorted_rows.get_or_init(|| {
            let mut rows: Vec<_> = (0..self.len() as u16).collect();
            rows.sort_unstable_by_key(|&r| self.path(r as usize));
            rows
        });
        let mut affected = Vec::new();
        for prefix in candidates {
            let at = sorted.partition_point(|&r| self.path(r as usize) < prefix.as_str());
            if let Some(&r) = sorted.get(at)
                && self.path(r as usize) == prefix
            {
                affected.push(r);
            }
            // Compare against prefix + '/' without allocating that string for every block.
            let start = sorted[at..].partition_point(|&r| {
                self.path(r as usize)
                    .as_bytes()
                    .strip_prefix(prefix.as_bytes())
                    .is_some_and(|tail| tail.first().is_none_or(|&b| b < b'/'))
            }) + at;
            affected.extend(sorted[start..].iter().copied().take_while(|&r| {
                self.path(r as usize)
                    .strip_prefix(prefix.as_str())
                    .is_some_and(|s| s.starts_with('/'))
            }));
        }
        affected
    }
    pub fn directory(&self, row: usize) -> bool {
        self.entries[row].flags & 1 != 0
    }
    pub fn ascii(&self, row: usize) -> bool {
        self.entries[row].flags & 2 != 0
    }
    pub fn arena(&self) -> ArenaPtr {
        match &self.prepared {
            Prepared::Owned(store) => store.as_arena_ptr(),
            Prepared::Mapped {
                map,
                chunks,
                indices,
                ..
            } => ArenaPtr::new(
                map[chunks.clone()].as_ptr(),
                map[indices.clone()].as_ptr().cast(),
            ),
        }
    }
    pub fn bytes(&self) -> usize {
        self.entries.len() * (std::mem::size_of::<Entry>() + std::mem::size_of::<ChunkedString>())
            + self.sorted_rows.get().map_or(0, |rows| rows.capacity() * 2)
            + self.original_bytes
            + match &self.prepared {
                Prepared::Owned(store) => store.heap_bytes(),
                Prepared::Mapped {
                    chunks, indices, ..
                } => chunks.len() + indices.len(),
            }
    }
    /// A validated current-format record can be copied unchanged to the next cache file.
    pub fn encoded_block(&self) -> Option<&[u8]> {
        match &self.prepared {
            Prepared::Mapped {
                map,
                encoded_block: Some(range),
                ..
            } => Some(&map[range.clone()]),
            _ => None,
        }
    }
    pub fn signature(&self) -> Vec<u8> {
        let mut bits = vec![0u8; 8192];
        let mut buffer = [0; fff_search::constants::PATH_BUF_SIZE];
        for path in &self.paths {
            for triple in path
                .read_to_buf(self.arena(), &mut buffer)
                .as_bytes()
                .windows(3)
            {
                let bit = trigram_bit(triple);
                bits[bit / 8] |= 1 << (bit % 8);
            }
        }
        bits
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.len() as u32).to_le_bytes());
        for row in 0..self.len() {
            out.extend_from_slice(&(self.path(row).len() as u16).to_le_bytes());
            out.push(u8::from(self.directory(row)));
            out.extend_from_slice(self.path(row).as_bytes());
        }
        while !out.len().is_multiple_of(8) {
            out.push(0);
        }
        let indices_count: usize = self.paths.iter().map(ChunkedString::chunk_count).sum();
        let chunk_bytes = match &self.prepared {
            Prepared::Owned(s) => s.heap_bytes() - indices_count * 4,
            Prepared::Mapped { chunks, .. } => chunks.len(),
        };
        out.extend_from_slice(b"CFI1");
        out.extend_from_slice(&(self.len() as u32).to_le_bytes());
        out.extend_from_slice(&((chunk_bytes / 16) as u32).to_le_bytes());
        out.extend_from_slice(&(indices_count as u32).to_le_bytes());
        let mut start = 0u32;
        for path in &self.paths {
            out.extend_from_slice(&start.to_le_bytes());
            out.extend_from_slice(&path.byte_len.to_le_bytes());
            out.extend_from_slice(&path.filename_offset.to_le_bytes());
            start += path.chunk_count() as u32;
        }
        let arena = self.arena();
        // The immutable arena owns exactly chunk_bytes readable bytes and out copies them.
        out.extend_from_slice(unsafe { std::slice::from_raw_parts(arena.as_ptr(), chunk_bytes) });
        let mut pointers = [std::ptr::null(); MAX_PATH_CHUNKS];
        for path in &self.paths {
            for &chunk in path.resolve_ptrs(arena, &mut pointers) {
                // Every resolved chunk belongs to this same arena allocation.
                let id = unsafe { chunk.offset_from(arena.as_ptr()) } as usize / 16;
                out.extend_from_slice(&(id as u32).to_le_bytes());
            }
        }
        out
    }
    pub fn decode(
        map: Arc<Mmap>,
        range: Range<usize>,
        legacy: bool,
        encoded_block: Option<Range<usize>>,
    ) -> Option<Self> {
        let bytes = map.get(range.clone())?;
        let number = |at: usize| -> Option<usize> {
            Some(u32::from_le_bytes(bytes.get(at..at.checked_add(4)?)?.try_into().ok()?) as usize)
        };
        let count = number(0)?;
        if count == 0 || count > BATCH {
            return None;
        }
        let mut at = 4;
        let mut entries = Vec::with_capacity(count);
        let mut bounds = Bounds::default();
        for _ in 0..count {
            let size = u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?) as usize;
            let directory = *bytes.get(at + 2)?;
            at += 3;
            let path = std::str::from_utf8(bytes.get(at..at.checked_add(size)?)?).ok()?;
            if size == 0
                || (legacy
                    && (path.starts_with('/')
                        || path.contains('\0')
                        || path
                            .split('/')
                            .any(|c| c.is_empty() || c == "." || c == "..")))
                || directory > 1
            {
                return None;
            }
            bounds.include(&entries, bytes, path);
            entries.push(Entry {
                start: u32::try_from(at).ok()?,
                length: size as u16,
                flags: directory | (u8::from(path.is_ascii()) << 1),
            });
            at += size;
        }
        at = at.next_multiple_of(8);
        if bytes.get(at..at + 4)? != b"CFI1" || number(at + 4)? != count {
            return None;
        }
        let chunks_count = number(at + 8)?;
        let indices_count = number(at + 12)?;
        if chunks_count > indices_count || indices_count > count * MAX_PATH_CHUNKS {
            return None;
        }
        let refs = at + 16;
        let chunks = refs + count * 8;
        let indices = chunks + chunks_count * 16;
        if indices.checked_add(indices_count * 4)? != bytes.len() {
            return None;
        }
        if !(range.start + indices).is_multiple_of(std::mem::align_of::<u32>())
            || !cfg!(target_endian = "little")
        {
            return None;
        }
        for i in 0..indices_count {
            if number(indices + i * 4)? >= chunks_count {
                return None;
            }
        }
        let mut paths = Vec::with_capacity(count);
        let mut next = 0;
        let mut buffer = [0u8; fff_search::constants::PATH_BUF_SIZE];
        for (row, entry) in entries.iter().enumerate() {
            let p = refs + row * 8;
            let offset = number(p)?;
            let size = u16::from_le_bytes(bytes.get(p + 4..p + 6)?.try_into().ok()?);
            let filename = u16::from_le_bytes(bytes.get(p + 6..p + 8)?.try_into().ok()?);
            let needed = usize::from(size).div_ceil(16);
            if offset != next
                || size == 0
                || usize::from(size) >= fff_search::constants::PATH_BUF_SIZE
                || filename > size
                || next + needed > indices_count
            {
                return None;
            }
            // Validate UTF-8 even for checksummed data: FFF creates unchecked str views.
            // Only legacy caches need the expensive normalization/equivalence check.
            let prepared = &mut buffer[..usize::from(size)];
            for n in 0..needed {
                let chunk = chunks + number(indices + (offset + n) * 4)? * 16;
                let take = (usize::from(size) - n * 16).min(16);
                prepared[n * 16..n * 16 + take].copy_from_slice(bytes.get(chunk..chunk + take)?);
            }
            let text = std::str::from_utf8(&prepared).ok()?;
            if !text.is_char_boundary(usize::from(filename))
                || (legacy
                    && (folded(std::str::from_utf8(bytes.get(entry.range())?).ok()?) != text
                        || text.rfind('/').map_or(0, |n| n + 1) != usize::from(filename)))
            {
                return None;
            }
            paths.push(ChunkedString::new(offset as u32, size, filename));
            next += needed;
        }
        if next != indices_count {
            return None;
        }
        let prepared = Prepared::Mapped {
            map: Arc::clone(&map),
            encoded_block,
            chunks: range.start + chunks..range.start + indices,
            indices: range.start + indices..range.end,
        };
        Some(Self {
            original_bytes: entries.iter().map(|entry| usize::from(entry.length)).sum(),
            original_start: range.start,
            originals: Originals::Mapped(map),
            entries,
            sorted_rows: OnceLock::new(),
            bounds,
            paths,
            prepared,
        })
    }
}

pub fn original_ranges(path: &str, offsets: &[(u32, u32)]) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = Vec::new();
    let mut folded_at = 0;
    for (at, grapheme) in path.grapheme_indices(true) {
        let end = folded_at + folded(grapheme).len();
        if offsets
            .iter()
            .any(|&(a, b)| (a as usize) < end && (b as usize) > folded_at)
        {
            if let Some(last) = ranges.last_mut()
                && last.end == at
            {
                last.end += grapheme.len();
            } else {
                ranges.push(at..at + grapheme.len());
            }
        }
        folded_at = end;
    }
    ranges
}

pub fn trigram_bit(bytes: &[u8]) -> usize {
    let value = (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2]);
    (value.wrapping_mul(2654435761) >> 16) as usize
}
