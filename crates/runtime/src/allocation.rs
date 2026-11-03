//! Process-fatal allocation policy for standalone Rinzu executables.
use std::alloc::{GlobalAlloc, Layout};

/// Preserve the underlying allocator's successful contracts, but never return
/// an allocation failure to Rust's diagnostic/unwind or managed Error paths.
pub struct FatalAllocator<A>(pub A);

#[cold]
pub fn fatal() -> ! {
    // SAFETY: immediate process exit needs no allocation, output, unwinding,
    // managed cleanup, other Worker or external event-loop cooperation.
    crate::native_fatal::resource()
}

/// Only an explicitly reported memory failure is fatal; preserve normal I/O,
/// missing-file and OS admission errors without guessing from NULL or failure.
pub fn checked_io<T>(result: std::io::Result<T>) -> std::io::Result<T> {
    if result
        .as_ref()
        .is_err_and(|error| error.kind() == std::io::ErrorKind::OutOfMemory)
    {
        fatal();
    }
    result
}

fn required(pointer: *mut u8) -> *mut u8 {
    if pointer.is_null() {
        fatal()
    }
    pointer
}

// SAFETY: layouts/pointers and successful ownership are delegated unchanged to
// A. The only changed behavior is immediate process termination on a NULL result.
unsafe impl<A: GlobalAlloc> GlobalAlloc for FatalAllocator<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: GlobalAlloc supplies the valid nonzero allocation layout.
        required(unsafe { self.0.alloc(layout) })
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: preserve A's initialization and alignment contract.
        required(unsafe { self.0.alloc_zeroed(layout) })
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: the caller supplies A's live pointer/layout and valid new size.
        required(unsafe { self.0.realloc(pointer, layout, size) })
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: the original allocator receives its exact live pointer/layout.
        unsafe { self.0.dealloc(pointer, layout) }
    }
}
