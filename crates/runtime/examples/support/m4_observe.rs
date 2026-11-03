use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Instant;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
struct Observed;
fn acquired(bytes: usize) {
    ALLOCATIONS.fetch_add(1, Relaxed);
    ALLOCATED.fetch_add(bytes, Relaxed);
    let live = LIVE.fetch_add(bytes, Relaxed) + bytes;
    PEAK.fetch_max(live, Relaxed);
}
// SAFETY: all operations delegate the exact layout/pointer contract to System;
// the observer uses only nonallocating atomics and never changes returned memory.
unsafe impl GlobalAlloc for Observed {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the allocator receives the caller's valid layout unchanged.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            acquired(layout.size());
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: preserve System's allocation/zeroing contract.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            acquired(layout.size());
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: the same global allocator owns this exact pointer/layout.
        unsafe { System.dealloc(pointer, layout) };
        LIVE.fetch_sub(layout.size(), Relaxed);
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: forward the exact live allocation and requested size.
        let result = unsafe { System.realloc(pointer, layout, size) };
        if !result.is_null() {
            LIVE.fetch_sub(layout.size(), Relaxed);
            acquired(size);
        }
        result
    }
}
#[global_allocator]
static ALLOCATOR: Observed = Observed;

pub fn measure(
    name: &str,
    size: usize,
    iterations: usize,
    mut action: impl FnMut(),
) -> serde_json::Value {
    for _ in 0..8 {
        action();
    }
    let before = (
        ALLOCATIONS.load(Relaxed),
        ALLOCATED.load(Relaxed),
        LIVE.load(Relaxed),
    );
    PEAK.store(before.2, Relaxed);
    let start = Instant::now();
    for _ in 0..iterations {
        action();
    }
    let seconds = start.elapsed().as_secs_f64();
    let amounts = (
        ALLOCATIONS.load(Relaxed) - before.0,
        ALLOCATED.load(Relaxed) - before.1,
        LIVE.load(Relaxed).saturating_sub(before.2),
        PEAK.load(Relaxed) - before.2,
    );
    serde_json::json!({"workload":name,"input_bytes":size,"iterations":iterations,"seconds":seconds,
        "rust_allocations":amounts.0,"rust_allocated_bytes":amounts.1,
        "rust_live_growth":amounts.2,"rust_peak_additional_bytes":amounts.3})
}
