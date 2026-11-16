//! Immutable bounded path snapshots. Original names and FFF's prepared chunks share a mapping.
use fff_search::crawler::{
    ArenaPtr, ChunkedPathStore, ChunkedPathStoreBuilder, ChunkedString, MAX_PATH_CHUNKS,
};
use memmap2::Mmap;
use objc2_foundation::{NSLocale, NSString, NSStringCompareOptions};
use std::{ops::Range, sync::Arc};
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

pub const BATCH: usize = 2048;
pub const REVISION: &str = "c188e7a8f90e4ee4aabe18f93d5869dfb4289f12";

pub fn folded(text: &str) -> String {
    if text.is_ascii() {
        return text.to_ascii_lowercase();
    }
    // Use the same ICU folding and locale as Swift for non-ASCII paths and queries.
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
    range: Range<usize>,
    directory: bool,
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
        indices: Range<usize>,
    },
}
pub struct Snapshot {
    originals: Originals,
    entries: Vec<Entry>,
    pub paths: Vec<ChunkedString>,
    prepared: Prepared,
}
impl Snapshot {
    pub fn build(rows: &[(String, bool)]) -> Self {
        let mut originals = Vec::new();
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
            let start = originals.len();
            originals.extend_from_slice(path.as_bytes());
            entries.push(Entry {
                range: start..originals.len(),
                directory: *directory,
            });
            paths.push(builder.add_file_immediate(
                &normalized,
                normalized.rfind('/').map_or(0, |n| n + 1) as u16,
            ));
        }
        Self {
            originals: Originals::Owned(originals),
            entries,
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
        // Both construction paths validate UTF-8 and range boundaries.
        std::str::from_utf8(&bytes[self.entries[row].range.clone()])
            .expect("validated snapshot UTF-8")
    }
    pub fn directory(&self, row: usize) -> bool {
        self.entries[row].directory
    }
    pub fn arena(&self) -> ArenaPtr {
        match &self.prepared {
            Prepared::Owned(store) => store.as_arena_ptr(),
            Prepared::Mapped {
                map,
                chunks,
                indices,
            } => ArenaPtr::new(
                map[chunks.clone()].as_ptr(),
                map[indices.clone()].as_ptr().cast(),
            ),
        }
    }
    pub fn bytes(&self) -> usize {
        self.entries.len() * (std::mem::size_of::<Entry>() + std::mem::size_of::<ChunkedString>())
            + match &self.prepared {
                Prepared::Owned(s) => {
                    s.heap_bytes()
                        + match &self.originals {
                            Originals::Owned(v) => v.len(),
                            _ => 0,
                        }
                }
                Prepared::Mapped {
                    chunks, indices, ..
                } => chunks.len() + indices.len(),
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
    pub fn decode(map: Arc<Mmap>, range: Range<usize>) -> Option<Self> {
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
        for _ in 0..count {
            let size = u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?) as usize;
            let directory = *bytes.get(at + 2)?;
            at += 3;
            let path = std::str::from_utf8(bytes.get(at..at.checked_add(size)?)?).ok()?;
            if size == 0
                || path.starts_with('/')
                || path.contains('\0')
                || path
                    .split('/')
                    .any(|c| c.is_empty() || c == "." || c == "..")
                || directory > 1
            {
                return None;
            }
            entries.push(Entry {
                range: range.start + at..range.start + at + size,
                directory: directory == 1,
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
            // Validate prepared strings against the original before exposing an arena to FFF.
            let mut prepared = Vec::with_capacity(usize::from(size));
            for n in 0..needed {
                let chunk = chunks + number(indices + (offset + n) * 4)? * 16;
                let take = (usize::from(size) - n * 16).min(16);
                prepared.extend_from_slice(bytes.get(chunk..chunk + take)?);
            }
            let text = std::str::from_utf8(&prepared).ok()?;
            let original = std::str::from_utf8(map.get(entry.range.clone())?).ok()?;
            if folded(original) != text
                || text.rfind('/').map_or(0, |n| n + 1) != usize::from(filename)
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
            chunks: range.start + chunks..range.start + indices,
            indices: range.start + indices..range.end,
        };
        Some(Self {
            originals: Originals::Mapped(map),
            entries,
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
