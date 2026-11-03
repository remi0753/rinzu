use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use rinzu_runtime::transport::{
    TransferDescriptor, TransferKind, TransferValue, TransportBudget, TransportDirection, encode,
    preflight,
};

struct TrackingAllocator;

static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
static PEAK_ALLOCATED: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static GLOBAL_ALLOCATOR: TrackingAllocator = TrackingAllocator;

// SAFETY: this wrapper delegates allocation to System and only updates
// process-local counters after successful operations.
unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller provides a valid allocation layout.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record_allocation(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller provides a valid allocation layout.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            record_allocation(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        ALLOCATED.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: pointer/layout are the live pair supplied by the allocator caller.
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: pointer/layout identify a live allocation and new_size is the
        // replacement size requested by the caller.
        let replacement = unsafe { System.realloc(pointer, layout, new_size) };
        if !replacement.is_null() {
            if new_size >= layout.size() {
                record_allocation(new_size - layout.size());
            } else {
                ALLOCATED.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        replacement
    }
}

fn record_allocation(bytes: usize) {
    let current = ALLOCATED.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK_ALLOCATED.fetch_max(current, Ordering::Relaxed);
}

fn main() {
    println!(
        "payload_bytes,encoded_bytes,copy_bytes,credit_wait_ns,wall_time_ns,os_peak_rss_bytes,allocation_peak_bytes"
    );
    for payload_bytes in [1024_usize, 1024 * 1024, 100 * 1024 * 1024] {
        run_case(payload_bytes);
    }
}

fn run_case(payload_bytes: usize) {
    let descriptor = TransferDescriptor {
        kind: TransferKind::ImmutableBytes,
        source: None,
    };
    let payload = TransferValue::ImmutableBytes(Arc::from(vec![0x5a; payload_bytes]));
    let exact = preflight(&descriptor, &payload).expect("benchmark preflight");
    let budget = TransportBudget::new(exact, exact);
    let allocation_baseline = ALLOCATED.load(Ordering::Relaxed);
    PEAK_ALLOCATED.store(allocation_baseline, Ordering::Relaxed);
    let wall_start = Instant::now();
    let credit_start = Instant::now();
    let reservation = budget
        .reserve(TransportDirection::Input, exact)
        .expect("benchmark credit reservation");
    let credit_wait = credit_start.elapsed();
    let buffer = encode(&descriptor, &payload, reservation).expect("benchmark encode");
    let encoded_bytes = buffer.bytes().len();
    let decoded = buffer
        .materialize(&descriptor)
        .expect("benchmark materialize");
    let wall_time = wall_start.elapsed();
    let TransferValue::ImmutableBytes(decoded) = decoded else {
        panic!("benchmark decoded the wrong transfer type");
    };
    assert_eq!(decoded.len(), payload_bytes);
    assert_eq!(decoded.first(), Some(&0x5a));
    assert_eq!(budget.snapshot().used_bytes, 0);
    let allocation_peak = PEAK_ALLOCATED
        .load(Ordering::Relaxed)
        .saturating_sub(allocation_baseline);
    let copy_bytes = encoded_bytes + payload_bytes;
    println!(
        "{payload_bytes},{encoded_bytes},{copy_bytes},{},{},{},{allocation_peak}",
        credit_wait.as_nanos(),
        wall_time.as_nanos(),
        process_peak_rss_bytes(),
    );
}

#[cfg(unix)]
fn process_peak_rss_bytes() -> u64 {
    // SAFETY: getrusage initializes the provided structure for the current process.
    unsafe {
        let mut usage = std::mem::zeroed::<libc::rusage>();
        if libc::getrusage(libc::RUSAGE_SELF, &mut usage) != 0 {
            return 0;
        }
        let resident = u64::try_from(usage.ru_maxrss).unwrap_or(0);
        if cfg!(target_os = "macos") {
            resident
        } else {
            resident.saturating_mul(1024)
        }
    }
}

#[cfg(not(unix))]
const fn process_peak_rss_bytes() -> u64 {
    0
}
