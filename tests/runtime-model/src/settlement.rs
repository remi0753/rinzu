//! Common single-winner async settlement and reserved shutdown control item.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

const PENDING: u8 = 0;
const COMPLETING: u8 = 1;
const CANCELLING: u8 = 2;
const SETTLED: u8 = 3;
const NO_OUTCOME: u8 = 0;
const COMPLETED: u8 = 1;
const CANCELLED: u8 = 2;
const TIMED_OUT: u8 = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationKind {
    Io,
    Timer,
    Blocking,
    Spawn,
    CallbackClose,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Completed,
    Cancelled,
    TimedOut,
}

impl Outcome {
    const fn encoded(self) -> u8 {
        match self {
            Self::Completed => COMPLETED,
            Self::Cancelled => CANCELLED,
            Self::TimedOut => TIMED_OUT,
        }
    }

    const fn transition(self) -> u8 {
        match self {
            Self::Completed => COMPLETING,
            Self::Cancelled | Self::TimedOut => CANCELLING,
        }
    }
}

#[derive(Debug)]
struct ResolverInner {
    state: AtomicU8,
    outcome: AtomicU8,
    future_settlements: AtomicUsize,
    resource_releases: AtomicUsize,
    late_result_releases: AtomicUsize,
    resource_count: usize,
}

#[derive(Clone, Debug)]
pub struct Resolver {
    pub kind: OperationKind,
    inner: Arc<ResolverInner>,
}

impl Resolver {
    #[must_use]
    pub fn new(kind: OperationKind, resource_count: usize) -> Self {
        Self {
            kind,
            inner: Arc::new(ResolverInner {
                state: AtomicU8::new(PENDING),
                outcome: AtomicU8::new(NO_OUTCOME),
                future_settlements: AtomicUsize::new(0),
                resource_releases: AtomicUsize::new(0),
                late_result_releases: AtomicUsize::new(0),
                resource_count,
            }),
        }
    }

    /// Race one terminal signal. The `Pending` CAS is the linearization point.
    #[must_use]
    pub fn try_settle(&self, outcome: Outcome) -> bool {
        if self
            .inner
            .state
            .compare_exchange(
                PENDING,
                outcome.transition(),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            self.inner
                .outcome
                .store(outcome.encoded(), Ordering::Release);
            self.inner
                .future_settlements
                .fetch_add(1, Ordering::Relaxed);
            self.inner
                .resource_releases
                .fetch_add(self.inner.resource_count, Ordering::Relaxed);
            self.inner.state.store(SETTLED, Ordering::Release);
            true
        } else {
            // A loser may own a native late result, but owns none of the
            // Future/resource ledger controlled by the resolver.
            self.inner
                .late_result_releases
                .fetch_add(1, Ordering::Relaxed);
            false
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> ResolverSnapshot {
        let outcome = match self.inner.outcome.load(Ordering::Acquire) {
            COMPLETED => Some(Outcome::Completed),
            CANCELLED => Some(Outcome::Cancelled),
            TIMED_OUT => Some(Outcome::TimedOut),
            _ => None,
        };
        ResolverSnapshot {
            settled: self.inner.state.load(Ordering::Acquire) == SETTLED,
            outcome,
            future_settlements: self.inner.future_settlements.load(Ordering::Relaxed),
            resource_releases: self.inner.resource_releases.load(Ordering::Relaxed),
            late_result_releases: self.inner.late_result_releases.load(Ordering::Relaxed),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolverSnapshot {
    pub settled: bool,
    pub outcome: Option<Outcome>,
    pub future_settlements: usize,
    pub resource_releases: usize,
    pub late_result_releases: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SaturatedChild {
    Admitted,
    Released,
}

/// Runtime limits plus one separately reserved control item.  Every vector is
/// allocated before shutdown begins; control turns mutate it in place.
#[derive(Debug)]
pub struct SaturatedRuntime {
    children: Vec<SaturatedChild>,
    cursor: usize,
    batch_limit: usize,
    control_pending: bool,
    control_turns: usize,
    max_control_items: usize,
    allocations_during_shutdown: usize,
    released_operation_slots: usize,
}

impl SaturatedRuntime {
    /// Pre-admit one task, operation, registration and queue slot per item.
    ///
    /// # Panics
    ///
    /// Panics when `batch_limit` is zero.
    #[must_use]
    pub fn new(limit: usize, batch_limit: usize) -> Self {
        assert!(batch_limit > 0);
        Self {
            children: vec![SaturatedChild::Admitted; limit],
            cursor: 0,
            batch_limit,
            control_pending: false,
            control_turns: 0,
            max_control_items: 0,
            allocations_during_shutdown: 0,
            released_operation_slots: 0,
        }
    }

    pub fn begin_shutdown(&mut self) {
        self.control_pending = true;
        self.max_control_items = self.max_control_items.max(1);
    }

    /// Run one bounded batch using the same reserved control item and cursor.
    pub fn run_control_turn(&mut self) {
        if !self.control_pending {
            return;
        }
        self.control_turns += 1;
        let end = (self.cursor + self.batch_limit).min(self.children.len());
        for child in &mut self.children[self.cursor..end] {
            if *child == SaturatedChild::Admitted {
                *child = SaturatedChild::Released;
                self.released_operation_slots += 1;
            }
        }
        self.cursor = end;
        self.control_pending = self.cursor < self.children.len();
    }

    #[must_use]
    pub fn is_shutdown_complete(&self) -> bool {
        !self.control_pending
            && self.cursor == self.children.len()
            && self
                .children
                .iter()
                .all(|child| *child == SaturatedChild::Released)
    }

    #[must_use]
    pub const fn control_turns(&self) -> usize {
        self.control_turns
    }

    #[must_use]
    pub const fn max_control_items(&self) -> usize {
        self.max_control_items
    }

    #[must_use]
    pub const fn allocations_during_shutdown(&self) -> usize {
        self.allocations_during_shutdown
    }

    #[must_use]
    pub const fn released_operation_slots(&self) -> usize {
        self.released_operation_slots
    }
}
