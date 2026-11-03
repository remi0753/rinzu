//! Bounded, all-or-nothing value transport used by the `spawn` prototype.
//!
//! The model deliberately has separate input and result operations.  Each
//! operation performs an exact-size preflight, reserves all bytes before
//! encoding, and owns its reservation through a settlement guard.  Dropping
//! any guard follows the same release path, which makes failure and
//! cancellation accounting observable in tests.

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportFailure {
    ResourceLimit,
    CreditTimeout,
    Encode,
    Decode,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InjectFailure {
    None,
    Encode,
    Decode,
    Cancel,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BudgetSnapshot {
    pub capacity: usize,
    pub used: usize,
    pub peak: usize,
    pub reservations: usize,
    pub releases: usize,
    pub waits: usize,
}

#[derive(Debug)]
struct BudgetState {
    used: usize,
    peak: usize,
    reservations: usize,
    releases: usize,
    waits: usize,
}

#[derive(Debug)]
struct BudgetInner {
    capacity: usize,
    state: Mutex<BudgetState>,
    changed: Condvar,
}

#[derive(Clone, Debug)]
pub struct TransportBudget(Arc<BudgetInner>);

impl TransportBudget {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self(Arc::new(BudgetInner {
            capacity,
            state: Mutex::new(BudgetState {
                used: 0,
                peak: 0,
                reservations: 0,
                releases: 0,
                waits: 0,
            }),
            changed: Condvar::new(),
        }))
    }

    /// Reserve the complete encoded size, waiting only for existing credit.
    ///
    /// # Errors
    ///
    /// Returns `ResourceLimit` when `bytes` can never fit and `CreditTimeout`
    /// when existing reservations do not release before `timeout`.
    pub fn reserve(
        &self,
        bytes: usize,
        timeout: Duration,
    ) -> Result<(TransportReservation, Duration), TransportFailure> {
        if bytes > self.0.capacity {
            return Err(TransportFailure::ResourceLimit);
        }

        let wait_started = Instant::now();
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let deadline = wait_started + timeout;
        while state.used + bytes > self.0.capacity {
            state.waits += 1;
            let now = Instant::now();
            if now >= deadline {
                return Err(TransportFailure::CreditTimeout);
            }
            let remaining = deadline.saturating_duration_since(now);
            let (next, result) = self
                .0
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = next;
            if result.timed_out() && state.used + bytes > self.0.capacity {
                return Err(TransportFailure::CreditTimeout);
            }
        }

        state.used += bytes;
        state.peak = state.peak.max(state.used);
        state.reservations += 1;
        drop(state);
        Ok((
            TransportReservation {
                inner: Arc::clone(&self.0),
                bytes,
                released: false,
            },
            wait_started.elapsed(),
        ))
    }

    #[must_use]
    pub fn snapshot(&self) -> BudgetSnapshot {
        let state = self
            .0
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        BudgetSnapshot {
            capacity: self.0.capacity,
            used: state.used,
            peak: state.peak,
            reservations: state.reservations,
            releases: state.releases,
            waits: state.waits,
        }
    }
}

#[derive(Debug)]
pub struct TransportReservation {
    inner: Arc<BudgetInner>,
    bytes: usize,
    released: bool,
}

impl Drop for TransportReservation {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.used -= self.bytes;
        state.releases += 1;
        self.released = true;
        drop(state);
        self.inner.changed.notify_all();
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ResourceSnapshot {
    pub slots_acquired: usize,
    pub slots_released: usize,
    pub roots_acquired: usize,
    pub roots_released: usize,
    pub buffers_acquired: usize,
    pub buffers_released: usize,
}

#[derive(Debug, Default)]
struct ResourceState(ResourceSnapshot);

#[derive(Clone, Debug, Default)]
pub struct ResourceLedger(Arc<Mutex<ResourceState>>);

impl ResourceLedger {
    #[must_use]
    pub fn snapshot(&self) -> ResourceSnapshot {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .0
    }

    fn acquire(&self, kind: ResourceKind) -> ResourceGuard {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match kind {
            ResourceKind::Slot => state.0.slots_acquired += 1,
            ResourceKind::Root => state.0.roots_acquired += 1,
            ResourceKind::Buffer => state.0.buffers_acquired += 1,
        }
        ResourceGuard {
            ledger: self.clone(),
            kind,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ResourceKind {
    Slot,
    Root,
    Buffer,
}

#[derive(Debug)]
struct ResourceGuard {
    ledger: ResourceLedger,
    kind: ResourceKind,
}

impl Drop for ResourceGuard {
    fn drop(&mut self) {
        let mut state = self
            .ledger
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match self.kind {
            ResourceKind::Slot => state.0.slots_released += 1,
            ResourceKind::Root => state.0.roots_released += 1,
            ResourceKind::Buffer => state.0.buffers_released += 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TransportMetrics {
    pub exact_bytes: usize,
    pub copied_bytes: usize,
    pub credit_wait: Duration,
    pub pause: Duration,
}

/// Encode then decode one byte payload under one transport settlement.
///
/// # Errors
///
/// Returns the selected injected error, or a budget admission error.  All
/// resources acquired before the error are released by the same guards used
/// by the success path.
pub fn transport_bytes(
    value: &[u8],
    budget: &TransportBudget,
    ledger: &ResourceLedger,
    inject: InjectFailure,
    timeout: Duration,
) -> Result<(Vec<u8>, TransportMetrics), TransportFailure> {
    let pause_started = Instant::now();
    let _slot = ledger.acquire(ResourceKind::Slot);
    let _root = ledger.acquire(ResourceKind::Root);
    let exact_bytes = value.len();
    let (_reservation, credit_wait) = budget.reserve(exact_bytes, timeout)?;
    let _buffer = ledger.acquire(ResourceKind::Buffer);

    let mut encoded = Vec::with_capacity(exact_bytes);
    let split = exact_bytes / 2;
    encoded.extend_from_slice(&value[..split]);
    if inject == InjectFailure::Encode {
        return Err(TransportFailure::Encode);
    }
    encoded.extend_from_slice(&value[split..]);
    if inject == InjectFailure::Cancel {
        return Err(TransportFailure::Cancelled);
    }
    if inject == InjectFailure::Decode {
        return Err(TransportFailure::Decode);
    }

    let mut decoded = Vec::with_capacity(exact_bytes);
    decoded.extend_from_slice(&encoded);
    Ok((
        decoded,
        TransportMetrics {
            exact_bytes,
            copied_bytes: exact_bytes * 2,
            credit_wait,
            pause: pause_started.elapsed(),
        },
    ))
}

#[derive(Clone, Copy, Debug)]
pub struct SpawnTransportMetrics {
    pub input: TransportMetrics,
    pub result: TransportMetrics,
}

/// Model input and result as independent settlements.  The input reservation
/// is necessarily dropped before result admission, so a capacity-sized input
/// cannot deadlock a non-empty result.
///
/// # Errors
///
/// Returns the first input or result transport failure.
pub fn spawn_round_trip(
    input: &[u8],
    result: &[u8],
    budget: &TransportBudget,
    ledger: &ResourceLedger,
) -> Result<SpawnTransportMetrics, TransportFailure> {
    let (_, input_metrics) = transport_bytes(
        input,
        budget,
        ledger,
        InjectFailure::None,
        Duration::from_secs(5),
    )?;
    debug_assert_eq!(budget.snapshot().used, 0);
    let (_, result_metrics) = transport_bytes(
        result,
        budget,
        ledger,
        InjectFailure::None,
        Duration::from_secs(5),
    )?;
    Ok(SpawnTransportMetrics {
        input: input_metrics,
        result: result_metrics,
    })
}
