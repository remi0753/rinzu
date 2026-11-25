use super::*;
use rinzu_runtime::lifetime_trace::Probe;
use rinzu_runtime::native_completion::{
    CompletionPointer, NotifiedNativeCompletion, TransferNativeCompletion,
};
use rinzu_runtime_model::native_lifetime::{Binding, CohortLayout, replay_cohort};
mod adversarial {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/support/cohort.rs"
    ));
}
const PAYLOAD: [i64; 3] = [7183, 29, 65537];
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn perform(probe: &Arc<Probe>, raw: usize, g: usize, slot: i64) {
    use rinzu_runtime::callback_entry::{CallbackFrame, FrameCallback};
    struct Context {
        probe: Arc<Probe>,
        id: u64,
        magic: u64,
    }
    impl Drop for Context {
        fn drop(&mut self) {
            assert_eq!(self.magic, 7183);
            self.probe.record("call_env_free", self.id, 0, 0);
        }
    }
    unsafe extern "C" fn body(raw: *mut core::ffi::c_void, _: *const CallbackFrame) -> u64 {
        // SAFETY: the opened gate pins this fully initialized context until settlement.
        let c = unsafe { &*raw.cast::<Context>() };
        assert_eq!(c.magic, 7183);
        for kind in ["callback_admit", "callback_begin", "callback_return"] {
            c.probe.record(kind, c.id, 0, 0);
        }
        c.magic
    }
    let id = 100 + u64::try_from(g).unwrap();
    let mut context = Box::new(Context {
        probe: Arc::clone(probe),
        id,
        magic: 7183,
    });
    // SAFETY: this synchronous callback has the exact physical frame-entry signature.
    let gate =
        unsafe { FrameCallback::prepare_deferred(body, std::ptr::from_mut(&mut *context).cast()) };
    gate.open();
    probe.record("call_enter", id, 0, 0);
    probe.record("sdk_start", id, 0, 0);
    probe.record("generation", id, i64::try_from(raw).unwrap(), slot);
    // SAFETY: this opened generated entry has precisely u64(void) ABI.
    let entry: unsafe extern "C" fn() -> u64 = unsafe { std::mem::transmute(gate.entry()) };
    // SAFETY: the gate and its initialized Context remain pinned.
    assert_eq!(unsafe { entry() }, 7183);
    probe.record("call_return", id, 0, 0);
    assert!(gate.close());
    probe.record("call_close", id, 0, 0);
    assert!(gate.settled());
    probe.record("call_settle", id, 0, 0);
    probe.record("call_read", id, 0, 0);
    probe.record("handoff", id, 0, 0);
    drop(gate);
    drop(context);
}
pub(super) fn receive<const N: usize>(
    operations: &mut SourceOperations,
    shared: &Arc<Mutex<SharedState>>,
    futures: [u64; N],
) {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while !futures
        .iter()
        .all(|f| shared.lock().unwrap().completed.contains_key(f))
    {
        assert!(std::time::Instant::now() < deadline);
        operations.receive_one_ready().unwrap();
        std::thread::yield_now();
    }
}
pub(super) fn retire(p: &Probe, g: usize) {
    for kind in ["close_admission", "carrier_free", "sdk_pin_free"] {
        p.record(kind, 100 + u64::try_from(g).unwrap(), 0, 0);
    }
}
pub(super) fn start(
    module: &Arc<IrModule>,
    owner: &Rc<RefCell<OwnerValues>>,
    shared: &Arc<Mutex<SharedState>>,
    operations: &mut SourceOperations,
    p: &Arc<Probe>,
    raw: usize,
    g: usize,
) -> [u64; 2] {
    start_mode(module, owner, shared, operations, p, raw, g, false)
}
#[allow(
    clippy::too_many_arguments,
    reason = "Test identifies the real Call, Owner and SDK generation"
)]
pub(super) fn start_mode(
    module: &Arc<IrModule>,
    owner: &Rc<RefCell<OwnerValues>>,
    shared: &Arc<Mutex<SharedState>>,
    operations: &mut SourceOperations,
    p: &Arc<Probe>,
    raw: usize,
    g: usize,
    early: bool,
) -> [u64; 2] {
    let id = 100 + u64::try_from(g).unwrap();
    p.observe_carrier(raw, id);
    perform_call(module, owner, p, raw, g);
    if early {
        let _producer = Probe::producer(id);
        p.record("publish_begin", id, 0, 0);
        // SAFETY: before admission there is no target reference; the SDK owns this
        // initialized generation until its final release exchange.
        unsafe {
            NotifiedNativeCompletion::publish_with_lifetime_trace(
                raw as *mut NotifiedNativeCompletion,
                p,
            );
        }
        p.record("publish_end", id, 0, 0);
    }
    std::array::from_fn(|i| {
        // SAFETY: SDK storage remains pinned through every receipt's physical copy.
        let completion = unsafe { CompletionPointer::new(raw) };
        start_completion(shared, operations, p, completion, (id, i), true)
    })
}
pub(super) fn start_completion(
    shared: &Arc<Mutex<SharedState>>,
    operations: &mut SourceOperations,
    probe: &Probe,
    completion: CompletionPointer,
    (generation, index): (u64, usize),
    transfer: bool,
) -> u64 {
    let Value::Future(future) = shared
        .lock()
        .unwrap()
        .future(
            0,
            PendingRequest::NativeCompletion {
                completion,
                transfer,
                source: None,
            },
        )
        .unwrap()
    else {
        unreachable!()
    };
    probe.bind_future(future, generation, index);
    operations.start(future).unwrap();
    future
}
pub(super) fn publish(p: &Arc<Probe>, raw: usize, g: usize) -> std::thread::JoinHandle<()> {
    let p = Arc::clone(p);
    let id = 100 + u64::try_from(g).unwrap();
    p.block_for("producer_release", id);
    std::thread::spawn(move || {
        let _scope = Probe::producer(id);
        p.record("publish_begin", id, 0, 0);
        // SAFETY: this unique SDK producer owns the initialized generation until final
        // exchange; thereafter publication touches only its independently retained target.
        unsafe {
            NotifiedNativeCompletion::publish_with_lifetime_trace(
                raw as *mut NotifiedNativeCompletion,
                &p,
            );
        }
        p.record("publish_end", id, 0, 0);
    })
}
#[test]
fn multiple_calls_and_receipts_preserve_shared_target_and_old_generation() {
    let module = Arc::new(
        crate::test_library::compile_library(&rinzu_compiler::SourceFile::new(
            rinzu_compiler::FileId(883),
            "cohort.rinzu",
            r#"
 native {fn start(g:Int,callback:fn()->CSize)->CSize symbol "cohort_start";}
 fn answer()->CSize fallback 0{return CSize(7183);}
 async fn main(){let pointer=start(0,answer);}
 "#,
        ))
        .unwrap()
        .ir,
    );
    for layout in [CohortLayout::Carriers, CohortLayout::Reuse] {
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
                    pending_operations: 2,
                    ..Default::default()
                },
            );
            let mut storage =
                std::array::from_fn::<_, 2, _>(|_| Box::new(TransferNativeCompletion::default()));
            storage[0].result = PAYLOAD[0];
            storage[1].result = PAYLOAD[1];
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
            let second = start(&module, &owner, &shared, &mut operations, &p, raw1, 1);
            assert_eq!(operations.deferred_native.len(), 2);
            if controlled {
                assert!(operations.request_control(second[1], true));
            }
            let mut producers = vec![publish(&p, raw0, 0)];
            p.wait_for("sdk_exchange", 100, 1);
            receive(&mut operations, &shared, first);
            retire(&p, 0);
            let deadline = std::time::Instant::now() + Duration::from_secs(15);
            while !operations.deferred_native.is_empty() {
                assert!(std::time::Instant::now() < deadline);
                operations.receive_one_ready().unwrap();
                std::thread::yield_now();
            }
            p.wait("registered", 4);
            let third = if layout == CohortLayout::Reuse {
                // Every old receipt has copied and control acquisition has closed. The paused
                // old publisher no longer dereferences this storage, only its retained target.
                *storage[0] = TransferNativeCompletion {
                    result: PAYLOAD[2],
                    ..Default::default()
                };
                Some(start(
                    &module,
                    &owner,
                    &shared,
                    &mut operations,
                    &p,
                    raw0,
                    2,
                ))
            } else {
                None
            };
            assert_ne!(
                aliases[0].request_cancel(),
                rinzu_runtime::scheduler::ResolveResult::Won
            );
            assert_ne!(
                aliases[1].request_timeout(),
                rinzu_runtime::scheduler::ResolveResult::Won
            );
            assert_eq!(aliases[1].native_transfer_result(), Some(PAYLOAD[0]));
            p.record("alias_result", 100, PAYLOAD[0], 1);
            if third.is_some() {
                assert_eq!(
                    storage[0]
                        .completion
                        .base
                        .cancelled
                        .load(std::sync::atomic::Ordering::Acquire),
                    0
                );
                p.record("generation_status", 102, 0, 0);
            }
            producers.push(publish(&p, raw1, 1));
            p.wait_for("sdk_exchange", 101, 1);
            receive(&mut operations, &shared, second);
            p.wait("record_release", 2);
            retire(&p, 1);
            if let Some(third) = third {
                while !operations.deferred_native.is_empty() {
                    operations.receive_one_ready().unwrap();
                    std::thread::yield_now();
                }
                p.wait("registered", 6);
                producers.push(publish(&p, raw0, 2));
                p.wait_for("sdk_exchange", 102, 1);
                receive(&mut operations, &shared, third);
                p.wait("record_release", 4);
                retire(&p, 2);
            }
            for (g, fs) in [(0, first), (1, second)] {
                for (i, f) in fs.into_iter().enumerate() {
                    let state = shared.lock().unwrap();
                    let result = &state.completed[&f];
                    if controlled && ((g == 0 && i == 0) || (g == 1 && i == 1)) {
                        assert_eq!(
                            result.as_ref().unwrap_err().primary.cause,
                            FailureCause::Cancelled
                        );
                    } else {
                        assert!(
                            matches!(result,Ok(Value::Scalar(v)) if v.bits()==PAYLOAD[g] as u64)
                        );
                    }
                }
            }
            drop(aliases);
            p.wait("record_release", layout.count() * 2);
            assert!(operations.is_baseline());
            let release = Arc::clone(&p);
            let count = layout.count();
            let tail = std::thread::spawn(move || {
                release.wait("owner_dispose", 1);
                for g in 0..count {
                    release.release_for("producer_release", 100 + u64::try_from(g).unwrap());
                }
            });
            drop(operations);
            p.record("owner_dispose", 0, 0, 0);
            tail.join().unwrap();
            for producer in producers {
                producer.join().unwrap();
            }
            drop(storage);
            let events = p.events();
            let r = replay_cohort(Binding::RustChannel, layout, controlled, &events, 100_000)
                .unwrap_or_else(|e| panic!("{e}\n{}", events.join("\n")));
            let rejected =
                adversarial::reject_corruptions(Binding::RustChannel, layout, controlled, &events);
            println!(
                "COHORT Rust {layout:?} controlled={controlled} events={} frontier={} rejected={rejected}",
                r.events, r.maximum_frontier
            );
            owner.borrow_mut().finish().unwrap();
        }
    }
}

pub(super) fn perform_call(
    module: &Arc<IrModule>,
    owner: &Rc<RefCell<OwnerValues>>,
    p: &Arc<Probe>,
    raw: usize,
    g: usize,
) {
    perform_call_in_slot(
        module,
        owner,
        p,
        raw,
        (g, if g == 100 { 2 } else { i64::from(g == 1) }),
    );
}

pub(super) fn perform_call_in_slot(
    module: &Arc<IrModule>,
    owner: &Rc<RefCell<OwnerValues>>,
    p: &Arc<Probe>,
    raw: usize,
    (g, slot): (usize, i64),
) {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    super::super::native::cohort_tests::perform(module, owner, raw, (g, slot), p);
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = (module, owner);
        perform(p, raw, g, slot);
    }
}
