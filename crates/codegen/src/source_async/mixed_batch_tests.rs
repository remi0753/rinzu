use super::batched_tests::{capture, return_reference};
use super::cohort_tests::{perform_call_in_slot, receive, start_completion};
use super::extended_tests::{PollingStorage, arm_poll_in_slot};
use super::*;
use rinzu_runtime::lifetime_trace::Probe;
use rinzu_runtime::native_completion::{
    CapturedNativeWake, CompletionPointer, NativeCompletion, NotifiedNativeCompletion,
    TransferNativeCompletion,
};
use rinzu_runtime_model::native_lifetime::{Binding, MixedBatchSchedule};
use std::sync::atomic::Ordering;

mod trace {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/support/mixed_batches.rs"
    ));
}

fn token(g: usize) -> u64 {
    100 + u64::try_from(g).unwrap()
}

enum Carrier {
    Transfer(Box<TransferNativeCompletion>),
    Unit(Box<NotifiedNativeCompletion>),
}

impl Carrier {
    fn header(&self) -> &NativeCompletion {
        match self {
            Self::Transfer(c) => &c.completion.base,
            Self::Unit(c) => &c.base,
        }
    }

    fn address(&self) -> usize {
        std::ptr::from_ref(self.header()) as usize
    }
}

struct Fixture<'a> {
    module: &'a Arc<IrModule>,
    owner: Rc<RefCell<OwnerValues>>,
    shared: Arc<Mutex<SharedState>>,
    ops: SourceOperations,
    probe: Arc<Probe>,
    carriers: [Option<Carrier>; 4],
}

impl<'a> Fixture<'a> {
    fn new(module: &'a Arc<IrModule>, probe: Arc<Probe>) -> Self {
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
                pending_operations: 6,
                ..Default::default()
            },
        );
        let carriers = [
            Some(Carrier::Transfer(Box::new(TransferNativeCompletion {
                result: 7183,
                ..Default::default()
            }))),
            Some(Carrier::Unit(Box::default())),
            None,
            Some(Carrier::Unit(Box::default())),
        ];
        Self {
            module,
            owner,
            shared,
            ops,
            probe,
            carriers,
        }
    }

    fn start<const N: usize>(&mut self, g: usize) -> [u64; N] {
        let carrier = self.carriers[g].as_ref().unwrap();
        let raw = carrier.address();
        let slot = [0, 1, 0, 2][g];
        self.probe.observe_carrier(raw, token(g));
        perform_call_in_slot(self.module, &self.owner, &self.probe, raw, (g, slot));
        let header = carrier.header();
        self.probe.record(
            "carrier_abi",
            token(g),
            i64::from(header.version),
            i64::from(header.size),
        );
        // SAFETY: this SDK allocation remains pinned through the copy and closure of every drain.
        let pointer = unsafe { CompletionPointer::new(raw) };
        std::array::from_fn(|i| {
            start_completion(
                &self.shared,
                &mut self.ops,
                &self.probe,
                pointer,
                (token(g), i),
                header.version == 3,
            )
        })
    }

    fn collect<const N: usize>(&mut self, g: usize, futures: [u64; N]) {
        receive(&mut self.ops, &self.shared, futures);
        self.probe.record("close_admission", token(g), 0, 0);
        if g != 0 {
            drop(self.carriers[g].take().unwrap());
        }
        self.probe.record("carrier_free", token(g), 0, 0);
        self.probe.record("sdk_pin_free", token(g), 0, 0);
    }

    fn collect_polling(&mut self, future: u64, poll: Box<PollingStorage>, wakes: usize) {
        poll.publish(&self.probe);
        self.probe.wait("worker_wake_end", wakes);
        receive(&mut self.ops, &self.shared, [future]);
        self.probe.record("close_admission", 200, 0, 0);
        drop(poll);
        self.probe.record("carrier_free", 200, 0, 0);
        self.probe.record("sdk_pin_free", 200, 0, 0);
    }

    fn capture(&self, g: usize) -> CapturedNativeWake {
        capture(
            &self.probe,
            g,
            self.carriers[g].as_ref().unwrap().address(),
            true,
        )
    }
}

fn notify_batch(
    probe: &Probe,
    leader: usize,
    wake: &CapturedNativeWake,
    duplicate: CapturedNativeWake,
) {
    assert_eq!(wake.address(), duplicate.address());
    probe.record(
        "merge_wake",
        token(leader ^ 1),
        i64::try_from(token(leader)).unwrap(),
        i64::try_from(wake.address()).unwrap(),
    );
    return_reference(probe, leader ^ 1, duplicate);
    let _scope = Probe::producer(token(leader));
    wake.notify();
}

fn run(
    module: &Arc<IrModule>,
    schedule: MixedBatchSchedule,
    leaders: [usize; 2],
    controlled: bool,
) {
    let probe = Probe::new();
    let _scope = Probe::install(Arc::clone(&probe));
    let mut f = Fixture::new(module, Arc::clone(&probe));
    let old: [u64; 2] = f.start(0);
    probe.wait("registered", 2);
    let aliases = old.map(|future| {
        let ActiveOperation::NativeCompletion { result, .. } = &f.ops.active[&future].1 else {
            unreachable!();
        };
        result.clone()
    });
    if controlled {
        assert!(f.ops.request_control(old[0], false));
    }
    let [first_unit] = f.start(1);
    probe.wait("registered", 3);
    let (poll, polling) = arm_poll_in_slot(module, &f.owner, &f.shared, &mut f.ops, &probe, true);
    let mut poll = Some(poll);
    let together = schedule == MixedBatchSchedule::Together;
    if together {
        f.collect_polling(polling, poll.take().unwrap(), 1);
    }
    let [first_leader, second_leader] = leaders;
    let first_wake = f.capture(first_leader);
    if !together {
        if first_leader == 0 {
            probe.wait("worker_wake_end", 2);
            f.collect(0, old);
        } else {
            probe.wait("worker_wake_end", 1);
            f.collect(1, [first_unit]);
        }
    }
    let first_duplicate = f.capture(first_leader ^ 1);
    notify_batch(&probe, first_leader, &first_wake, first_duplicate);
    probe.wait("worker_wake_end", if together { 4 } else { 3 });
    if together || first_leader != 0 {
        f.collect(0, old);
    }

    let mut recycled = f.carriers[0].take().unwrap();
    let Carrier::Transfer(storage) = &mut recycled else {
        unreachable!()
    };
    **storage = TransferNativeCompletion {
        result: 65537,
        ..Default::default()
    };
    f.carriers[2] = Some(recycled);
    let [replacement] = f.start(2);
    probe.wait("registered", 4);
    let [second_unit] = f.start(3);
    probe.wait("registered", 5);
    use rinzu_runtime::scheduler::ResolveResult;
    assert_ne!(aliases[0].request_cancel(), ResolveResult::Won);
    assert_ne!(aliases[1].request_timeout(), ResolveResult::Won);
    assert_eq!(aliases[1].native_transfer_result(), Some(7183));
    probe.record("alias_result", 100, 7183, 1);
    let fresh = f.carriers[2].as_ref().unwrap().header();
    assert_eq!(fresh.cancelled.load(Ordering::Acquire), 0);
    assert_eq!(fresh.done.load(Ordering::Acquire), 0);
    probe.record("generation_status", 102, 0, 0);

    let second_wake = f.capture(second_leader);
    let second_wake = if schedule == MixedBatchSchedule::LateArrival {
        probe.wait("worker_wake_end", 4);
        let late_raw = f.carriers[2].as_ref().unwrap().address();
        let late_probe = Arc::clone(&probe);
        let retained = Rc::new(RefCell::new(None));
        let returned_wake = Rc::clone(&retained);
        f.ops.after_ready_selection = Some(Box::new(move |future| {
            assert_eq!(future, second_unit);
            late_probe.record("ready_selection", future, 0, 0);
            let duplicate = capture(&late_probe, 2, late_raw, true);
            notify_batch(&late_probe, second_leader, &second_wake, duplicate);
            late_probe.wait("worker_wake_end", 5);
            returned_wake.replace(Some(second_wake));
        }));
        f.collect(3, [second_unit]);
        retained.take().unwrap()
    } else {
        if !together {
            probe.wait("worker_wake_end", 4);
            if second_leader == 2 {
                f.collect(2, [replacement]);
            } else {
                f.collect(3, [second_unit]);
            }
        }
        let second_duplicate = f.capture(second_leader ^ 1);
        notify_batch(&probe, second_leader, &second_wake, second_duplicate);
        second_wake
    };
    probe.wait("worker_wake_end", if together { 6 } else { 5 });
    if together || first_leader == 0 {
        f.collect(1, [first_unit]);
    }
    if together || second_leader != 2 {
        f.collect(2, [replacement]);
    }
    if together || second_leader != 3 {
        f.collect(3, [second_unit]);
    }
    if !together {
        f.collect_polling(polling, poll.take().unwrap(), 6);
    }

    {
        let state = f.shared.lock().unwrap();
        for (future, expected) in [(old[1], 7183), (replacement, 65537)] {
            assert!(
                matches!(&state.completed[&future], Ok(Value::Scalar(v)) if v.bits() == expected)
            );
        }
        for future in [first_unit, second_unit, polling] {
            assert!(matches!(&state.completed[&future], Ok(Value::Unit)));
        }
        if controlled {
            assert!(
                matches!(&state.completed[&old[0]], Err(f) if f.primary.cause == FailureCause::Cancelled)
            );
        } else {
            assert!(matches!(&state.completed[&old[0]], Ok(Value::Scalar(v)) if v.bits() == 7183));
        }
    }
    drop(aliases);
    probe.wait("record_release", 6);
    assert!(f.ops.is_baseline());
    drop(f.ops);
    probe.record("owner_dispose", 0, 0, 0);
    let reverse = first_leader != second_leader - 2;
    let mut releases = [(first_leader, first_wake), (second_leader, second_wake)];
    if reverse {
        releases.reverse();
    }
    for (g, wake) in releases {
        return_reference(&probe, g, wake);
    }
    trace::check(
        Binding::RustChannel,
        schedule,
        controlled,
        leaders.map(token),
        reverse,
        &probe.events(),
    );
    f.owner.borrow_mut().finish().unwrap();
}

#[test]
fn mixed_abis_share_consecutive_notification_batches() {
    let module = Arc::new(
        crate::test_library::compile_library(&rinzu_compiler::SourceFile::new(
            rinzu_compiler::FileId(888),
            "mixed-batches.rinzu",
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
        MixedBatchSchedule::Staggered,
        MixedBatchSchedule::Together,
        MixedBatchSchedule::LateArrival,
    ] {
        for leaders in [[0, 2], [0, 3], [1, 2], [1, 3]] {
            if schedule == MixedBatchSchedule::LateArrival && leaders != [1, 3] {
                continue;
            }
            for controlled in [false, true] {
                run(&module, schedule, leaders, controlled);
            }
        }
    }
}
