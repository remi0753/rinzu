use super::*;
use rinzu_runtime::lifetime_trace::Probe;
struct SdkCall {
    probe: Arc<Probe>,
    pointer: usize,
    generation: usize,
    slot: i64,
}
thread_local! {static SDK: RefCell<Option<SdkCall>>=const{RefCell::new(None)};}
unsafe extern "C" fn start(g: i64, callback: unsafe extern "C" fn() -> u64) -> usize {
    let (p, raw, expected, slot) = SDK.with(|s| {
        let s = s.borrow();
        let state = s.as_ref().unwrap();
        (
            Arc::clone(&state.probe),
            state.pointer,
            state.generation,
            state.slot,
        )
    });
    assert_eq!(usize::try_from(g).unwrap(), expected);
    let id = 100 + u64::try_from(expected).unwrap();
    p.record("sdk_start", id, 0, 0);
    p.record("generation", id, i64::try_from(raw).unwrap(), slot);
    // SAFETY: NativeCall supplies exactly this opened callback ABI and live environment.
    assert_eq!(unsafe { callback() }, 7183);
    raw
}
pub(in crate::source_async) fn perform(
    module: &Arc<IrModule>,
    owner: &Rc<RefCell<OwnerValues>>,
    raw: usize,
    (g, slot): (usize, i64),
    p: &Arc<Probe>,
) {
    let _scope = Probe::call(100 + u64::try_from(g).unwrap());
    let signature = module
        .source_native_signatures
        .iter()
        .find(|s| s.name == "start")
        .unwrap()
        .clone();
    let (call_id, plans) = module
        .functions
        .iter()
        .flat_map(|f| &f.blocks)
        .flat_map(|b| &b.operations)
        .find_map(|o| {
            if let OperationKind::PrepareNativeCall {
                call, callbacks, ..
            } = &o.kind
            {
                Some((*call, callbacks.clone()))
            } else {
                None
            }
        })
        .unwrap();
    let engine = NativeEngine::new(Arc::new(NativeBindings {
        module: Some(Arc::clone(module)),
        entries: BTreeMap::from([(
            signature.name.clone(),
            (signature.clone(), start as *const () as usize),
        )]),
        ..NativeBindings::default()
    }))
    .unwrap();
    SDK.with(|s| {
        *s.borrow_mut() = Some(SdkCall {
            probe: Arc::clone(p),
            pointer: raw,
            generation: g,
            slot,
        })
    });
    let cancellation = CancellationState::default();
    let diagnostic = Diagnostic {
        cause: FailureCause::Panic,
        source: None,
    };
    let mut call = NativeCall::prepare(
        &signature,
        &plans,
        vec![
            Value::int(i64::try_from(g).unwrap()),
            Value::FunctionReference("answer".into()),
        ],
        owner,
        &engine,
        diagnostic,
        CallbackOrigin {
            cancellation: &cancellation,
            main: None,
        },
    )
    .unwrap();
    call.enter(&engine).unwrap();
    call.close().unwrap();
    call.settle().unwrap();
    let value = call.read(diagnostic).unwrap();
    assert!(matches!(value,Value::Scalar(v) if v.bits() == u64::try_from(raw).unwrap()));
    Probe::emit("handoff", 0);
    let function = module.functions.iter().find(|f| f.name == "main").unwrap();
    let mut frame = Frame::new(module, 0, function, vec![]).with_owner(owner);
    frame.native_calls.insert(call_id, call);
    frame.release_native(call_id).unwrap();
    drop(frame);
    SDK.with(|s| s.borrow_mut().take());
    let e = engine.bindings.evidence();
    assert!(e.is_baseline());
    assert_eq!(e.callback_entries, 1);
    assert_eq!(e.released, 1);
}
