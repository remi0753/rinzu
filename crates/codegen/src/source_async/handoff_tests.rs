use super::*;
use rinzu_runtime::lifetime_trace::Probe;
use rinzu_runtime::native_completion::{
    CompletionPointer, NotifiedNativeCompletion, TransferNativeCompletion,
};
use rinzu_runtime_model::native_lifetime::{Binding, replay_handoff};

pub(in crate::source_async) fn publish(carrier: &TransferNativeCompletion, probe: &Probe) {
    probe.record("publish_begin", 0, 0, 0);
    // SAFETY: the caller pins a uniquely published v3 carrier with immutable payload.
    unsafe {
        NotifiedNativeCompletion::publish_with_lifetime_trace(
            std::ptr::from_ref(&carrier.completion).cast_mut(),
            probe,
        );
    }
    probe.record("publish_end", 0, 0, 0);
}
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn perform_runtime_call(probe: &Arc<Probe>, carrier: &Arc<TransferNativeCompletion>, early: bool) {
    use rinzu_runtime::callback_entry::{CallbackFrame, FrameCallback};
    struct Context {
        probe: Arc<Probe>,
        magic: u64,
    }
    impl Drop for Context {
        fn drop(&mut self) {
            assert_eq!(self.magic, 7183);
            self.probe.record("call_env_free", 0, 0, 0);
        }
    }
    unsafe extern "C" fn body(raw: *mut core::ffi::c_void, _: *const CallbackFrame) -> u64 {
        // SAFETY: the caller pins this initialized Context through gate settlement.
        let context = unsafe { &*raw.cast::<Context>() };
        assert_eq!(context.magic, 7183);
        for kind in ["callback_admit", "callback_begin", "callback_return"] {
            context.probe.record(kind, 0, 0, 0);
        }
        context.magic
    }
    let mut context = Box::new(Context {
        probe: Arc::clone(probe),
        magic: 7183,
    });
    // SAFETY: the exact synchronous callback never escapes this opened gate.
    let callback =
        unsafe { FrameCallback::prepare_deferred(body, std::ptr::from_mut(&mut *context).cast()) };
    callback.open();
    probe.record("call_enter", 0, 0, 0);
    probe.record("sdk_start", 0, 0, 0);
    // SAFETY: the prepared entry has exactly the u64(void) physical signature.
    let entry: unsafe extern "C" fn() -> u64 = unsafe { std::mem::transmute(callback.entry()) };
    // SAFETY: the opened gate and exact entry signature remain live above.
    assert_eq!(unsafe { entry() }, 7183);
    if early {
        publish(carrier, probe);
    }
    probe.record("call_return", 0, 0, 0);
    assert!(callback.close());
    probe.record("call_close", 0, 0, 0);
    assert!(callback.settled());
    probe.record("call_settle", 0, 0, 0);
    probe.record("call_read", 0, 0, 0);
    probe.record("handoff", 0, 0, 0);
    drop(callback);
    drop(context);
}
mod adversarial {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/support/handoff.rs"
    ));
}
fn check(full: bool, events: &[String]) {
    let r = replay_handoff(Binding::RustChannel, full, 7183, events, 100_000)
        .unwrap_or_else(|e| panic!("{e}\n{}", events.join("\n")));
    let rejected = adversarial::reject_corruptions(Binding::RustChannel, full, events);
    println!(
        "HANDOFF Rust full={full} events={} frontier={} rejected={rejected}",
        r.events, r.maximum_frontier
    );
}
#[test]
fn call_handoff_preserves_drain_decisions_and_wake_lifetime() {
    let module = Arc::new(
        crate::test_library::compile_library(&rinzu_compiler::SourceFile::new(
            rinzu_compiler::FileId(882),
            "handoff.rinzu",
            r#"
    native {fn start(callback:fn()->CSize)->CSize symbol "handoff_start";}
    fn answer()->CSize fallback 0{return CSize(7183);}
    async fn main(){let pointer=start(answer);}
    "#,
        ))
        .unwrap()
        .ir,
    );
    for (full, retry, early, concurrent) in [
        (false, false, true, false),
        (false, false, false, false),
        (true, false, true, false),
        (true, false, false, false),
        (true, true, false, false),
        (true, false, false, true),
    ] {
        for control in [0, 1, 2] {
            if concurrent && control == 0 {
                continue;
            }
            let probe = Probe::new();
            let _scope = Probe::install(Arc::clone(&probe));
            let owner = OwnerValues::new(&module).unwrap();
            let shared = Arc::new(Mutex::new(SharedState::default()));
            let mut operations = SourceOperations::new(
                Arc::clone(&module),
                Arc::clone(&shared),
                &SourceAsyncOptions::default(),
                &owner,
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                Arc::new(NativeBindings::default()),
            );
            if full {
                operations.mailbox = OperationMailbox::new(
                    rinzu_runtime::resources::ResourceLimits {
                        pending_operations: 0,
                        ..Default::default()
                    },
                    Arc::new(|| {}),
                );
            }
            let carrier = Arc::new(TransferNativeCompletion {
                result: 7183,
                ..Default::default()
            });
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            super::native::handoff_tests::perform(&module, &owner, &carrier, early);
            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            perform_runtime_call(&probe, &carrier, early);
            // SAFETY: the SDK fixture's Arc pins storage through every drain and wake.
            let pointer = unsafe { CompletionPointer::new(Arc::as_ptr(&carrier) as usize) };
            let Value::Future(future) = shared
                .lock()
                .unwrap()
                .future(
                    0,
                    PendingRequest::NativeCompletion {
                        completion: pointer,
                        transfer: true,
                        source: None,
                    },
                )
                .unwrap()
            else {
                unreachable!()
            };
            operations.start(future).unwrap();
            if !full {
                probe.wait("registered", 1);
            }
            let producer = if concurrent {
                use std::sync::atomic::Ordering;
                probe.block("deferred_flag");
                let carrier = Arc::clone(&carrier);
                let probe = Arc::clone(&probe);
                Some(std::thread::spawn(move || {
                    while carrier.completion.base.cancelled.load(Ordering::Acquire) == 0 {
                        std::thread::yield_now();
                    }
                    publish(&carrier, &probe);
                    probe.release("deferred_flag");
                }))
            } else {
                None
            };
            let accepted = control != 0 && operations.request_control(future, control == 2);
            if let Some(producer) = producer {
                producer.join().unwrap();
                assert!(pointer.is_done());
            }
            if full {
                assert_eq!(accepted, !early && control != 0);
            }
            if retry {
                probe.record("credit", 0, 0, 0);
                operations.mailbox = OperationMailbox::new(Default::default(), Arc::new(|| {}));
                assert!(!operations.receive_one_ready().unwrap());
                probe.wait("registered", 1);
            }
            if !early && !concurrent {
                publish(&carrier, &probe);
            }
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while !shared.lock().unwrap().completed.contains_key(&future) {
                assert!(std::time::Instant::now() < deadline);
                operations.receive_one_ready().unwrap();
                std::thread::yield_now();
            }
            if accepted {
                assert_eq!(
                    shared.lock().unwrap().completed[&future]
                        .as_ref()
                        .unwrap_err()
                        .primary
                        .cause,
                    FailureCause::Cancelled
                );
            } else {
                assert!(
                    matches!(&shared.lock().unwrap().completed[&future],Ok(Value::Scalar(v)) if v.bits()==7183)
                );
            }
            assert!(operations.is_baseline());
            if !full || retry {
                probe.wait("record_release", 1);
                probe.record("close_admission", 0, 0, 0);
                probe.record("carrier_free", 0, 0, 0);
            }
            drop(carrier);
            probe.record("sdk_pin_free", 0, 0, 0);
            drop(operations);
            probe.record("owner_dispose", 0, 0, 0);
            check(full, &probe.events());
            owner.borrow_mut().finish().unwrap();
        }
    }
}
