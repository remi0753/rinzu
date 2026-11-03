//! Generic Host Loop model scenarios retained from v0.1.
//! Current native platform conformance lives with the source SDK fixtures.

pub mod resource_probe;
pub mod stress;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Result, ensure};
use rinzu_runtime::host_loop::{HostLoopController, HostLoopError, HostPhase, LeaveReport};

pub const HOST_CONFORMANCE_CAPACITY: usize = 8;
pub const HOST_CONFORMANCE_TURN_BUDGET: usize = 2;

pub struct HostScenario {
    completed: Arc<AtomicUsize>,
}

pub fn stage_host_scenario(
    controller: &Arc<HostLoopController>,
    request_shutdown: impl Fn() + Send + Sync + 'static,
) -> Result<HostScenario> {
    let completed = Arc::new(AtomicUsize::new(0));
    let request_shutdown = Arc::new(request_shutdown);
    for index in 0..HOST_CONFORMANCE_CAPACITY {
        let completed = Arc::clone(&completed);
        let request_shutdown = Arc::clone(&request_shutdown);
        controller.enqueue(move || {
            completed.fetch_add(1, Ordering::Relaxed);
            if index + 1 == HOST_CONFORMANCE_CAPACITY {
                request_shutdown();
            }
        })?;
    }
    ensure!(
        matches!(controller.enqueue(|| {}), Err(HostLoopError::QueueFull)),
        "Host adapter did not expose bounded event saturation"
    );
    Ok(HostScenario { completed })
}

pub fn verify_host_scenario(
    controller: &HostLoopController,
    generation: u64,
    leave: LeaveReport,
    nested_delivery_supported: bool,
    scenario: &HostScenario,
) -> Result<()> {
    ensure!(
        scenario.completed.load(Ordering::Relaxed) == HOST_CONFORMANCE_CAPACITY,
        "Host adapter did not drain every admitted event exactly once"
    );
    ensure!(
        !leave.integration_failed,
        "Host integration closed as failure"
    );
    ensure!(
        !leave.needs_runtime_pump,
        "Host leave stranded managed work"
    );
    ensure!(
        controller.snapshot().phase == HostPhase::Managed,
        "Host leave did not return the controller to Managed"
    );
    let late = controller.owner_turn(generation)?;
    ensure!(
        late.stale && late.executed == 0,
        "late callback entered managed code"
    );
    ensure!(
        !nested_delivery_supported,
        "historical Host fixtures must not claim untested nested delivery"
    );
    Ok(())
}
