use core::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use rinzu_runtime::host_loop::{
    HostLoopAdapter, HostLoopController, HostPostResult, OwnerTurnTarget, PostOutcome,
};

struct VectorAdapter {
    outcome: HostPostResult,
    owner: Mutex<Option<(OwnerTurnTarget, u64)>>,
    deactivations: AtomicUsize,
}

impl HostLoopAdapter for VectorAdapter {
    fn prepare(&self, owner: OwnerTurnTarget, generation: u64) -> Result<usize, String> {
        *self.owner.lock().unwrap() = Some((owner, generation));
        Ok(1)
    }

    fn post_owner_turn(&self, _ready_token: usize, _generation: u64) -> HostPostResult {
        self.outcome
    }

    fn deactivate(&self, _ready_token: usize, _generation: u64) {
        self.deactivations.fetch_add(1, Ordering::AcqRel);
    }
}

#[test]
fn production_host_loop_consumes_the_reference_model_vectors() {
    let document: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/conformance/runtime-model-host-vectors.json"
    ))
    .unwrap();
    assert_eq!(document["schema"], "rinzu.runtime-model.host-vectors");
    assert_eq!(document["version"], 1);
    for vector in document["vectors"].as_array().unwrap() {
        run_vector(vector);
    }
}

fn run_vector(vector: &serde_json::Value) {
    let name = vector["name"].as_str().unwrap();
    let outcome = match vector["postResult"].as_str().unwrap() {
        "accepted" => HostPostResult::Accepted,
        "closed" => HostPostResult::Closed,
        "failure" => HostPostResult::Failure,
        value => panic!("unknown post result `{value}` in {name}"),
    };
    let adapter = Arc::new(VectorAdapter {
        outcome,
        owner: Mutex::new(None),
        deactivations: AtomicUsize::new(0),
    });
    let runtime_adapter: Arc<dyn HostLoopAdapter> = adapter.clone();
    let controller = HostLoopController::new(runtime_adapter, 2, 1);
    let activation = controller.enter().unwrap();
    let managed_turns = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&managed_turns);
    let post = controller
        .enqueue(move || {
            count.fetch_add(1, Ordering::AcqRel);
        })
        .unwrap();
    match outcome {
        HostPostResult::Accepted => {
            assert_eq!(post, PostOutcome::Accepted, "{name}");
            let (owner, generation) = adapter.owner.lock().unwrap().unwrap();
            // SAFETY: the controller and active generation remain live and the
            // vector invokes the owner callback on the creating thread.
            unsafe { (owner.callback)(owner.context.cast::<c_void>(), generation) };
        }
        HostPostResult::Closed => assert_eq!(post, PostOutcome::Closed, "{name}"),
        HostPostResult::Failure => assert_eq!(post, PostOutcome::Failed, "{name}"),
    }
    let leave = activation.leave().unwrap();
    let expected_failure = vector["terminal"] == "fail-fast";
    assert_eq!(leave.integration_failed, expected_failure, "{name}");
    assert_eq!(
        leave.needs_runtime_pump,
        vector["runtimePumpRequired"].as_bool().unwrap(),
        "{name}"
    );
    assert_eq!(
        managed_turns.load(Ordering::Acquire),
        vector["managedTurns"].as_u64().unwrap() as usize,
        "{name}"
    );
    assert_eq!(adapter.deactivations.load(Ordering::Acquire), 1, "{name}");
}
