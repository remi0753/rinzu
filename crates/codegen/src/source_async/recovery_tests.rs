use super::cohort_tests::{perform_call, publish, receive, start_completion};
use super::extended_tests::arm_poll;
use super::*;
use rinzu_runtime::lifetime_trace::Probe;
use rinzu_runtime::native_completion::{CompletionPointer, TransferNativeCompletion};
use rinzu_runtime_model::native_lifetime::Binding;

mod trace {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/support/unordered.rs"
    ));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeferredPath {
    Direct,
    Retry,
    ExchangeBeforeRegistration,
}

fn exchange_during_retry(
    probe: &Arc<Probe>,
    future: u64,
    raw: usize,
) -> std::thread::JoinHandle<()> {
    probe.block_for("drain_receipt", future);
    let probe = Arc::clone(probe);
    std::thread::spawn(move || {
        // The Owner already owns the credit, but has not submitted the ticket
        // to the reactor. Publication therefore observes an empty wake slot.
        probe.wait_for("drain_receipt", future, 1);
        publish(&probe, raw, 1).join().unwrap();
        probe.release_for("drain_receipt", future);
    })
}

fn run(module: &Arc<IrModule>, order: [usize; 3], path: DeferredPath, controlled: bool) {
    let probe = Probe::new();
    let _scope = Probe::install(Arc::clone(&probe));
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
            pending_operations: 2,
            ..Default::default()
        },
    );
    let mut carriers = [7183, 29].map(|result| {
        Some(Box::new(TransferNativeCompletion {
            result,
            ..Default::default()
        }))
    });
    let raw = carriers
        .each_mut()
        .map(|c| std::ptr::from_mut(c.as_deref_mut().unwrap()) as usize);
    let start = |ops: &mut SourceOperations, g: usize| {
        let generation = 100 + u64::try_from(g).unwrap();
        probe.observe_carrier(raw[g], generation);
        perform_call(module, &owner, &probe, raw[g], g);
        // SAFETY: each Box pins its carrier until the Owner has collected the receipt.
        let completion = unsafe { CompletionPointer::new(raw[g]) };
        start_completion(&shared, ops, &probe, completion, (generation, 0), true)
    };
    let first = start(&mut ops, 0);
    probe.wait("registered", 1);
    let (poll, polling) = arm_poll(module, &owner, &shared, &mut ops, &probe);
    let mut poll = Some(poll);
    let deferred = start(&mut ops, 1);
    assert!(ops.deferred_native.contains(&deferred));
    assert_eq!(ops.mailbox.resources().used.pending_operations, 2);
    if controlled {
        assert!(ops.request_control(deferred, true));
    }

    let futures = [first, deferred, polling];
    let mut producers = Vec::new();
    for g in order {
        let mut racing_publication = None;
        if g == 1 && path != DeferredPath::Direct {
            if path == DeferredPath::ExchangeBeforeRegistration {
                assert!(ops.deferred_native.contains(&deferred));
                racing_publication = Some(exchange_during_retry(&probe, deferred, raw[1]));
            }
            if ops.deferred_native.contains(&deferred) {
                ops.receive_one_ready().unwrap();
                assert!(!ops.deferred_native.contains(&deferred));
            }
            // A previous receive turn can submit this ticket before the reactor
            // finishes registration. The captured-target path needs that return.
            probe.wait("registered", 2);
        }
        if let Some(thread) = racing_publication {
            thread.join().unwrap();
        } else if g == 2 {
            poll.as_ref().unwrap().publish(&probe);
        } else {
            producers.push(publish(&probe, raw[g], g));
            probe.wait_for("sdk_exchange", 100 + u64::try_from(g).unwrap(), 1);
        }
        let used = ops.mailbox.resources().used.pending_operations;
        receive(&mut ops, &shared, [futures[g]]);
        if g == 1 && path == DeferredPath::Direct {
            assert_eq!(ops.mailbox.resources().used.pending_operations, used);
        }
        let id = [100, 101, 200][g];
        probe.record("close_admission", id, 0, 0);
        if g == 2 {
            drop(poll.take().unwrap());
        } else {
            drop(carriers[g].take().unwrap());
        }
        probe.record("carrier_free", id, 0, 0);
        probe.record("sdk_pin_free", id, 0, 0);
    }
    {
        let state = shared.lock().unwrap();
        assert!(matches!(&state.completed[&first], Ok(Value::Scalar(v)) if v.bits() == 7183));
        assert!(matches!(&state.completed[&polling], Ok(Value::Unit)));
        if controlled {
            assert_eq!(
                state.completed[&deferred]
                    .as_ref()
                    .unwrap_err()
                    .primary
                    .cause,
                FailureCause::Cancelled,
            );
        } else {
            assert!(matches!(&state.completed[&deferred], Ok(Value::Scalar(v)) if v.bits() == 29));
        }
    }
    let via_receipt = path != DeferredPath::Direct;
    probe.wait("record_release", 2 + usize::from(via_receipt));
    assert!(ops.is_baseline());
    let tail = Arc::clone(&probe);
    let release = std::thread::spawn(move || {
        tail.wait("owner_dispose", 1);
        for id in [100, 101] {
            tail.release_for("producer_release", id);
            tail.wait_for("publish_end", id, 1);
        }
    });
    drop(ops);
    probe.record("owner_dispose", 0, 0, 0);
    release.join().unwrap();
    for thread in producers {
        thread.join().unwrap();
    }
    let events = probe.events();
    let second_exchange = events
        .iter()
        .find(|s| s.starts_with("sdk_exchange,101,"))
        .unwrap();
    assert_eq!(
        second_exchange,
        if path == DeferredPath::Retry {
            "sdk_exchange,101,1,0"
        } else {
            "sdk_exchange,101,0,0"
        }
    );
    println!("UNORDERED Rust path={path:?}");
    trace::check(Binding::RustChannel, controlled, order, &events);
    owner.borrow_mut().finish().unwrap();
}

#[test]
fn ready_native_calls_recover_in_all_orders_with_deferred_and_polling() {
    let module = Arc::new(
        crate::test_library::compile_library(&rinzu_compiler::SourceFile::new(
            rinzu_compiler::FileId(886),
            "recovery.rinzu",
            r#"
native { fn start(g:Int,callback:fn()->CSize)->CSize symbol "cohort_start"; }
fn answer()->CSize fallback 0 { return CSize(7183); }
async fn main() { let raw=start(0,answer); }
"#,
        ))
        .unwrap()
        .ir,
    );
    for (index, order) in trace::PERMUTATIONS.into_iter().enumerate() {
        let path = if order[2] == 1 {
            DeferredPath::Retry
        } else {
            DeferredPath::Direct
        };
        run(&module, order, path, index >= 2);
    }
    run(&module, [0, 1, 2], DeferredPath::Retry, true);
    run(
        &module,
        [2, 1, 0],
        DeferredPath::ExchangeBeforeRegistration,
        true,
    );
}
