use parking_lot::Mutex;
use std::mem::MaybeUninit;

// this originally happen to be in TLS but there is a limit of TLS
// + the storage itself is not free, so now we rely on the fact that most calls
// are sequential in practice and allocate ONLY when we have a parallel access
static SORT_BUFFER: Mutex<Vec<u8>> = Mutex::new(Vec::new());

fn ensure_capacity(buf: &mut Vec<u8>, required: usize) {
    if buf.capacity() < required {
        let len = buf.len();
        buf.reserve(required - len);
    }
}

struct SharedSortBuf {
    guard: parking_lot::MutexGuard<'static, Vec<u8>>,
}

impl SharedSortBuf {
    fn as_slice_mut<T>(&mut self, len: usize) -> &mut [MaybeUninit<T>] {
        let align = std::mem::align_of::<MaybeUninit<T>>();
        let size = std::mem::size_of::<MaybeUninit<T>>();
        let required = len.saturating_mul(size).saturating_add(align);
        ensure_capacity(&mut self.guard, required);

        // SAFETY: the Vec<u8> is only 1-byte aligned, so we over-allocate by
        // `align` bytes and shift the pointer to satisfy T's alignment.
        // Callers never read uninitialised data through the returned slice.
        unsafe {
            let ptr = self.guard.as_mut_ptr();
            let offset = ptr.align_offset(align);
            debug_assert!(offset != usize::MAX && offset + len * size <= self.guard.capacity());
            std::slice::from_raw_parts_mut(ptr.add(offset) as *mut MaybeUninit<T>, len)
        }
    }
}

fn try_lock_shared_buf() -> Option<SharedSortBuf> {
    SORT_BUFFER.try_lock().map(|guard| SharedSortBuf { guard })
}

pub fn sort_with_buffer<T, F>(slice: &mut [T], compare: F)
where
    F: FnMut(&T, &T) -> std::cmp::Ordering,
{
    match try_lock_shared_buf() {
        Some(mut buf) => {
            let typed = buf.as_slice_mut::<T>(slice.len());
            glidesort::sort_with_buffer_by(slice, typed, compare);
        }
        None => glidesort::sort_by(slice, compare),
    }
}

