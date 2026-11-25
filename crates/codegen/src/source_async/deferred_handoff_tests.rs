use super::cohort_tests::{perform_call, receive, start_completion};
use super::extended_tests::arm_poll;
use super::*;
use rinzu_runtime::lifetime_trace::{DomainTrace, Probe};
use rinzu_runtime::native_completion::{
    CompletionPointer, NotifiedNativeCompletion, TransferNativeCompletion,
};
use rinzu_runtime::scheduler::ResolveResult;
use rinzu_runtime_model::native_lifetime::{
    Decision, DeferredPoll, DeferredPollAction as A, Protocol,
};
use std::sync::atomic::Ordering;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Publication {
    First,
    BothReady,
    DuringSelection,
    AfterPoll,
}

struct Domain {
    probe: Arc<Probe>,
    owner: Rc<RefCell<OwnerValues>>,
    shared: Arc<Mutex<SharedState>>,
    ops: SourceOperations,
}

impl Domain {
    fn new(module: &Arc<IrModule>, timeline: &Arc<DomainTrace>, d: usize) -> Self {
        let probe = timeline.probe(d);
        let _scope = Probe::install(Arc::clone(&probe));
        let owner = OwnerValues::new(module).unwrap();
        let shared = Arc::new(Mutex::new(SharedState::default()));
        let ops = SourceOperations::with_limits(
            Arc::clone(module),
            Arc::clone(&shared),
            &SourceAsyncOptions::default(),
            &owner,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            Arc::new(NativeBindings::default()),
            rinzu_runtime::resources::ResourceLimits {
                pending_operations: 1,
                ..Default::default()
            },
        );
        Self {
            probe,
            owner,
            shared,
            ops,
        }
    }

    fn start(&mut self, module: &Arc<IrModule>, raw: usize, receipts: usize) -> Vec<u64> {
        let _scope = Probe::install(Arc::clone(&self.probe));
        self.probe.observe_carrier(raw, 100);
        perform_call(module, &self.owner, &self.probe, raw, 0);
        (0..receipts)
            .map(|index| {
                // SAFETY: the SDK pins the same allocation through every admitted or direct copy.
                let completion = unsafe { CompletionPointer::new(raw) };
                start_completion(
                    &self.shared,
                    &mut self.ops,
                    &self.probe,
                    completion,
                    (100, index),
                    true,
                )
            })
            .collect()
    }

    fn ready(&mut self) {
        assert!(self.ops.receive_one_ready().unwrap());
    }

    fn completed(&self, f: u64) -> bool {
        self.shared.lock().unwrap().completed.contains_key(&f)
    }

    fn sample_target(&self, sample: usize, references: usize) -> usize {
        self.ops.reactor.trace_progress().unwrap();
        self.probe.wait_for("trace_progress_refs", 0, sample);
        let events = self.probe.events();
        let row = events
            .iter()
            .filter(|row| row.starts_with("trace_progress_refs,0,"))
            .nth(sample - 1)
            .unwrap();
        let fields: Vec<_> = row.split(',').collect();
        assert_eq!(fields[3].parse::<usize>().unwrap(), references);
        fields[2].parse().unwrap()
    }
}

fn publish_unregistered(raw: usize, probe: &Probe) {
    let _sdk = Probe::producer(100);
    probe.record("publish_begin", 100, 0, 0);
    // SAFETY: this is the unique final exchange of the pinned replacement carrier.
    let captured = unsafe {
        NotifiedNativeCompletion::capture_with_lifetime_trace(
            raw as *mut NotifiedNativeCompletion,
            probe,
        )
    };
    assert!(
        captured.is_none(),
        "a Deferred drain unexpectedly registered a target"
    );
    probe.record("publish_end", 100, 0, 0);
}

fn compare_deferred_trace(probe: &Probe, receivers: [u64; 2], poll: u64, operation: u64) {
    let mut model = DeferredPoll::default();
    let mut decisions = [None; 2];
    let mut settlements = [false; 2];
    let events = probe.events();
    for row in &events {
        let fields: Vec<_> = row.split(',').collect();
        let id = fields[1].parse::<u64>().unwrap();
        let arg = fields[2].parse::<i64>().unwrap();
        let receipt = receivers.iter().position(|&f| f == id);
        let action = match fields[0] {
            "drain_deferred" => Some(A::RejectAdmission(receipt.unwrap())),
            "deferred_begin" => {
                let index = receipt.unwrap();
                let decision = match arg {
                    1 => Decision::Cancelled,
                    2 => Decision::TimedOut,
                    _ => panic!("unknown control decision"),
                };
                assert!(decisions[index].replace(decision).is_none());
                None
            }
            "deferred_end" => {
                assert_eq!(arg, 1);
                let index = receipt.unwrap();
                Some(A::Control(index, decisions[index].take().unwrap()))
            }
            "drain_receipt" => {
                assert_eq!(id, poll);
                None
            }
            "sdk_exchange" => {
                assert_eq!((id, arg), (100, 0));
                Some(A::PublishSdk)
            }
            "physical" => {
                assert_eq!(id, operation);
                Some(A::PollReady)
            }
            "poll_selected" => {
                assert_eq!(id, poll);
                Some(A::SelectPoll)
            }
            "drain_release" if id == poll => Some(A::SettlePoll),
            "direct_copy" => {
                assert_eq!(arg, 7280);
                Some(A::DirectCopy(receipt.unwrap()))
            }
            "direct_settle" => {
                let index = receipt.unwrap();
                assert!(!std::mem::replace(&mut settlements[index], true));
                assert_eq!(
                    model.decision(index),
                    if arg == 1 {
                        Decision::Cancelled
                    } else {
                        assert_eq!(arg, 2);
                        Decision::TimedOut
                    }
                );
                None
            }
            "source_carrier_retire" => Some(A::RetireCarrier),
            _ => None,
        };
        if let Some(action) = action {
            assert!(
                model.actions().contains(&action),
                "{action:?} {row}\n{}",
                events.join("\n")
            );
            let before = model.clone();
            model.apply(action);
            before.check_step(action, &model).unwrap();
            model.check().unwrap();
        }
    }
    assert!(model.terminal() && settlements == [true, true]);
}

fn old_target_through_handoff(
    module: &Arc<IrModule>,
    timeline: &Arc<DomainTrace>,
    scalar: &mut TransferNativeCompletion,
) -> (
    Domain,
    rinzu_runtime::native_completion::CapturedNativeWake,
    ReactorOperation,
) {
    let mut old = Domain::new(module, timeline, 0);
    let raw = std::ptr::from_mut(scalar) as usize;
    let f = old.start(module, raw, 1)[0];
    let (operation, ActiveOperation::NativeCompletion { result, .. }) = &old.ops.active[&f] else {
        panic!("source operation missing its notified receipt");
    };
    let operation = *operation;
    let alias = result.clone();
    old.probe.wait_for("registered", operation.0, 1);
    let _sdk = Probe::producer(100);
    old.probe.record("publish_begin", 100, 0, 0);
    // SAFETY: the SDK's pinned carrier has one producer and a returned registration.
    let old_target = unsafe {
        NotifiedNativeCompletion::capture_with_lifetime_trace(
            raw as *mut NotifiedNativeCompletion,
            &old.probe,
        )
    }
    .unwrap();
    assert_eq!(old.sample_target(1, 2), old_target.address());
    old.probe.wait_for("worker_wake_end", operation.0, 1);
    receive(&mut old.ops, &old.shared, [f]);
    assert!(old.ops.is_baseline());
    old.probe
        .record("source_carrier_retire", 100, i64::try_from(raw).unwrap(), 0);
    (old, old_target, alias)
}

fn finish_origin(
    old: Domain,
    retained: rinzu_runtime::native_completion::CapturedNativeWake,
    attached: bool,
) {
    let target = retained.address();
    let _sdk = Probe::producer(100);
    if attached {
        retained.notify();
        old.probe.wait_for("sdk_progress", 0, 1);
    }
    drop(old.ops);
    old.probe.wait("env_free", 1);
    if !attached {
        retained.notify();
    }
    drop(retained);
    old.probe.wait("target_free", 1);
    let events = old.probe.events();
    assert!(events.contains(&format!("producer_refs,100,{target},1")));
    assert_eq!(
        events
            .iter()
            .filter(|line| line.starts_with("sdk_progress,"))
            .count(),
        usize::from(attached)
    );
    old.owner.borrow_mut().finish().unwrap();
}

fn handoff_module() -> Arc<IrModule> {
    Arc::new(
        crate::test_library::compile_library(&rinzu_compiler::SourceFile::new(
            rinzu_compiler::FileId(888),
            "deferred-handoff.rinzu",
            r#"
native {fn start(g:Int,callback:fn()->CSize)->CSize symbol "cohort_start";}
fn answer()->CSize fallback 0{return CSize(7183);}
async fn main(){let raw=start(0,answer);}
    "#,
        ))
        .unwrap()
        .ir,
    )
}

#[test]
fn deferred_poll_after_domain_handoff() {
    let module = handoff_module();
    for schedule in [
        Publication::First,
        Publication::BothReady,
        Publication::DuringSelection,
        Publication::AfterPoll,
    ] {
        for attached in [false, true] {
            let timeline = Arc::new(DomainTrace::default());
            let mut scalar = Box::new(TransferNativeCompletion {
                result: 7183,
                ..Default::default()
            });
            let (old, retained, alias) =
                old_target_through_handoff(&module, &timeline, &mut scalar);
            let raw = std::ptr::from_mut(&mut *scalar) as usize;
            *scalar = TransferNativeCompletion {
                result: 7280,
                ..Default::default()
            };
            let mut new = Domain::new(&module, &timeline, 1);
            new.probe
                .record("source_handoff", 100, i64::try_from(raw).unwrap(), 0);
            let (poll_storage, poll) =
                arm_poll(&module, &new.owner, &new.shared, &mut new.ops, &new.probe);
            let operation = new.ops.active[&poll].0;
            let receivers: [u64; 2] = new.start(&module, raw, 2).try_into().unwrap();
            assert_eq!(new.ops.deferred_native.len(), 2);
            assert_eq!(new.ops.mailbox.resources().used.pending_operations, 1);
            assert_eq!(alias.native_transfer_result(), Some(7183));
            assert_eq!(alias.request_cancel(), ResolveResult::Late);
            assert_eq!(alias.request_timeout(), ResolveResult::Late);
            drop(alias);
            assert_eq!(scalar.completion.base.cancelled.load(Ordering::Acquire), 0);
            let replacement_target = new.sample_target(1, 1);
            assert_ne!(retained.address(), replacement_target);
            let epoch = new.ops.completion_signal.epoch();
            finish_origin(old, retained, attached);
            assert_eq!(new.sample_target(2, 1), replacement_target);
            assert_eq!(new.ops.completion_signal.epoch(), epoch);
            assert_eq!(scalar.completion.notification.load(Ordering::Acquire), 0);
            assert!(new.ops.request_control(receivers[0], false));
            assert!(new.ops.request_control(receivers[1], true));
            assert!(!new.completed(poll) && receivers.iter().all(|&f| !new.completed(f)));
            let probe = Arc::clone(&new.probe);
            new.ops.after_ready_selection = Some(Box::new(move |selected| {
                assert_eq!(selected, poll);
                probe.record("poll_selected", selected, 0, 0);
                if schedule == Publication::DuringSelection {
                    publish_unregistered(raw, &probe);
                }
            }));
            if schedule == Publication::First {
                publish_unregistered(raw, &new.probe);
                new.ready();
                new.ready();
                assert!(!new.completed(poll));
                assert_eq!(new.ops.mailbox.resources().used.pending_operations, 1);
            }
            poll_storage.publish(&new.probe);
            new.probe.wait_for("worker_wake_end", operation.0, 1);
            if schedule == Publication::BothReady {
                publish_unregistered(raw, &new.probe);
            }
            new.ready();
            if matches!(
                schedule,
                Publication::AfterPoll | Publication::DuringSelection
            ) {
                assert!(new.completed(poll));
                assert!(receivers.iter().all(|&f| !new.completed(f)));
                if schedule == Publication::AfterPoll {
                    publish_unregistered(raw, &new.probe);
                }
            } else if schedule == Publication::BothReady {
                assert!(!new.completed(poll));
                assert_eq!(new.ops.mailbox.resources().used.pending_operations, 1);
            }
            receive(
                &mut new.ops,
                &new.shared,
                [poll, receivers[0], receivers[1]],
            );
            assert!(new.ops.is_baseline());
            assert!(matches!(
                new.shared.lock().unwrap().completed[&poll],
                Ok(Value::Unit)
            ));
            for f in receivers {
                assert_eq!(
                    new.shared.lock().unwrap().completed[&f]
                        .as_ref()
                        .unwrap_err()
                        .primary
                        .cause,
                    FailureCause::Cancelled
                );
            }
            new.probe
                .record("source_carrier_retire", 100, i64::try_from(raw).unwrap(), 0);
            assert_eq!(scalar.completion.notification.load(Ordering::Acquire), 1);
            drop(new.ops);
            new.probe.wait("env_free", 1);
            assert!(
                !new.probe
                    .events()
                    .iter()
                    .any(|e| e.starts_with("sdk_progress,"))
            );
            compare_deferred_trace(&new.probe, receivers, poll, operation.0);
            new.owner.borrow_mut().finish().unwrap();
            println!(
                "SOURCE HANDOFF {schedule:?} old_attached={attached} events={}",
                timeline.events().len()
            );
        }
    }
}

#[test]
fn deferred_retry_after_domain_handoff() {
    let module = handoff_module();
    for early in [false, true] {
        for attached in [false, true] {
            for timed in [false, true] {
                retry_after_handoff(&module, early, attached, timed);
            }
        }
    }
}

fn retry_after_handoff(module: &Arc<IrModule>, early: bool, attached: bool, timed: bool) {
    let timeline = Arc::new(DomainTrace::default());
    let mut scalar = Box::new(TransferNativeCompletion {
        result: 7183,
        ..Default::default()
    });
    let (old, retained, alias) = old_target_through_handoff(module, &timeline, &mut scalar);
    let raw = std::ptr::from_mut(&mut *scalar) as usize;
    *scalar = TransferNativeCompletion {
        result: 7280,
        ..Default::default()
    };
    let mut new = Domain::new(module, &timeline, 1);
    new.probe
        .record("source_handoff", 100, i64::try_from(raw).unwrap(), 0);
    let (poll_storage, poll) = arm_poll(module, &new.owner, &new.shared, &mut new.ops, &new.probe);
    let polling_op = new.ops.active[&poll].0.0;
    let receivers: [u64; 2] = new.start(module, raw, 2).try_into().unwrap();
    assert!(receivers[0] < receivers[1]);
    assert!(new.ops.request_control(receivers[0], timed));
    assert!(new.ops.request_control(receivers[1], !timed));
    assert_eq!(new.ops.mailbox.resources().used.pending_operations, 1);
    assert_eq!(new.ops.deferred_native.len(), 2);
    assert_eq!(alias.native_transfer_result(), Some(7183));
    assert_eq!(alias.request_timeout(), ResolveResult::Late);
    drop(alias);

    poll_storage.publish(&new.probe);
    new.probe.wait_for("worker_wake_end", polling_op, 1);
    let probe = Arc::clone(&new.probe);
    new.ops.after_ready_selection = Some(Box::new(move |selected| {
        assert_eq!(selected, poll);
        probe.record("poll_selected", selected, 0, 0);
    }));
    new.ready();
    assert!(new.completed(poll));
    assert_eq!(new.ops.mailbox.resources().used.pending_operations, 0);
    let replacement_target = new.sample_target(1, 1);
    assert_ne!(retained.address(), replacement_target);

    new.probe.block("register_retain");
    assert!(!new.ops.receive_one_ready().unwrap());
    let retried = new.ops.active[&receivers[0]].0.0;
    assert!(new.ops.deferred_native.contains(&receivers[1]));
    assert_eq!(new.ops.mailbox.resources().used.pending_operations, 1);
    new.probe.wait_for("register_retain", retried, 1);
    assert_eq!(scalar.completion.notification.load(Ordering::Acquire), 0);
    let epoch = new.ops.completion_signal.epoch();
    finish_origin(old, retained, attached);
    assert_eq!(new.ops.completion_signal.epoch(), epoch);
    assert_eq!(scalar.completion.notification.load(Ordering::Acquire), 0);
    if early {
        publish_unregistered(raw, &new.probe);
    }
    new.probe.release("register_retain");
    new.probe.wait_for("registered", retried, 1);
    let replacement_reference = if early {
        assert_eq!(new.sample_target(2, 1), replacement_target);
        None
    } else {
        assert_eq!(new.sample_target(2, 2), replacement_target);
        let _sdk = Probe::producer(100);
        // SAFETY: this is the only final exchange and the new Domain still pins the carrier.
        let captured = unsafe {
            NotifiedNativeCompletion::capture_with_lifetime_trace(
                raw as *mut NotifiedNativeCompletion,
                &new.probe,
            )
        }
        .unwrap();
        assert_eq!(captured.address(), replacement_target);
        captured.notify();
        Some(captured)
    };
    new.probe.wait_for("worker_wake_end", retried, 1);
    new.ready();
    assert!(!new.completed(receivers[0]) && new.completed(receivers[1]));
    assert_eq!(new.ops.mailbox.resources().used.pending_operations, 1);
    new.ready();
    assert!(new.completed(receivers[0]) && new.ops.is_baseline());
    new.probe
        .record("source_carrier_retire", 100, i64::try_from(raw).unwrap(), 0);
    drop(scalar);
    drop(new.ops);
    new.probe.wait("env_free", 1);
    if let Some(reference) = replacement_reference {
        let _sdk = Probe::producer(100);
        drop(reference);
    }
    new.probe.wait("target_free", 1);
    compare_retry_trace(&new.probe, receivers, poll, polling_op, retried, timed);
    new.owner.borrow_mut().finish().unwrap();
    println!("SOURCE HANDOFF retry early={early} attached={attached} timeout_first={timed}");
}

fn compare_retry_trace(
    probe: &Probe,
    receivers: [u64; 2],
    poll: u64,
    polling_op: u64,
    retried: u64,
    timed: bool,
) {
    let mut model = DeferredPoll::allowing_retry();
    let expected = if timed {
        [Decision::TimedOut, Decision::Cancelled]
    } else {
        [Decision::Cancelled, Decision::TimedOut]
    };
    let mut copied = false;
    let mut transferred = false;
    let events = probe.events();
    for row in &events {
        let fields: Vec<_> = row.split(',').collect();
        let id = fields[1].parse::<u64>().unwrap();
        let a = fields[2].parse::<i64>().unwrap();
        let action = match fields[0] {
            "drain_deferred" => Some(A::RejectAdmission(
                receivers.iter().position(|&f| f == id).unwrap(),
            )),
            "deferred_begin" => {
                let index = receivers.iter().position(|&f| f == id).unwrap();
                assert_eq!(
                    a,
                    if expected[index] == Decision::TimedOut {
                        2
                    } else {
                        1
                    }
                );
                Some(A::Control(index, expected[index]))
            }
            "drain_receipt" if id == receivers[0] => Some(A::Retry(0)),
            "register_cas" => {
                assert_eq!(id, retried);
                transferred = a == 1;
                None
            }
            "registered" => {
                assert_eq!(id, retried);
                assert_eq!(a, i64::from(transferred));
                Some(A::RegisterRetry(0))
            }
            "sdk_exchange" => {
                assert_eq!(a, i64::from(transferred));
                Some(A::PublishSdk)
            }
            "physical" if id == polling_op => Some(A::PollReady),
            "poll_selected" => {
                assert_eq!(id, poll);
                Some(A::SelectPoll)
            }
            "drain_release" if id == poll => Some(A::SettlePoll),
            "copied" if id == retried => {
                assert_eq!(a, 7280);
                copied = true;
                None
            }
            "physical" if id == retried => {
                assert!(copied);
                Some(A::CopyReceipt(0))
            }
            "direct_copy" => {
                assert_eq!((id, a), (receivers[1], 7280));
                Some(A::DirectCopy(1))
            }
            "receipt_decision" if id == receivers[0] => {
                assert_eq!(a, if timed { 2 } else { 1 });
                Some(A::SettleReceipt(0))
            }
            "direct_settle" => {
                assert_eq!((id, a), (receivers[1], if timed { 1 } else { 2 }));
                None
            }
            "source_carrier_retire" => Some(A::RetireCarrier),
            "producer_released" => Some(A::ReturnSdk),
            _ => None,
        };
        if let Some(action) = action {
            assert!(
                model.actions().contains(&action),
                "{row} {model:?}\n{}",
                events.join("\n")
            );
            let before = model.clone();
            model.apply(action);
            before.check_step(action, &model).unwrap();
            model.check().unwrap();
            if action == A::PublishSdk {
                assert_eq!(model.sdk_has_reference(), a == 1);
            }
        }
    }
    assert!(model.terminal(), "{model:?}\n{}", events.join("\n"));
    assert_eq!([model.decision(0), model.decision(1)], expected);
}
