//! Real-thread stress harness for the Host Loop lifecycle protocol.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;

const PHASE_ACTIVE: u64 = 1;
const PHASE_LEAVING: u64 = 2;
const PHASE_CLOSING: u64 = 3;
const PHASE_MASK: u64 = 0xff;
const GENERATION: u64 = 1;
const GENERATION_SHIFT: u32 = 8;
const COUNT_SHIFT: u32 = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StressPostResult {
    Accepted,
    Closed,
    Failure,
}

#[derive(Clone, Copy, Debug)]
pub struct StressConfig {
    pub iterations: usize,
    pub producers: usize,
    pub items_per_producer: usize,
}

impl Default for StressConfig {
    fn default() -> Self {
        Self {
            iterations: 200,
            producers: 4,
            items_per_producer: 32,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StressReport {
    pub iterations: usize,
    pub enqueued: usize,
    pub processed: usize,
    pub late_callback_noops: usize,
    pub fail_fast_wins: usize,
}

struct Controller {
    host_state: AtomicU64,
    work_pending: AtomicBool,
    owner_post_outstanding: AtomicBool,
    callbacks_pending: AtomicUsize,
    post_calls_in_flight: AtomicUsize,
    fail_fast_count: AtomicUsize,
    deactivated: AtomicBool,
    queue: Mutex<VecDeque<usize>>,
    processed: Vec<AtomicUsize>,
    duplicate_execution: AtomicBool,
}

impl Controller {
    fn new(item_count: usize) -> Self {
        Self {
            host_state: AtomicU64::new(pack(PHASE_ACTIVE, 0)),
            work_pending: AtomicBool::new(false),
            owner_post_outstanding: AtomicBool::new(false),
            callbacks_pending: AtomicUsize::new(0),
            post_calls_in_flight: AtomicUsize::new(0),
            fail_fast_count: AtomicUsize::new(0),
            deactivated: AtomicBool::new(false),
            queue: Mutex::new(VecDeque::new()),
            processed: (0..item_count).map(|_| AtomicUsize::new(0)).collect(),
            duplicate_execution: AtomicBool::new(false),
        }
    }

    fn enqueue(&self, item: usize, result: StressPostResult) {
        self.queue.lock().expect("queue poisoned").push_back(item);
        self.work_pending.store(true, Ordering::Release);
        self.try_post(result);
    }

    fn try_post(&self, result: StressPostResult) {
        if !self.acquire_lease() {
            return;
        }

        if phase(self.host_state.load(Ordering::Acquire)) != PHASE_ACTIVE {
            self.release_lease();
            return;
        }

        if self
            .owner_post_outstanding
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            self.release_lease();
            return;
        }

        self.post_calls_in_flight.fetch_add(1, Ordering::AcqRel);
        thread::yield_now();

        match result {
            StressPostResult::Accepted => {
                self.callbacks_pending.fetch_add(1, Ordering::Release);
            }
            StressPostResult::Closed => {
                self.owner_post_outstanding.store(false, Ordering::Release);
                self.transition_from_active(PHASE_LEAVING);
            }
            StressPostResult::Failure => {
                self.owner_post_outstanding.store(false, Ordering::Release);
                if self.transition_from_active(PHASE_CLOSING) {
                    self.fail_fast_count.fetch_add(1, Ordering::AcqRel);
                }
            }
        }

        self.post_calls_in_flight.fetch_sub(1, Ordering::AcqRel);
        self.release_lease();
    }

    fn acquire_lease(&self) -> bool {
        let mut current = self.host_state.load(Ordering::Acquire);
        loop {
            if phase(current) != PHASE_ACTIVE {
                return false;
            }
            let count = lease_count(current);
            let Some(next_count) = count.checked_add(1) else {
                return false;
            };
            let next = pack(PHASE_ACTIVE, next_count);
            match self.host_state.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(observed) => current = observed,
            }
        }
    }

    fn release_lease(&self) {
        let mut current = self.host_state.load(Ordering::Acquire);
        loop {
            let count = lease_count(current);
            assert!(count > 0, "PostLease counter underflow");
            let next = pack(phase(current), count - 1);
            match self.host_state.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return,
                Err(observed) => current = observed,
            }
        }
    }

    fn transition_from_active(&self, target_phase: u64) -> bool {
        let mut current = self.host_state.load(Ordering::Acquire);
        loop {
            if phase(current) != PHASE_ACTIVE {
                return false;
            }
            let next = pack(target_phase, lease_count(current));
            match self.host_state.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(observed) => current = observed,
            }
        }
    }

    fn deliver_one_callback(&self, result: StressPostResult) -> bool {
        if self
            .callbacks_pending
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_sub(1)
            })
            .is_err()
        {
            return false;
        }

        if phase(self.host_state.load(Ordering::Acquire)) != PHASE_ACTIVE {
            return true;
        }

        self.drain_managed_queue();
        self.owner_post_outstanding.store(false, Ordering::Release);
        self.work_pending.store(false, Ordering::Release);

        let queue_is_empty = self.queue.lock().expect("queue poisoned").is_empty();
        if !queue_is_empty {
            self.work_pending.store(true, Ordering::Release);
            self.try_post(result);
        }
        true
    }

    fn drain_managed_queue(&self) {
        let items: Vec<_> = self
            .queue
            .lock()
            .expect("queue poisoned")
            .drain(..)
            .collect();
        for item in items {
            if self.processed[item].fetch_add(1, Ordering::AcqRel) != 0 {
                self.duplicate_execution.store(true, Ordering::Release);
            }
        }
    }

    fn continue_leave(&self) {
        self.transition_from_active(PHASE_LEAVING);
        while lease_count(self.host_state.load(Ordering::Acquire)) != 0 {
            thread::yield_now();
        }
        assert_eq!(
            self.post_calls_in_flight.load(Ordering::Acquire),
            0,
            "deactivate overlapped postOwnerTurn"
        );
        if phase(self.host_state.load(Ordering::Acquire)) == PHASE_LEAVING {
            self.deactivated.store(true, Ordering::Release);
            self.owner_post_outstanding.store(false, Ordering::Release);
        }
    }

    fn runtime_pump(&self) {
        self.drain_managed_queue();
        self.work_pending.store(false, Ordering::Release);
    }
}

/// Run a real-thread stress suite for Accepted, Closed, and Failure paths.
///
/// # Panics
///
/// Panics on a lost item, duplicate execution, lifecycle split-brain, lease
/// leak, or overlap between `postOwnerTurn` and `deactivate`.
#[must_use]
pub fn run_host_loop_stress(config: StressConfig) -> StressReport {
    let mut report = StressReport::default();
    let modes = [
        StressPostResult::Accepted,
        StressPostResult::Closed,
        StressPostResult::Failure,
    ];

    for iteration in 0..config.iterations {
        let result = modes[iteration % modes.len()];
        let item_count = config.producers * config.items_per_producer;
        let controller = Arc::new(Controller::new(item_count));
        let barrier = Arc::new(Barrier::new(config.producers + 1));
        let completed = Arc::new(AtomicUsize::new(0));
        let mut workers = Vec::with_capacity(config.producers);

        for producer in 0..config.producers {
            let controller = Arc::clone(&controller);
            let barrier = Arc::clone(&barrier);
            let completed = Arc::clone(&completed);
            workers.push(thread::spawn(move || {
                barrier.wait();
                for offset in 0..config.items_per_producer {
                    controller.enqueue(producer * config.items_per_producer + offset, result);
                    if (producer + offset + iteration) % 3 == 0 {
                        thread::yield_now();
                    }
                }
                completed.fetch_add(1, Ordering::Release);
            }));
        }

        barrier.wait();
        let early_leave = iteration % 2 == 0;
        let mut spins = 0_usize;

        while completed.load(Ordering::Acquire) < config.producers {
            if result == StressPostResult::Accepted {
                controller.deliver_one_callback(result);
            }
            if early_leave && iteration_enough_work(&controller, item_count) {
                controller.continue_leave();
                break;
            }
            spins += 1;
            assert!(spins < 1_000_000, "producer progress timed out");
            thread::yield_now();
        }

        for worker in workers {
            worker.join().expect("producer panicked");
        }

        if phase(controller.host_state.load(Ordering::Acquire)) == PHASE_CLOSING {
            assert_eq!(controller.fail_fast_count.load(Ordering::Acquire), 1);
            assert!(!controller.deactivated.load(Ordering::Acquire));
            report.fail_fast_wins += 1;
        } else {
            if result == StressPostResult::Accepted && !early_leave {
                let mut callback_spins = 0_usize;
                while !controller.queue.lock().expect("queue poisoned").is_empty() {
                    controller.deliver_one_callback(result);
                    callback_spins += 1;
                    assert!(callback_spins < 1_000_000, "lost owner wake");
                    thread::yield_now();
                }
            }

            controller.continue_leave();
            while controller.deliver_one_callback(result) {}
            controller.runtime_pump();

            assert!(controller.deactivated.load(Ordering::Acquire));
            assert_eq!(controller.fail_fast_count.load(Ordering::Acquire), 0);
            assert_eq!(
                lease_count(controller.host_state.load(Ordering::Acquire)),
                0
            );
            assert_eq!(controller.post_calls_in_flight.load(Ordering::Acquire), 0);
            assert!(!controller.duplicate_execution.load(Ordering::Acquire));

            for execution_count in &controller.processed {
                assert_eq!(execution_count.load(Ordering::Acquire), 1, "lost work item");
            }
            report.processed += item_count;
        }

        report.late_callback_noops += controller.callbacks_pending.load(Ordering::Acquire);
        report.iterations += 1;
        report.enqueued += item_count;
    }

    report
}

fn iteration_enough_work(controller: &Controller, item_count: usize) -> bool {
    controller.queue.lock().expect("queue poisoned").len() >= item_count / 4
}

fn pack(phase: u64, in_flight: u32) -> u64 {
    phase | (GENERATION << GENERATION_SHIFT) | (u64::from(in_flight) << COUNT_SHIFT)
}

const fn phase(word: u64) -> u64 {
    word & PHASE_MASK
}

const fn lease_count(word: u64) -> u32 {
    (word >> COUNT_SHIFT) as u32
}
