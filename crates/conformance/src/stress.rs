//! Seed-replayable real-thread Host Loop stress.

use core::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, bail, ensure};
use rinzu_runtime::host_loop::{
    HostLoopAdapter, HostLoopController, HostPostResult, OwnerTurnTarget,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StressConfig {
    pub seed: u64,
    pub rounds: usize,
    pub producers: usize,
    pub items_per_producer: usize,
}

impl Default for StressConfig {
    fn default() -> Self {
        Self {
            seed: 0x7269_6e7a_755f_7631,
            rounds: 100,
            producers: 4,
            items_per_producer: 64,
        }
    }
}

#[derive(Clone, Copy)]
struct PostedTurn {
    owner: OwnerTurnTarget,
    generation: u64,
}

// SAFETY: OwnerTurnTarget follows the Host Loop process-lifetime callback
// contract; the receiver invokes it only on the controller owner thread.
unsafe impl Send for PostedTurn {}

struct QueueAdapter {
    sender: SyncSender<PostedTurn>,
    prepared: Mutex<Option<PostedTurn>>,
    deactivations: AtomicUsize,
}

impl HostLoopAdapter for QueueAdapter {
    fn prepare(&self, owner: OwnerTurnTarget, generation: u64) -> Result<usize, String> {
        *self
            .prepared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(PostedTurn { owner, generation });
        Ok(1)
    }

    fn post_owner_turn(&self, _ready_token: usize, _generation: u64) -> HostPostResult {
        let Some(turn) = *self
            .prepared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        else {
            return HostPostResult::Closed;
        };
        match self.sender.try_send(turn) {
            Ok(()) => HostPostResult::Accepted,
            Err(TrySendError::Full(_)) => HostPostResult::Failure,
            Err(TrySendError::Disconnected(_)) => HostPostResult::Closed,
        }
    }

    fn deactivate(&self, _ready_token: usize, _generation: u64) {
        self.deactivations.fetch_add(1, Ordering::AcqRel);
    }
}

impl QueueAdapter {
    fn prepared_turn(&self) -> PostedTurn {
        self.prepared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .expect("adapter was prepared before stress delivery")
    }
}

pub fn run_host_stress(config: StressConfig) -> Result<()> {
    ensure!(config.rounds > 0, "stress rounds must be non-zero");
    ensure!(config.producers > 0, "stress producers must be non-zero");
    ensure!(
        config.items_per_producer > 0,
        "stress items per producer must be non-zero"
    );
    let mut random = XorShift64::new(config.seed);
    for round in 0..config.rounds {
        let round_seed = random.next();
        if let Err(error) = run_round(config, round_seed) {
            bail!(
                "Host stress failed at round {round} with root seed {} and round seed {round_seed}: {error:#}; replay with `cargo run -p rinzu-conformance -- host-stress --seed {} --rounds {}`",
                config.seed,
                config.seed,
                round + 1
            );
        }
    }
    Ok(())
}

fn run_round(config: StressConfig, seed: u64) -> Result<()> {
    let (sender, receiver) = sync_channel(1);
    let adapter = Arc::new(QueueAdapter {
        sender,
        prepared: Mutex::new(None),
        deactivations: AtomicUsize::new(0),
    });
    let runtime_adapter: Arc<dyn HostLoopAdapter> = adapter.clone();
    let capacity = config.producers * config.items_per_producer;
    let controller = HostLoopController::new(runtime_adapter, capacity, 7);
    let activation = controller.enter()?;
    let expected = capacity;
    let completed = Arc::new(AtomicUsize::new(0));
    let producers_done = Arc::new(AtomicUsize::new(0));
    let mut random = XorShift64::new(seed);
    let schedules = (0..config.producers)
        .map(|_| {
            (0..config.items_per_producer)
                .map(|_| (random.next() & 7) as usize)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let mut producers = Vec::new();
    for schedule in schedules {
        let controller = Arc::clone(&controller);
        let completed = Arc::clone(&completed);
        let producers_done = Arc::clone(&producers_done);
        producers.push(thread::spawn(move || {
            for yields in schedule {
                for _ in 0..yields {
                    thread::yield_now();
                }
                let completed = Arc::clone(&completed);
                controller
                    .enqueue(move || {
                        completed.fetch_add(1, Ordering::AcqRel);
                    })
                    .expect("capacity reserves every stress item");
            }
            producers_done.fetch_add(1, Ordering::Release);
        }));
    }
    drain_until_complete(
        &receiver,
        &completed,
        &producers_done,
        config.producers,
        expected,
    )?;
    for producer in producers {
        producer
            .join()
            .map_err(|_| anyhow::anyhow!("stress producer panicked"))?;
    }
    let leave = activation.leave()?;
    ensure!(
        !leave.integration_failed,
        "stress adapter failed a host post"
    );
    ensure!(!leave.needs_runtime_pump, "stress leave stranded work");
    ensure!(
        adapter.deactivations.load(Ordering::Acquire) == 1,
        "stress adapter deactivated more or less than once"
    );
    let stale = adapter.prepared_turn();
    // SAFETY: this intentional late callback uses the still-live controller and
    // is invoked on the owner thread; the generation has already left.
    unsafe { (stale.owner.callback)(stale.owner.context.cast::<c_void>(), stale.generation) };
    ensure!(
        completed.load(Ordering::Acquire) == expected,
        "late callback executed managed work"
    );
    Ok(())
}

fn drain_until_complete(
    receiver: &Receiver<PostedTurn>,
    completed: &AtomicUsize,
    producers_done: &AtomicUsize,
    producer_count: usize,
    expected: usize,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while completed.load(Ordering::Acquire) < expected
        || producers_done.load(Ordering::Acquire) < producer_count
    {
        match receiver.try_recv() {
            Ok(turn) => {
                // SAFETY: delivery occurs on the owner thread while the active
                // controller and generation remain live.
                unsafe {
                    (turn.owner.callback)(turn.owner.context.cast::<c_void>(), turn.generation)
                };
            }
            Err(TryRecvError::Empty) => thread::yield_now(),
            Err(TryRecvError::Disconnected) => bail!("stress owner queue disconnected"),
        }
        ensure!(Instant::now() < deadline, "stress owner drain timed out");
    }
    while let Ok(turn) = receiver.try_recv() {
        // SAFETY: same owner-thread delivery contract as above.
        unsafe { (turn.owner.callback)(turn.owner.context.cast::<c_void>(), turn.generation) };
    }
    Ok(())
}

struct XorShift64(u64);

impl XorShift64 {
    fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9e37_79b9_7f4a_7c15
        } else {
            seed
        })
    }

    fn next(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_seed_host_stress_is_replayable() {
        run_host_stress(StressConfig {
            rounds: 8,
            items_per_producer: 16,
            ..StressConfig::default()
        })
        .unwrap();
    }
}
