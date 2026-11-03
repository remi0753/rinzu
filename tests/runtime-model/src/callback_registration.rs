//! Bounded callback registration prototype with exactly-once unregister.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const OPEN: u8 = 0;
const FAILED: u8 = 1;
const CLOSING: u8 = 2;
const CLOSED: u8 = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Delivery {
    Started,
    Queued,
    Overflow,
    Late,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistrationState {
    Open,
    Failed,
    Closing,
    Closed,
}

/// Native argument retained by a callback trampoline.
///
/// Dropping the value models the native-only release path.
#[derive(Debug)]
pub struct RetainedArgument {
    release_counter: Arc<AtomicUsize>,
}

impl RetainedArgument {
    #[must_use]
    pub fn new(release_counter: Arc<AtomicUsize>) -> Self {
        Self { release_counter }
    }
}

impl Drop for RetainedArgument {
    fn drop(&mut self) {
        self.release_counter.fetch_add(1, Ordering::AcqRel);
    }
}

#[derive(Debug, Default)]
struct HandlerQueue {
    active: Option<RetainedArgument>,
    pending: VecDeque<RetainedArgument>,
}

/// A `SerialBounded` registration from the normative callback contract.
#[derive(Debug)]
pub struct CallbackRegistration {
    state: AtomicU8,
    generation: AtomicU64,
    capacity: usize,
    native_unregister_count: AtomicUsize,
    queue: Mutex<HandlerQueue>,
}

// A poisoned prototype mutex is an unrecoverable test failure rather than a
// user-visible runtime condition.
#[allow(clippy::missing_panics_doc)]
impl CallbackRegistration {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            state: AtomicU8::new(OPEN),
            generation: AtomicU64::new(1),
            capacity,
            native_unregister_count: AtomicUsize::new(0),
            queue: Mutex::new(HandlerQueue::default()),
        }
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn deliver(&self, generation: u64, argument: RetainedArgument) -> Delivery {
        if generation != self.generation.load(Ordering::Acquire)
            || self.state.load(Ordering::Acquire) != OPEN
        {
            drop(argument);
            return Delivery::Late;
        }

        let mut queue = self.queue.lock().expect("callback queue poisoned");
        if generation != self.generation.load(Ordering::Acquire)
            || self.state.load(Ordering::Acquire) != OPEN
        {
            drop(argument);
            return Delivery::Late;
        }

        if queue.active.is_none() {
            queue.active = Some(argument);
            return Delivery::Started;
        }

        if queue.pending.len() < self.capacity {
            queue.pending.push_back(argument);
            return Delivery::Queued;
        }

        let overflow_winner = self
            .state
            .compare_exchange(OPEN, FAILED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        drop(argument);
        queue.active.take();
        queue.pending.clear();
        drop(queue);

        if overflow_winner {
            self.invalidate_generation();
            self.native_unregister_once();
            Delivery::Overflow
        } else {
            Delivery::Late
        }
    }

    /// Settle the active handler and start the next queued handler when open.
    pub fn complete_active(&self) {
        let mut queue = self.queue.lock().expect("callback queue poisoned");
        queue.active.take();
        if self.state.load(Ordering::Acquire) == OPEN {
            queue.active = queue.pending.pop_front();
        } else {
            queue.pending.clear();
        }
    }

    /// Close admission, unregister the native source, cancel the active
    /// handler, and release every queued argument.
    pub fn unregister(&self) {
        let previous = self.state.swap(CLOSING, Ordering::AcqRel);
        if previous == CLOSED {
            return;
        }

        self.invalidate_generation();
        self.native_unregister_once();
        let mut queue = self.queue.lock().expect("callback queue poisoned");
        queue.active.take();
        queue.pending.clear();
        self.state.store(CLOSED, Ordering::Release);
    }

    #[must_use]
    pub fn state(&self) -> RegistrationState {
        match self.state.load(Ordering::Acquire) {
            OPEN => RegistrationState::Open,
            FAILED => RegistrationState::Failed,
            CLOSING => RegistrationState::Closing,
            CLOSED => RegistrationState::Closed,
            value => panic!("invalid registration state {value}"),
        }
    }

    #[must_use]
    pub fn native_unregister_count(&self) -> usize {
        self.native_unregister_count.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn outstanding_handlers(&self) -> usize {
        let queue = self.queue.lock().expect("callback queue poisoned");
        usize::from(queue.active.is_some()) + queue.pending.len()
    }

    fn invalidate_generation(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    fn native_unregister_once(&self) {
        let _ = self.native_unregister_count.compare_exchange(
            0,
            1,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}
