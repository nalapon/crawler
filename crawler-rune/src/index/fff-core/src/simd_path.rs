use ahash::AHashMap;
use std::borrow::Cow;

/// SIMD chunk size in bytes (matches NEON/SSE2 register width).
/// This must stay in sync with neo_frizbee's internal chunk size.
pub(crate) const SIMD_CHUNK_BYTES: usize = 16;

/// Read-only view of a path store: 16-byte chunk arena plus the flat table of
/// per-path chunk indices. Both point into the owning store's Vecs.
#[derive(Clone, Copy)]
pub struct ArenaPtr {
    chunks: *const u8,
    indices: *const u32,
}

// SAFETY: The arena is a read-only immutable part of file sync
unsafe impl Send for ArenaPtr {}
unsafe impl Sync for ArenaPtr {}

impl ArenaPtr {
    #[inline]
    pub fn new(chunks: *const u8, indices: *const u32) -> Self {
        Self { chunks, indices }
    }

    #[inline]
    pub fn null() -> Self {
        Self {
            chunks: std::ptr::null(),
            indices: std::ptr::null(),
        }
    }

    #[inline]
    pub fn as_ptr(self) -> *const u8 {
        self.chunks
    }

    #[inline]
    fn chunk_ptr(self, idx: u32) -> *const u8 {
        unsafe { self.chunks.add(idx as usize * SIMD_CHUNK_BYTES) }
    }
}

impl std::fmt::Debug for ArenaPtr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "--arena-raw-pointer-0x({:?})", self.chunks)
    }
}

#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub(crate) struct SimdChunk(pub(crate) [u8; SIMD_CHUNK_BYTES]);

impl Default for SimdChunk {
    #[inline]
    fn default() -> Self {
        Self([0u8; SIMD_CHUNK_BYTES])
    }
}

impl std::fmt::Debug for SimdChunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Show the actual bytes, trimming trailing zeros for readability
        let end = self.0.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
        write!(f, "SimdChunk({:?})", &self.0[..end])
    }
}

pub use crate::constants::PATH_BUF_SIZE;

/// Chunk pointer capacity needed for the longest path the platform allows.
pub const MAX_PATH_CHUNKS: usize = PATH_BUF_SIZE.div_ceil(SIMD_CHUNK_BYTES);

/// A path stored as a run of chunk indices in the owning store's flat index
/// table. 8 bytes; every read needs the store's `ArenaPtr`.
#[derive(Clone)]
pub struct ChunkedString {
    index_offset: u32,
    pub byte_len: u16,
    /// Byte offset where the filename begins. 0 for root-level files.
    pub filename_offset: u16,
}

impl ChunkedString {
    pub fn empty() -> Self {
        Self {
            index_offset: 0,
            byte_len: 0,
            filename_offset: 0,
        }
    }

    #[inline]
    pub fn new(index_offset: u32, byte_len: u16, filename_offset: u16) -> Self {
        Self {
            index_offset,
            byte_len,
            filename_offset,
        }
    }

    #[inline]
    pub fn chunk_count(&self) -> usize {
        chunks_needed(self.byte_len as usize)
    }

    #[inline]
    pub(crate) fn equals(&self, arena: ArenaPtr, other: &str) -> bool {
        if other.len() != self.byte_len as usize {
            return false;
        }
        self.indices(arena)
            .iter()
            .zip(other.as_bytes().chunks(SIMD_CHUNK_BYTES))
            .all(|(&index, expected)| {
                let actual =
                    unsafe { core::slice::from_raw_parts(arena.chunk_ptr(index), expected.len()) };
                actual == expected
            })
    }

    #[inline]
    pub(crate) fn common_suffix_len_ignore_ascii_case(
        &self,
        arena: ArenaPtr,
        other: &[u8],
    ) -> usize {
        let indices = self.indices(arena);
        let mut end = self.byte_len as usize;
        let mut matched = 0;
        while end > 0 && matched < other.len() {
            let chunk = (end - 1) / SIMD_CHUNK_BYTES;
            let len = end - chunk * SIMD_CHUNK_BYTES;
            let bytes =
                unsafe { core::slice::from_raw_parts(arena.chunk_ptr(indices[chunk]), len) };
            for (&a, &b) in bytes
                .iter()
                .rev()
                .zip(other[..other.len() - matched].iter().rev())
            {
                if !a.eq_ignore_ascii_case(&b) {
                    return matched;
                }
                matched += 1;
            }
            end -= len;
        }
        matched
    }

    #[inline]
    pub(crate) fn has_extension(&self, arena: ArenaPtr, extension: &str) -> bool {
        let filename_len = (self.byte_len - self.filename_offset) as usize;
        if filename_len <= extension.len() + 1 {
            return false;
        }
        let dot = self.byte_len as usize - extension.len() - 1;
        let chunk = self.indices(arena)[dot / SIMD_CHUNK_BYTES];
        let byte = unsafe { *arena.chunk_ptr(chunk).add(dot % SIMD_CHUNK_BYTES) };
        byte == b'.'
            && self.common_suffix_len_ignore_ascii_case(arena, extension.as_bytes())
                == extension.len()
    }

    #[inline]
    fn indices<'a>(&self, arena: ArenaPtr) -> &'a [u32] {
        let count = self.chunk_count();
        if count == 0 {
            return &[];
        }
        unsafe { core::slice::from_raw_parts(arena.indices.add(self.index_offset as usize), count) }
    }

    #[inline]
    pub fn resolve_ptrs<'a>(&self, arena: ArenaPtr, buf: &'a mut [*const u8]) -> &'a [*const u8] {
        let indices = self.indices(arena);
        let count = indices.len().min(buf.len());
        for (slot, &idx) in buf.iter_mut().zip(&indices[..count]) {
            *slot = arena.chunk_ptr(idx);
        }
        &buf[..count]
    }

    #[inline]
    fn write_slice_to_vec(
        indices: &[u32],
        arena: ArenaPtr,
        offset_in_chunk: usize,
        len: usize,
        vec: &mut Vec<u8>,
    ) {
        let mut written = 0usize;
        for (i, &idx) in indices.iter().enumerate() {
            let chunk_bytes =
                unsafe { core::slice::from_raw_parts(arena.chunk_ptr(idx), SIMD_CHUNK_BYTES) };
            let start = if i == 0 { offset_in_chunk } else { 0 };
            let end = SIMD_CHUNK_BYTES.min(start + (len - written));
            vec.extend_from_slice(&chunk_bytes[start..end]);
            written += end - start;
        }
    }

    /// Borrow filenames contained in one chunk; copy filenames spanning chunks.
    #[inline]
    pub fn filename_cow<'a>(&self, arena: ArenaPtr) -> Cow<'a, str> {
        let fname_offset = self.filename_offset as usize;
        let fname_len = self.byte_len as usize - fname_offset;
        if fname_len == 0 {
            return Cow::Borrowed("");
        }

        let indices = self.indices(arena);
        let start_chunk = fname_offset / SIMD_CHUNK_BYTES;
        let offset_in_chunk = fname_offset % SIMD_CHUNK_BYTES;

        if offset_in_chunk + fname_len <= SIMD_CHUNK_BYTES {
            let ptr = arena.chunk_ptr(indices[start_chunk]);
            let slice = unsafe { core::slice::from_raw_parts(ptr.add(offset_in_chunk), fname_len) };
            return Cow::Borrowed(unsafe { core::str::from_utf8_unchecked(slice) });
        }

        let mut out = String::with_capacity(fname_len);
        let needed_chunks = chunks_needed(offset_in_chunk + fname_len);
        Self::write_slice_to_vec(
            &indices[start_chunk..start_chunk + needed_chunks],
            arena,
            offset_in_chunk,
            fname_len,
            unsafe { out.as_mut_vec() },
        );
        Cow::Owned(out)
    }

    /// Truncates at `buf.len()` if exceeded -- use `[u8; PATH_BUF_SIZE]` to avoid.
    #[inline]
    pub fn read_to_buf<'a>(&self, arena: ArenaPtr, buf: &'a mut [u8]) -> &'a str {
        let total = (self.byte_len as usize).min(buf.len());
        let indices = self.indices(arena);
        let chunks_to_copy = total.div_ceil(SIMD_CHUNK_BYTES).min(indices.len());

        for (i, &idx) in indices[..chunks_to_copy].iter().enumerate() {
            let dst_offset = i * SIMD_CHUNK_BYTES;
            let take = SIMD_CHUNK_BYTES.min(total - dst_offset);

            unsafe {
                core::ptr::copy_nonoverlapping(
                    arena.chunk_ptr(idx),
                    buf.as_mut_ptr().add(dst_offset),
                    take,
                );
            }
        }

        unsafe { core::str::from_utf8_unchecked(&buf[..total]) }
    }

    #[inline]
    pub fn write_dir_to(&self, arena: ArenaPtr, out: &mut String) {
        out.clear();

        let dir_len = self.filename_offset as usize;
        out.reserve(dir_len);
        let indices = self.indices(arena);
        let dir_chunks = chunks_needed(dir_len).min(indices.len());
        let vec = unsafe { out.as_mut_vec() };
        for (i, &idx) in indices[..dir_chunks].iter().enumerate() {
            let take = SIMD_CHUNK_BYTES.min(dir_len - i * SIMD_CHUNK_BYTES);
            vec.extend_from_slice(unsafe {
                core::slice::from_raw_parts(arena.chunk_ptr(idx), take)
            });
        }
    }

    #[inline]
    pub fn write_filename_to(&self, arena: ArenaPtr, out: &mut String) {
        out.clear();

        let fname_offset = self.filename_offset as usize;
        let fname_len = self.byte_len as usize - fname_offset;
        out.reserve(fname_len);
        let indices = self.indices(arena);
        let start_chunk = fname_offset / SIMD_CHUNK_BYTES;
        let offset_in_chunk = fname_offset % SIMD_CHUNK_BYTES;
        let needed_chunks = chunks_needed(offset_in_chunk + fname_len);
        Self::write_slice_to_vec(
            &indices[start_chunk..start_chunk + needed_chunks],
            arena,
            offset_in_chunk,
            fname_len,
            unsafe { out.as_mut_vec() },
        );
    }

    #[inline]
    pub fn write_to_string(&self, arena: ArenaPtr, out: &mut String) {
        out.clear();
        self.append_to_string(arena, out);
    }

    pub(crate) fn append_to_string(&self, arena: ArenaPtr, out: &mut String) {
        let total = self.byte_len as usize;
        if total == 0 {
            return;
        }
        out.reserve(total);
        let vec = unsafe { out.as_mut_vec() };
        for (i, &idx) in self.indices(arena).iter().enumerate() {
            let take = SIMD_CHUNK_BYTES.min(total - i * SIMD_CHUNK_BYTES);
            vec.extend_from_slice(unsafe {
                core::slice::from_raw_parts(arena.chunk_ptr(idx), take)
            });
        }
    }
}

impl std::fmt::Debug for ChunkedString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChunkedString")
            .field("index_offset", &self.index_offset)
            .field("chunks", &self.chunk_count())
            .field("byte_len", &self.byte_len)
            .field("filename_offset", &self.filename_offset)
            .finish()
    }
}

#[inline]
const fn chunks_needed(byte_len: usize) -> usize {
    if byte_len == 0 {
        0
    } else {
        byte_len.div_ceil(SIMD_CHUNK_BYTES)
    }
}

#[derive(Clone, Debug)]
pub struct ChunkedPathStore {
    arena: Vec<SimdChunk>,
    indices: Vec<u32>,
}

// SAFETY: arena is immutable after construction. Pointers derived from it are
// only read during scoring (no mutation, no reallocation).
unsafe impl Send for ChunkedPathStore {}
unsafe impl Sync for ChunkedPathStore {}

impl ChunkedPathStore {
    pub fn heap_bytes(&self) -> usize {
        self.arena.len() * SIMD_CHUNK_BYTES + self.indices.len() * std::mem::size_of::<u32>()
    }


    #[inline]
    pub fn as_arena_ptr(&self) -> ArenaPtr {
        ArenaPtr::new(self.arena.as_ptr() as *const u8, self.indices.as_ptr())
    }
}

/// At runtime the builder should be split out from the store after `finish()`.
#[derive(Clone, Debug)]
pub struct ChunkedPathStoreBuilder {
    arena: Vec<SimdChunk>,
    indices: Vec<u32>,
    chunk_dedup: AHashMap<[u8; SIMD_CHUNK_BYTES], u32>,
}

impl ChunkedPathStoreBuilder {
    pub fn new(estimated_files: usize) -> Self {
        // most paths fit into 64 bytes = 4 chunks; dedup keeps the arena well below that
        let est_indices = estimated_files * 4;
        Self {
            arena: Vec::with_capacity(est_indices / 2),
            indices: Vec::with_capacity(est_indices),
            chunk_dedup: AHashMap::with_capacity(est_indices / 2),
        }
    }

    pub fn finish(self) -> ChunkedPathStore {
        let Self {
            mut arena,
            mut indices,
            ..
        } = self;
        arena.shrink_to_fit();
        indices.shrink_to_fit();
        ChunkedPathStore { arena, indices }
    }

    pub fn as_arena_ptr(&self) -> ArenaPtr {
        ArenaPtr::new(self.arena.as_ptr() as *const u8, self.indices.as_ptr())
    }

    /// Like [`add_file_immediate`] but for directory paths where the entire
    /// string is the "directory" portion (filename_offset == byte_len).
    pub fn add_dir_immediate(&mut self, dir_rel_path: &str) -> ChunkedString {
        self.add_file_immediate(dir_rel_path, dir_rel_path.len() as u16)
    }

    pub fn add_file_immediate(&mut self, rel_path: &str, filename_offset: u16) -> ChunkedString {
        let path_bytes = rel_path.as_bytes();
        let index_offset = self.indices.len() as u32;

        for chunk in path_bytes.chunks(SIMD_CHUNK_BYTES) {
            let mut chunk_bytes = [0u8; SIMD_CHUNK_BYTES];
            chunk_bytes[..chunk.len()].copy_from_slice(chunk);

            let arena_idx = match self.chunk_dedup.get(&chunk_bytes) {
                Some(&idx) => idx,
                None => {
                    let idx = self.arena.len() as u32;
                    self.arena.push(SimdChunk(chunk_bytes));
                    self.chunk_dedup.insert(chunk_bytes, idx);
                    idx
                }
            };

            self.indices.push(arena_idx);
        }

        ChunkedString::new(index_offset, path_bytes.len() as u16, filename_offset)
    }
}


