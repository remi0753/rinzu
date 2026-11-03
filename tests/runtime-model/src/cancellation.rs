//! Cancellation polling and structured child lifetime prototype.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Backend {
    BaselineJit,
    Aot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Workload {
    InfiniteBackedge,
    DeepRecursion,
    LongArithmetic,
}

#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CancellationReport {
    pub iterations: u64,
    pub polls: u64,
    pub finally_runs: usize,
    pub cancelled: bool,
}

struct FinallyCounter<'a>(&'a AtomicUsize);

impl Drop for FinallyCounter<'_> {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn recursion_call_edges(
    depth: u16,
    token: &CancellationToken,
    report: &mut CancellationReport,
) -> bool {
    report.polls += 1;
    if token.is_cancelled() {
        return false;
    }
    report.iterations += 1;
    if depth == 0 {
        true
    } else {
        recursion_call_edges(depth - 1, token, report)
    }
}

/// Run managed CPU work until a compiler-inserted poll observes cancellation.
///
/// Both backends deliberately use the same poll sites: backedges, non-leaf
/// call edges, and the arithmetic loop backedge.  No workload contains an
/// `await`; the backedge workloads allocate nothing after entering the loop.
#[must_use]
pub fn run_until_cancelled(
    backend: Backend,
    workload: Workload,
    token: &CancellationToken,
    started: &mpsc::Sender<()>,
) -> CancellationReport {
    let finally_runs = AtomicUsize::new(0);
    let mut report = CancellationReport::default();
    let finally_guard = FinallyCounter(&finally_runs);
    let _ = started.send(());

    // The match keeps backend selection explicit even though the semantic
    // instruction stream is intentionally identical.
    let poll_stride = match backend {
        Backend::BaselineJit | Backend::Aot => 1_024_u64,
    };

    match workload {
        Workload::InfiniteBackedge => loop {
            report.iterations = report.iterations.wrapping_add(1);
            report.polls += 1;
            if token.is_cancelled() {
                report.cancelled = true;
                break;
            }
        },
        Workload::DeepRecursion => loop {
            if !recursion_call_edges(256, token, &mut report) {
                report.cancelled = true;
                break;
            }
        },
        Workload::LongArithmetic => {
            let mut accumulator = 0_u64;
            loop {
                for offset in 0..poll_stride {
                    accumulator = accumulator.wrapping_mul(33).wrapping_add(offset);
                    report.iterations += 1;
                }
                report.polls += 1;
                if token.is_cancelled() {
                    report.cancelled = true;
                    std::hint::black_box(accumulator);
                    break;
                }
            }
        }
    }
    drop(finally_guard);
    report.finally_runs = finally_runs.load(Ordering::Relaxed);
    report
}

#[derive(Clone, Debug, Default)]
pub struct TaskRegistry {
    active: Arc<AtomicUsize>,
}

impl TaskRegistry {
    #[must_use]
    pub fn active(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }
}

struct ActiveTask(Arc<AtomicUsize>);

impl Drop for ActiveTask {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Debug)]
pub struct ChildFuture {
    _child_id: usize,
}

impl Drop for ChildFuture {
    fn drop(&mut self) {
        // Deliberately does not detach or cancel: ownership remains in scope.
    }
}

#[derive(Debug)]
pub struct StructuredScope {
    token: CancellationToken,
    registry: TaskRegistry,
    children: Vec<JoinHandle<()>>,
    next_id: usize,
    closed: bool,
}

impl StructuredScope {
    #[must_use]
    pub fn new(registry: TaskRegistry) -> Self {
        Self {
            token: CancellationToken::default(),
            registry,
            children: Vec::new(),
            next_id: 0,
            closed: false,
        }
    }

    /// Spawn a child owned by this scope. Dropping the returned Future does
    /// not detach the thread; the scope retains its join handle.
    ///
    /// # Panics
    ///
    /// Panics when called after the scope has closed.
    pub fn spawn(&mut self) -> ChildFuture {
        assert!(!self.closed, "cannot spawn into a closed scope");
        let child_id = self.next_id;
        self.next_id += 1;
        let token = self.token.clone();
        let active = Arc::clone(&self.registry.active);
        active.fetch_add(1, Ordering::AcqRel);
        self.children.push(thread::spawn(move || {
            let _active = ActiveTask(active);
            let mut value = 0_u64;
            while !token.is_cancelled() {
                value = value.wrapping_add(1);
                std::hint::black_box(value);
                thread::yield_now();
            }
        }));
        ChildFuture {
            _child_id: child_id,
        }
    }

    /// Cancel and join every structured child.
    ///
    /// # Panics
    ///
    /// Panics when a child thread has itself panicked.
    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.token.cancel();
        for child in self.children.drain(..) {
            child.join().expect("structured child panicked");
        }
    }
}

impl Drop for StructuredScope {
    fn drop(&mut self) {
        self.close();
    }
}
