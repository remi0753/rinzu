use super::cohort_tests::{receive, start};
use super::*;
use rinzu_runtime::lifetime_trace::{DomainTrace, Probe};
use rinzu_runtime::native_completion::{NotifiedNativeCompletion, TransferNativeCompletion};

#[test]
fn managed_capture_cycle_retains_cached_native_futures_after_physical_release() {
    let module = Arc::new(
        crate::test_library::compile_library(&rinzu_compiler::SourceFile::new(
            rinzu_compiler::FileId(891),
            "native-capture-graph.rinzu",
            format!(
                "{}\nnative {{fn start(g:Int,callback:fn()->CSize)->CSize symbol \"cohort_start\";}}\nfn answer()->CSize fallback 0{{return CSize(7183);}}\nasync fn main(){{let raw=start(0,answer);}}",
                include_str!("../../tests/fixtures/native-capture-graph.rinzu")
            ),
        ))
        .unwrap()
        .ir,
    );
    for cancelled in [false, true] {
        run(&module, cancelled);
    }
}

fn run(module: &Arc<IrModule>, cancelled: bool) {
    let trace = Arc::new(DomainTrace::default());
    let probe = trace.probe(0);
    let _trace = Probe::install(Arc::clone(&probe));
    let owner = OwnerValues::new(module).unwrap();
    let shared = Arc::new(Mutex::new(SharedState::default()));
    owner.borrow_mut().future_state = Arc::downgrade(&shared);
    owner.borrow_mut().enable_gc_stress();
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let bindings = Arc::new(NativeBindings::default());
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let native = NativeEngine::new(Arc::clone(&bindings)).unwrap();
    let evaluate = |name, arguments| {
        evaluate_sync_context(
            module,
            name,
            arguments,
            Some(OwnerEvaluation {
                domain: None,
                callback: false,
                owner: &owner,
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                source_native: &native,
            }),
            &CancellationState::new(None),
            &mut Vec::new(),
            None,
        )
        .unwrap()
    };
    let mut operations = SourceOperations::new(
        Arc::clone(module),
        Arc::clone(&shared),
        &SourceAsyncOptions::default(),
        &owner,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        bindings,
    );
    let carrier = Box::new(TransferNativeCompletion {
        result: 7183,
        ..Default::default()
    });
    let raw = std::ptr::from_ref(&carrier.completion) as usize;
    let futures = start(module, &owner, &shared, &mut operations, &probe, raw, 0);
    probe.wait("registered", futures.len());
    if cancelled {
        assert!(operations.request_control(futures[1], false));
    }
    let graph = evaluate("capturePair", futures.map(Value::Future).into());
    let root = OwnerFrame::new(&owner, std::slice::from_ref(&graph));
    owner.borrow_mut().collect().unwrap();
    assert!(shared.lock().unwrap().completed.is_empty());
    assert!(
        !probe
            .events()
            .iter()
            .any(|row| row.starts_with("physical,"))
    );

    let retained = {
        let _sdk = Probe::producer(100);
        // SAFETY: both receipts and the SDK pin the initialized carrier through its only exchange.
        unsafe {
            NotifiedNativeCompletion::capture_with_lifetime_trace(
                raw as *mut NotifiedNativeCompletion,
                &probe,
            )
        }
        .unwrap()
    };
    retained.notify();
    receive(&mut operations, &shared, futures);
    {
        let mut shared = shared.lock().unwrap();
        for f in futures {
            shared.observe(0, f).unwrap();
        }
    }
    assert!(operations.is_baseline());
    drop(carrier);
    drop(operations);
    probe.wait("env_free", 1);
    let address = retained.address();
    let notifications = probe
        .events()
        .iter()
        .filter(|row| row.starts_with("sdk_progress,"))
        .count();
    retained.notify();
    drop(retained);
    probe.wait("target_free", 1);
    let events = probe.events();
    assert!(events.contains(&format!("producer_refs,0,{address},1")));
    assert_eq!(
        events
            .iter()
            .filter(|row| row.starts_with("sdk_progress,"))
            .count(),
        notifications
    );

    for _ in 0..2 {
        owner.borrow_mut().collect().unwrap();
        for (i, name) in ["capturedFirst", "capturedSecond"].into_iter().enumerate() {
            assert_eq!(
                evaluate(name, vec![graph.clone()]),
                Value::Future(futures[i])
            );
            let shared = shared.lock().unwrap();
            if cancelled && i == 1 {
                assert_eq!(
                    shared.completed[&futures[i]]
                        .as_ref()
                        .unwrap_err()
                        .primary
                        .cause,
                    FailureCause::Cancelled
                );
            } else {
                assert!(
                    matches!(shared.completed[&futures[i]], Ok(Value::Scalar(value)) if value.bits() == 7183)
                );
            }
        }
    }
    drop(root);
    drop(graph);
    owner.borrow_mut().collect().unwrap();
    let shared = shared.lock().unwrap();
    assert!(shared.completed.is_empty());
    assert_eq!(shared.gc_retired_futures, 2);
    drop(shared);
    owner.borrow_mut().finish().unwrap();
    println!("MANAGED NATIVE CAPTURE Rust cancelled={cancelled}");
}
