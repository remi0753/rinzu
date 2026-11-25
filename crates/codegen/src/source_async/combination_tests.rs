use super::cohort_tests::{publish, receive, retire, start, start_mode};
use super::*;
use rinzu_runtime::lifetime_trace::Probe;
use rinzu_runtime::native_completion::TransferNativeCompletion;
use rinzu_runtime_model::native_lifetime::{Binding, CohortMode, replay_combination};
mod adversarial {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/support/combination.rs"
    ));
}
#[test]
fn early_publication_and_direct_deferred_collection_share_multiple_calls() {
    let module = Arc::new(
        crate::test_library::compile_library(&rinzu_compiler::SourceFile::new(
            rinzu_compiler::FileId(884),
            "combination.rinzu",
            r#"
        native {fn start(g:Int,callback:fn()->CSize)->CSize symbol "cohort_start";}
        fn answer()->CSize fallback 0{return CSize(7183);}
        async fn main(){let pointer=start(0,answer);}
        "#,
        ))
        .unwrap()
        .ir,
    );
    for mode in [CohortMode::Early, CohortMode::Deferred] {
        for controlled in [false, true] {
            let p = Probe::new();
            let _scope = Probe::install(Arc::clone(&p));
            let owner = OwnerValues::new(&module).unwrap();
            let shared = Arc::new(Mutex::new(SharedState::default()));
            let mut operations = SourceOperations::with_limits(
                Arc::clone(&module),
                Arc::clone(&shared),
                &SourceAsyncOptions::default(),
                &owner,
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                Arc::new(NativeBindings::default()),
                rinzu_runtime::resources::ResourceLimits {
                    pending_operations: if mode == CohortMode::Early { 4 } else { 2 },
                    ..Default::default()
                },
            );
            let mut storage = [
                Box::new(TransferNativeCompletion {
                    result: 7183,
                    ..Default::default()
                }),
                Box::new(TransferNativeCompletion {
                    result: 29,
                    ..Default::default()
                }),
            ];
            let raw0 = std::ptr::from_mut(&mut *storage[0]) as usize;
            let raw1 = std::ptr::from_mut(&mut *storage[1]) as usize;
            let first = start(&module, &owner, &shared, &mut operations, &p, raw0, 0);
            p.wait("registered", 2);
            let aliases: Vec<_> = first
                .iter()
                .map(|f| {
                    let ActiveOperation::NativeCompletion { result, .. } = &operations.active[f].1
                    else {
                        unreachable!()
                    };
                    result.clone()
                })
                .collect();
            if controlled {
                assert!(operations.request_control(first[0], false));
            }
            let second = start_mode(
                &module,
                &owner,
                &shared,
                &mut operations,
                &p,
                raw1,
                1,
                mode == CohortMode::Early,
            );
            let mut producers = Vec::new();
            if mode == CohortMode::Deferred {
                assert_eq!(operations.deferred_native.len(), 2);
                if controlled {
                    assert!(operations.request_control(second[0], false));
                    assert!(operations.request_control(second[1], true));
                }
                producers.push(publish(&p, raw1, 1));
                p.wait_for("sdk_exchange", 101, 1);
                receive(&mut operations, &shared, second);
                assert_eq!(operations.mailbox.resources().used.pending_operations, 2);
                assert_eq!(
                    p.events()
                        .iter()
                        .filter(|e| e.starts_with("receipt,"))
                        .count(),
                    2
                );
                retire(&p, 1);
            }
            producers.push(publish(&p, raw0, 0));
            p.wait_for("sdk_exchange", 100, 1);
            receive(&mut operations, &shared, first);
            retire(&p, 0);
            if mode == CohortMode::Early {
                p.wait("registered", 4);
            }
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
            if mode == CohortMode::Early {
                receive(&mut operations, &shared, second);
                p.wait("record_release", 2);
                retire(&p, 1);
            }
            for (g, fs) in [(0, first), (1, second)] {
                for (i, f) in fs.into_iter().enumerate() {
                    let state = shared.lock().unwrap();
                    let result = &state.completed[&f];
                    if controlled && (g == 0 && i == 0 || mode == CohortMode::Deferred && g == 1) {
                        assert_eq!(
                            result.as_ref().unwrap_err().primary.cause,
                            FailureCause::Cancelled
                        );
                    } else {
                        assert!(
                            matches!(result,Ok(Value::Scalar(v)) if v.bits()==if g==0 {7183}else{29})
                        );
                    }
                }
            }
            drop(aliases);
            p.wait(
                "record_release",
                if mode == CohortMode::Early { 4 } else { 2 },
            );
            assert!(operations.is_baseline());
            let release = Arc::clone(&p);
            let tail = std::thread::spawn(move || {
                release.wait("owner_dispose", 1);
                release.release_for("producer_release", 100);
            });
            drop(operations);
            p.record("owner_dispose", 0, 0, 0);
            tail.join().unwrap();
            for producer in producers {
                producer.join().unwrap();
            }
            drop(storage);
            let events = p.events();
            let r = replay_combination(Binding::RustChannel, mode, controlled, &events, 100_000)
                .unwrap_or_else(|e| panic!("{e}\n{}", events.join("\n")));
            let rejected =
                adversarial::reject_corruptions(Binding::RustChannel, mode, controlled, &events);
            println!(
                "COMBINATION Rust {mode:?} controlled={controlled} events={} frontier={} rejected={rejected}",
                r.events, r.maximum_frontier
            );
            owner.borrow_mut().finish().unwrap();
        }
    }
}
