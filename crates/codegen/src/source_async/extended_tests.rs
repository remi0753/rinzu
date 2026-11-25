use super::cohort_tests::{
    perform_call_in_slot, publish, receive, retire, start, start_completion,
};
use super::*;
use rinzu_runtime::lifetime_trace::Probe;
use rinzu_runtime::native_completion::{
    CompletionPointer, NativeCompletion, NotifiedNativeCompletion, TransferNativeCompletion,
};
use rinzu_runtime_model::native_lifetime::{Binding, CohortMode, replay_extended};
mod adversarial {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/support/combination.rs"
    ));
}

#[repr(C, align(8))]
pub(super) struct PollingStorage {
    prefix: u32,
    completion: NativeCompletion,
}

impl PollingStorage {
    pub(super) fn publish(&self, probe: &Probe) {
        probe.record("poll_publish", 200, 0, 0);
        self.completion
            .done
            .store(1, std::sync::atomic::Ordering::Release);
    }
}

pub(super) fn arm_poll(
    module: &Arc<IrModule>,
    owner: &Rc<RefCell<OwnerValues>>,
    shared: &Arc<Mutex<SharedState>>,
    ops: &mut SourceOperations,
    p: &Arc<Probe>,
) -> (Box<PollingStorage>, u64) {
    arm_poll_in_slot(module, owner, shared, ops, p, false)
}

pub(super) fn arm_poll_in_slot(
    module: &Arc<IrModule>,
    owner: &Rc<RefCell<OwnerValues>>,
    shared: &Arc<Mutex<SharedState>>,
    ops: &mut SourceOperations,
    p: &Arc<Probe>,
    mixed: bool,
) -> (Box<PollingStorage>, u64) {
    let sdk = Box::new(PollingStorage {
        prefix: 0,
        completion: NativeCompletion {
            version: 1,
            size: 16,
            done: std::sync::atomic::AtomicU32::new(0),
            cancelled: std::sync::atomic::AtomicU32::new(0),
        },
    });
    let raw = std::ptr::from_ref(&sdk.completion) as usize;
    assert_eq!(raw % 8, 4);
    p.observe_carrier(raw, 200);
    perform_call_in_slot(module, owner, p, raw, (100, if mixed { 4 } else { 2 }));
    if mixed {
        p.record(
            "carrier_abi",
            200,
            i64::from(sdk.completion.version),
            i64::from(sdk.completion.size),
        );
    }
    // SAFETY: this SDK-owned v1 storage remains pinned through physical copy and control closure.
    let completion = unsafe { CompletionPointer::new(raw) };
    let f = start_completion(shared, ops, p, completion, (200, 0), false);
    p.record("poll_bind", 200, 1, 0);
    (sdk, f)
}

fn poll_call(
    module: &Arc<IrModule>,
    owner: &Rc<RefCell<OwnerValues>>,
    shared: &Arc<Mutex<SharedState>>,
    ops: &mut SourceOperations,
    p: &Arc<Probe>,
) {
    let (sdk, f) = arm_poll(module, owner, shared, ops, p);
    sdk.publish(p);
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !shared.lock().unwrap().completed.contains_key(&f) {
        assert!(std::time::Instant::now() < deadline);
        ops.receive_one_ready().unwrap();
        std::thread::yield_now();
    }
    assert!(matches!(
        shared.lock().unwrap().completed[&f],
        Ok(Value::Unit)
    ));
    let op = p
        .events()
        .iter()
        .find_map(|s| {
            let a: Vec<_> = s.split(',').collect();
            (a[0] == "receipt" && a[2] == "200").then(|| a[1].parse::<u64>().unwrap())
        })
        .unwrap();
    p.wait_for("record_release", op, 1);
    p.record("close_admission", 200, 0, 0);
    drop(sdk);
    p.record("carrier_free", 200, 0, 0);
    p.record("sdk_pin_free", 200, 0, 0);
}
fn callback_publish(p: &Arc<Probe>, raw: usize) -> std::thread::JoinHandle<()> {
    p.block_for("callback_begin", 101);
    let p = Arc::clone(p);
    std::thread::spawn(move || {
        p.wait_for("callback_begin", 101, 1);
        let _scope = Probe::producer(101);
        p.record("publish_begin", 101, 0, 0);
        // SAFETY: the started SDK owns the initialized carrier, and no drain has yet registered.
        unsafe {
            NotifiedNativeCompletion::publish_with_lifetime_trace(
                raw as *mut NotifiedNativeCompletion,
                &p,
            );
        }
        p.record("publish_end", 101, 0, 0);
        p.release_for("callback_begin", 101);
    })
}
#[test]
fn reuse_combines_live_callback_publication_split_receipts_and_rust_polling() {
    let module = Arc::new(
        crate::test_library::compile_library(&rinzu_compiler::SourceFile::new(
            rinzu_compiler::FileId(885),
            "extended.rinzu",
            r#"
    native {fn start(g:Int,callback:fn()->CSize)->CSize symbol "cohort_start";}
    fn answer()->CSize fallback 0{return CSize(7183);}
    async fn main(){let raw=start(0,answer);}
    "#,
        ))
        .unwrap()
        .ir,
    );
    for mode in [CohortMode::Early, CohortMode::Deferred, CohortMode::Mixed] {
        for polling in [false, true] {
            for controlled in [false, true] {
                let p = Probe::new();
                let _scope = Probe::install(Arc::clone(&p));
                let owner = OwnerValues::new(&module).unwrap();
                let shared = Arc::new(Mutex::new(SharedState::default()));
                let capacity = match mode {
                    CohortMode::Early => 4,
                    CohortMode::Mixed => 3,
                    _ => 2,
                };
                let mut ops = SourceOperations::with_limits(
                    Arc::clone(&module),
                    Arc::clone(&shared),
                    &SourceAsyncOptions::default(),
                    &owner,
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    Arc::new(NativeBindings::default()),
                    rinzu_runtime::resources::ResourceLimits {
                        pending_operations: capacity,
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
                let first = start(&module, &owner, &shared, &mut ops, &p, raw0, 0);
                p.wait("registered", 2);
                let aliases: Vec<_> = first
                    .iter()
                    .map(|f| {
                        let ActiveOperation::NativeCompletion { result, .. } = &ops.active[f].1
                        else {
                            unreachable!()
                        };
                        result.clone()
                    })
                    .collect();
                if controlled {
                    assert!(ops.request_control(first[0], false));
                }
                let mut producers = Vec::new();
                if mode == CohortMode::Early {
                    producers.push(callback_publish(&p, raw1));
                }
                let second = start(&module, &owner, &shared, &mut ops, &p, raw1, 1);
                if mode == CohortMode::Early {
                    p.wait("registered", 4);
                    p.wait("worker_wake_end", 2);
                } else {
                    assert_eq!(
                        ops.deferred_native.len(),
                        if mode == CohortMode::Mixed { 1 } else { 2 }
                    );
                    if mode == CohortMode::Mixed {
                        p.wait("registered", 3);
                    }
                    if controlled {
                        assert!(ops.request_control(second[0], false));
                        assert!(ops.request_control(second[1], true));
                    }
                    producers.push(publish(&p, raw1, 1));
                    p.wait_for("sdk_exchange", 101, 1);
                }
                receive(&mut ops, &shared, second);
                let registered = match mode {
                    CohortMode::Early => 4,
                    CohortMode::Mixed => 3,
                    _ => 2,
                };
                assert_eq!(
                    p.events()
                        .iter()
                        .filter(|s| s.starts_with("receipt,"))
                        .count(),
                    registered
                );
                assert_eq!(ops.mailbox.resources().used.pending_operations, 2);
                retire(&p, 1);
                if polling && mode != CohortMode::Deferred {
                    poll_call(&module, &owner, &shared, &mut ops, &p);
                }
                producers.push(publish(&p, raw0, 0));
                p.wait_for("sdk_exchange", 100, 1);
                receive(&mut ops, &shared, first);
                retire(&p, 0);
                if polling && mode == CohortMode::Deferred {
                    poll_call(&module, &owner, &shared, &mut ops, &p);
                }
                *storage[0] = TransferNativeCompletion {
                    result: 65537,
                    ..Default::default()
                };
                let third = start(&module, &owner, &shared, &mut ops, &p, raw0, 2);
                p.wait("registered", registered + 2);
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
                assert_eq!(
                    storage[0]
                        .completion
                        .base
                        .cancelled
                        .load(std::sync::atomic::Ordering::Acquire),
                    0
                );
                p.record("generation_status", 102, 0, 0);
                drop(aliases);
                let last_operation = p
                    .events()
                    .iter()
                    .filter_map(|s| {
                        let a: Vec<_> = s.split(',').collect();
                        (a[0] == "receipt" && a[2] == "102").then(|| a[1].parse::<u64>().unwrap())
                    })
                    .next_back()
                    .unwrap();
                p.block_for("worker_wake_begin", last_operation);
                producers.push(publish(&p, raw0, 2));
                p.wait_for("sdk_exchange", 102, 1);
                receive(&mut ops, &shared, third);
                p.wait_for("worker_wake_begin", last_operation, 1);
                retire(&p, 2);
                // Each SDK thread has exchanged its last carrier access for a target reference.
                // Free the actual SDK allocations while the worker still owns its wake record.
                drop(storage);
                let record_retained = p
                    .events()
                    .iter()
                    .all(|s| !s.starts_with(&format!("record_release,{last_operation},")));
                p.release_for("worker_wake_begin", last_operation);
                assert!(record_retained);
                for (g, fs) in [(0, first), (1, second), (2, third)] {
                    for (i, f) in fs.into_iter().enumerate() {
                        let state = shared.lock().unwrap();
                        let result = &state.completed[&f];
                        if controlled && (g == 0 && i == 0 || g == 1 && mode != CohortMode::Early) {
                            assert_eq!(
                                result.as_ref().unwrap_err().primary.cause,
                                FailureCause::Cancelled
                            );
                        } else {
                            assert!(
                                matches!(result,Ok(Value::Scalar(v)) if v.bits()==[7183,29,65537][g])
                            );
                        }
                    }
                }
                p.wait("record_release", registered + 2 + usize::from(polling));
                assert!(ops.is_baseline());
                let release = Arc::clone(&p);
                let tail = std::thread::spawn(move || {
                    release.wait("owner_dispose", 1);
                    for id in [100, 101, 102] {
                        release.release_for("producer_release", id);
                        release.wait_for("publish_end", id, 1);
                    }
                });
                drop(ops);
                p.record("owner_dispose", 0, 0, 0);
                tail.join().unwrap();
                for thread in producers {
                    thread.join().unwrap();
                }
                let events = p.events();
                let r = replay_extended(
                    Binding::RustChannel,
                    mode,
                    controlled,
                    polling,
                    &events,
                    100_000,
                )
                .unwrap_or_else(|e| panic!("{e}\n{}", events.join("\n")));
                let rejected = adversarial::reject_extended(
                    Binding::RustChannel,
                    mode,
                    controlled,
                    polling,
                    &events,
                );
                println!(
                    "EXTENDED Rust {mode:?} poll={polling} control={controlled} events={} frontier={} rejected={rejected}",
                    r.events, r.maximum_frontier
                );
                owner.borrow_mut().finish().unwrap();
            }
        }
    }
}
