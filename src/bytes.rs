//! Implements an arena allocator for arbitrary bytes.
use std::{ptr, slice};
use std::cell::{Cell, RefCell};

struct ByteArenaChunk(Vec<u8>);
impl ByteArenaChunk {
    #[inline]
    fn with_capacity(capacity: usize) -> Self {
        ByteArenaChunk(Vec::with_capacity(capacity))
    }
    #[inline]
    fn start(&self) -> *mut u8 {
        self.0.as_ptr() as *mut u8
    }
    #[inline]
    fn end(&self) -> *mut u8 {
        unsafe {
            let capacity = self.0.capacity();
            self.0.as_ptr().offset(capacity as isize) as *mut u8
        }
    }
}

/// Arena allocator for bytes.
///
/// This allows the user to request arena allocation
/// of an arbitrary number of bytes.
/// It should usually be much faster than `typed_arena::Arena`,
/// since the implementation is highly optimized
/// and only requires a couple instructions in the common case.
pub struct ByteArena {
    current: Cell<*mut u8>,
    end: Cell<*mut u8>,
    chunks: RefCell<Vec<ByteArenaChunk>>,
}

impl ByteArena {
    pub fn new() -> Self {
        ByteArena {
            current:  Cell::new(ptr::null_mut()),
            end: Cell::new(ptr::null_mut()),
            chunks: RefCell::new(Vec::new())
        }
    }
    #[inline]
    pub fn with_capacity(capacity: usize) -> Self {
        let chunk = ByteArenaChunk::with_capacity(capacity);
        let start = chunk.start();
        let end = chunk.end();
        ByteArena {
            current: Cell::new(start),
            end: Cell::new(end),
            chunks: RefCell::new(vec![chunk]),
        }
    }
    #[inline]
    fn remaining(&self) -> usize {
        self.end.get() as usize - self.current.get() as usize
    }
    #[inline]
    pub unsafe fn alloc_uninitialized(&self, amount: usize) -> *mut u8 {
        if self.remaining() < amount {
            self.reserve(amount)
        }
        debug_assert!(self.remaining() >= amount);
        let ptr = self.current.get();
        self.current.set(ptr.offset(amount as isize));
        ptr
    }
    #[inline]
    pub fn alloc_copied<'a>(&'a self, source: &[u8]) -> &'a mut [u8] {
        let amount = source.len();
        unsafe {
            let ptr = self.alloc_uninitialized(amount);
            ptr::copy_nonoverlapping(source.as_ptr(), ptr, amount);
            slice::from_raw_parts_mut(ptr, amount)
        }
    }
    #[inline]
    pub fn alloc_zeroed(&self, amount: usize) -> &mut [u8] {
        unsafe {
            let ptr = self.alloc_uninitialized(amount);
            ptr::write_bytes(ptr, 0, amount);
            slice::from_raw_parts_mut(ptr, amount)
        }
    }
    #[cold]
    #[inline(never)]
    fn reserve(&self, amount: usize) {
        assert!(self.remaining() < amount);
        let capacity = amount.checked_next_power_of_two().expect("Capacity overflow").max(4096);
        assert!(capacity >= amount);
        let chunk = ByteArenaChunk::with_capacity(capacity);
        self.current.set(chunk.start());
        self.end.set(chunk.end());
        self.chunks.borrow_mut().push(chunk);
    }
}
unsafe impl Send for ByteArena {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_send() {
        let arena = ByteArena::new();
        arena.alloc_zeroed(1000);
        ::std::thread::spawn(move || {
            arena.alloc_zeroed(1000);
        });
    }
}
