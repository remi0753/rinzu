use super::*;
use rinzu_runtime::lifetime_trace::Probe;
use rinzu_runtime::native_completion::TransferNativeCompletion;
thread_local! {static SDK: RefCell<Option<(Arc<TransferNativeCompletion>,bool)>>=const{RefCell::new(None)};}
unsafe extern "C" fn start(callback: unsafe extern "C" fn() -> u64) -> usize {
    let (carrier, early) = SDK.with(|s| s.borrow().as_ref().unwrap().clone());
    Probe::emit("sdk_start", 0);
    // SAFETY: NativeCall supplies the exact live, opened callback signature.
    assert_eq!(unsafe { callback() }, 7183);
    if early {
        super::super::operations::handoff_tests::publish(&carrier, &Probe::current().unwrap());
    }
    Arc::as_ptr(&carrier) as usize
}
pub(in crate::source_async) fn perform(
    module: &Arc<IrModule>,
    owner: &Rc<RefCell<OwnerValues>>,
    carrier: &Arc<TransferNativeCompletion>,
    early: bool,
) {
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
    SDK.with(|s| *s.borrow_mut() = Some((Arc::clone(carrier), early)));
    let cancellation = CancellationState::default();
    let diagnostic = Diagnostic {
        cause: FailureCause::Panic,
        source: None,
    };
    let mut call = NativeCall::prepare(
        &signature,
        &plans,
        vec![Value::FunctionReference("answer".into())],
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
    assert!(matches!(value,Value::Scalar(v) if v.bits() == Arc::as_ptr(carrier) as u64));
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
