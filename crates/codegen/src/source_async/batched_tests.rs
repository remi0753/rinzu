use super::cohort_tests::{receive, retire, start};
use super::extended_tests::{PollingStorage, arm_poll};
use super::*;
use rinzu_runtime::lifetime_trace::Probe;
use rinzu_runtime::native_completion::{
    CapturedNativeWake, NotifiedNativeCompletion, TransferNativeCompletion,
};
use rinzu_runtime_model::native_lifetime::{BatchSchedule, Binding};
use std::sync::atomic::Ordering;

mod trace {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/support/batched.rs"
    ));
}

fn token(g: usize) -> u64 {
    100 + u64::try_from(g).unwrap()
}

pub(super) fn capture(
    probe: &Probe,
    g: usize,
    raw: usize,
    record_target: bool,
) -> CapturedNativeWake {
    let _scope = Probe::producer(token(g));
    probe.record("publish_begin", token(g), 0, 0);
    // SAFETY: this call is the sole producer. The stable Box pins the
    // carrier through the exchange and the returned reference owns its tail.
    let wake = unsafe {
        NotifiedNativeCompletion::capture_with_lifetime_trace(
            raw as *mut NotifiedNativeCompletion,
            probe,
        )
    }
    .unwrap();
    if record_target {
        probe.record(
            "batch_target",
            token(g),
            i64::try_from(wake.address()).unwrap(),
            0,
        );
    }
    wake
}

pub(super) fn return_reference(probe: &Probe, g: usize, wake: CapturedNativeWake) {
    let _scope = Probe::producer(token(g));
    drop(wake);
    probe.record("publish_end", token(g), 0, 0);
}

fn collect_member(
    ops: &mut SourceOperations,
    shared: &Arc<Mutex<SharedState>>,
    probe: &Probe,
    future: [u64; 2],
    carrier: &mut Option<Box<TransferNativeCompletion>>,
    g: usize,
) {
    receive(ops, shared, future);
    probe.record("close_admission", token(g), 0, 0);
    drop(carrier.take().unwrap());
    probe.record("carrier_free", token(g), 0, 0);
    probe.record("sdk_pin_free", token(g), 0, 0);
}

fn collect_polling(
    ops: &mut SourceOperations,
    shared: &Arc<Mutex<SharedState>>,
    probe: &Probe,
    poll: Box<PollingStorage>,
    future: u64,
) {
    poll.publish(probe);
    receive(ops, shared, [future]);
    probe.record("close_admission", 200, 0, 0);
    drop(poll);
    probe.record("carrier_free", 200, 0, 0);
    probe.record("sdk_pin_free", 200, 0, 0);
}

fn run(module: &Arc<IrModule>, schedule: BatchSchedule, leader: usize, controlled: bool) {
    let split = schedule.split_admission();
    let p = Probe::new();
    let _scope = Probe::install(Arc::clone(&p));
    let owner = OwnerValues::new(module).unwrap();
    let shared = Arc::new(Mutex::new(SharedState::default()));
    let mut ops = SourceOperations::with_limits(
        Arc::clone(module),
        Arc::clone(&shared),
        &SourceAsyncOptions::default(),
        &owner,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        Arc::new(NativeBindings::default()),
        rinzu_runtime::resources::ResourceLimits {
            pending_operations: if split { 4 } else { 5 },
            ..Default::default()
        },
    );
    let mut storage = [7183, 29].map(|result| {
        Some(Box::new(TransferNativeCompletion {
            result,
            ..Default::default()
        }))
    });
    let raw = storage
        .each_mut()
        .map(|c| std::ptr::from_mut(c.as_deref_mut().unwrap()) as usize);
    let old = start(module, &owner, &shared, &mut ops, &p, raw[0], 0);
    p.wait("registered", 2);
    let aliases = old.map(|future| {
        let ActiveOperation::NativeCompletion { result, .. } = &ops.active[&future].1 else {
            unreachable!();
        };
        result.clone()
    });
    if controlled {
        assert!(ops.request_control(old[0], false));
    }
    let peer = start(module, &owner, &shared, &mut ops, &p, raw[1], 1);
    p.wait("registered", 4);
    let old_wake = capture(&p, 0, raw[0], false);
    {
        let _scope = Probe::producer(100);
        old_wake.notify();
    }
    receive(&mut ops, &shared, old);
    retire(&p, 0);
    let early_poll = split.then(|| arm_poll(module, &owner, &shared, &mut ops, &p));
    *storage[0].as_deref_mut().unwrap() = TransferNativeCompletion {
        result: 65537,
        ..Default::default()
    };
    let replacement = start(module, &owner, &shared, &mut ops, &p, raw[0], 2);
    p.wait("registered", if split { 5 } else { 6 });
    assert_eq!(ops.deferred_native.contains(&replacement[1]), split);
    let (poll, polling) =
        early_poll.unwrap_or_else(|| arm_poll(module, &owner, &shared, &mut ops, &p));
    let mut poll = Some(poll);
    assert_ne!(
        aliases[0].request_cancel(),
        rinzu_runtime::scheduler::ResolveResult::Won
    );
    assert_ne!(
        aliases[1].request_timeout(),
        rinzu_runtime::scheduler::ResolveResult::Won
    );
    assert_eq!(aliases[1].native_transfer_result(), Some(7183));
    p.record("alias_result", 100, 7183, 1);
    let new = &storage[0].as_ref().unwrap().completion;
    assert_eq!(new.base.cancelled.load(Ordering::Acquire), 0);
    assert_eq!(new.base.done.load(Ordering::Acquire), 0);
    p.record("generation_status", 102, 0, 0);
    if split && controlled {
        assert!(ops.request_control(replacement[1], true));
    }

    let together = schedule != BatchSchedule::Staggered;
    if together {
        collect_polling(&mut ops, &shared, &p, poll.take().unwrap(), polling);
    }
    if schedule == BatchSchedule::TogetherRetry {
        assert!(!ops.receive_one_ready().unwrap());
        assert!(!ops.deferred_native.contains(&replacement[1]));
        p.wait("registered", 6);
    }
    let groups = [old, peer, replacement];
    let duplicate = 3 - leader;
    let lead = capture(&p, leader, raw[usize::from(leader == 1)], true);
    if !together {
        collect_member(
            &mut ops,
            &shared,
            &p,
            groups[leader],
            &mut storage[usize::from(leader == 1)],
            leader,
        );
    }
    let covered = capture(&p, duplicate, raw[usize::from(duplicate == 1)], true);
    assert_eq!(lead.address(), covered.address());
    p.record(
        "merge_wake",
        token(duplicate),
        i64::try_from(token(leader)).unwrap(),
        i64::try_from(lead.address()).unwrap(),
    );
    return_reference(&p, duplicate, covered);
    {
        let _scope = Probe::producer(token(leader));
        lead.notify();
    }
    if together {
        // The reactor sends each packet before entering its wake callback.
        // These packets remain available when the Owner starts its next turn.
        let direct = schedule == BatchSchedule::TogetherDeferred;
        p.wait("worker_wake_begin", if direct { 6 } else { 7 });
        if direct {
            let occupied = ops.mailbox.resources().used.pending_operations;
            assert!(ops.receive_one_ready().unwrap());
            assert_eq!(ops.mailbox.resources().used.pending_operations, occupied);
            assert!(
                shared
                    .lock()
                    .unwrap()
                    .completed
                    .contains_key(&replacement[1])
            );
        }
        collect_member(&mut ops, &shared, &p, peer, &mut storage[1], 1);
        collect_member(&mut ops, &shared, &p, replacement, &mut storage[0], 2);
    } else {
        collect_member(
            &mut ops,
            &shared,
            &p,
            groups[duplicate],
            &mut storage[usize::from(duplicate == 1)],
            duplicate,
        );
        collect_polling(&mut ops, &shared, &p, poll.take().unwrap(), polling);
    }
    {
        let state = shared.lock().unwrap();
        for (g, futures) in groups.into_iter().enumerate() {
            for (i, id) in futures.into_iter().enumerate() {
                let result = &state.completed[&id];
                if controlled && ((g == 0 && i == 0) || (split && g == 2 && i == 1)) {
                    assert_eq!(
                        result.as_ref().unwrap_err().primary.cause,
                        FailureCause::Cancelled
                    );
                } else {
                    assert!(
                        matches!(result, Ok(Value::Scalar(v)) if v.bits() == [7183, 29, 65537][g])
                    );
                }
            }
        }
        assert!(matches!(&state.completed[&polling], Ok(Value::Unit)));
    }
    drop(aliases);
    p.wait(
        "record_release",
        if schedule == BatchSchedule::TogetherDeferred {
            6
        } else {
            7
        },
    );
    assert!(ops.is_baseline());
    drop(ops);
    p.record("owner_dispose", 0, 0, 0);
    return_reference(&p, 0, old_wake);
    return_reference(&p, leader, lead);
    trace::check(
        Binding::RustChannel,
        schedule,
        controlled,
        token(leader),
        &p.events(),
    );
    owner.borrow_mut().finish().unwrap();
}

#[test]
fn batches_native_receipts_across_reuse_and_both_recovery_orders() {
    let module = Arc::new(
        crate::test_library::compile_library(&rinzu_compiler::SourceFile::new(
            rinzu_compiler::FileId(887),
            "batched.rinzu",
            r#"
native { fn start(g:Int,callback:fn()->CSize)->CSize symbol "cohort_start"; }
fn answer()->CSize fallback 0 { return CSize(7183); }
async fn main() { let raw=start(0,answer); }
"#,
        ))
        .unwrap()
        .ir,
    );
    for schedule in [
        BatchSchedule::Staggered,
        BatchSchedule::Together,
        BatchSchedule::TogetherDeferred,
        BatchSchedule::TogetherRetry,
    ] {
        for leader in [1, 2] {
            for controlled in [false, true] {
                run(&module, schedule, leader, controlled);
            }
        }
    }
}
