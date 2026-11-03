//! Source execution's balanced ordinary native call ledger.
use super::data::native_fatal;
use super::*;
mod supplant;
use crate::source_native_abi::NativeAbiStubs;
use rinzu_compiler::{CallbackArgumentPlan, NativeCallId, SourceAbiType, SourceNativeSignature};
use rinzu_runtime::callback_entry::{
    CallbackAbiCursor, CallbackFrame, FrameCallback, PointerU64Callback,
};
use rinzu_runtime::data::{
    NativeRecordError, NativeRecordLayout, NativeRecordLoan, NativeRecordLoanError,
    NativeUserDataInvocation, NativeUserDataScope,
};
use std::sync::atomic::{AtomicUsize, Ordering};
pub(super) use supplant::Supplants;

#[derive(Debug, Default)]
struct Counters {
    prepared: AtomicUsize,
    returned: AtomicUsize,
    closed: AtomicUsize,
    settled: AtomicUsize,
    read: AtomicUsize,
    released: AtomicUsize,
    abandoned: AtomicUsize,
    callback_entries: AtomicUsize,
    copied_bytes: AtomicUsize,
    live_calls: AtomicUsize,
    live_buffers: AtomicUsize,
    live_callbacks: AtomicUsize,
    normal_returns: AtomicUsize,
    fallback_returns: AtomicUsize,
    return_bytes: AtomicUsize,
    return_metadata: AtomicUsize,
}
impl Counters {
    fn evidence(&self) -> SourceNativeCallEvidence {
        SourceNativeCallEvidence {
            prepared: self.prepared.load(Ordering::Relaxed),
            returned: self.returned.load(Ordering::Relaxed),
            closed: self.closed.load(Ordering::Relaxed),
            settled: self.settled.load(Ordering::Relaxed),
            read: self.read.load(Ordering::Relaxed),
            released: self.released.load(Ordering::Relaxed),
            abandoned: self.abandoned.load(Ordering::Relaxed),
            callback_entries: self.callback_entries.load(Ordering::Relaxed),
            copied_bytes: self.copied_bytes.load(Ordering::Relaxed),
            live_calls: self.live_calls.load(Ordering::Relaxed),
            live_buffers: self.live_buffers.load(Ordering::Relaxed),
            live_callbacks: self.live_callbacks.load(Ordering::Relaxed),
            callback_returns: CallbackReturnEvidence {
                normal: self.normal_returns.load(Ordering::Relaxed),
                fallback: self.fallback_returns.load(Ordering::Relaxed),
                bytes: self.return_bytes.load(Ordering::Relaxed),
                metadata_bytes: self.return_metadata.load(Ordering::Relaxed),
            },
            foreign: ForeignInvocationEvidence::default(),
        }
    }
    fn returned_buffer(
        &self,
        buffer: &rinzu_runtime::callback_return::ProducedBuffer,
        fallback: bool,
    ) {
        (if fallback {
            &self.fallback_returns
        } else {
            &self.normal_returns
        })
        .fetch_add(1, Ordering::Relaxed);
        self.return_bytes
            .fetch_add(buffer.storage_bytes(), Ordering::Relaxed);
        self.return_metadata
            .fetch_add(buffer.metadata_bytes(), Ordering::Relaxed);
    }
}

#[derive(Debug, Default)]
pub(super) struct NativeBindings {
    module: Option<Arc<IrModule>>,
    entries: BTreeMap<String, (SourceNativeSignature, usize)>,
    records: BTreeMap<String, Arc<NativeRecordLayout>>,
    counters: Arc<Counters>,
    consumers: std::collections::BTreeSet<String>,
}
impl NativeBindings {
    pub fn new(
        module: &IrModule,
        packages: Option<&crate::source_package::LoadedSourcePackages>,
    ) -> Result<Self> {
        let entries = match packages {
            Some(packages) => packages.source_bindings(module)?,
            None => BTreeMap::new(),
        };
        let records = packages
            .map(|packages| crate::source_native_record_layouts(module, packages.descriptors()))
            .transpose()?
            .unwrap_or_default()
            .into_iter()
            .map(|(name, layout)| (name, Arc::new(layout)))
            .collect();
        Ok(Self {
            module: Some(Arc::new(module.clone())),
            entries,
            records,
            counters: Arc::default(),
            consumers: packages
                .map(|packages| packages.return_consumers(&module.source_native_signatures))
                .transpose()?
                .unwrap_or_default(),
        })
    }
    pub fn evidence(&self) -> SourceNativeCallEvidence {
        self.counters.evidence()
    }
}

#[derive(Clone)]
pub(super) struct NativeEngine {
    pub bindings: Arc<NativeBindings>,
    stubs: Option<Rc<NativeAbiStubs>>,
}
impl NativeEngine {
    pub fn new(bindings: Arc<NativeBindings>) -> Result<Self> {
        let stubs = if bindings.entries.is_empty() {
            None
        } else {
            Some(Rc::new(NativeAbiStubs::new(
                &bindings
                    .entries
                    .values()
                    .map(|(signature, _)| signature.clone())
                    .collect::<Vec<_>>(),
            )?))
        };
        Ok(Self { bindings, stubs })
    }
}

struct LengthContext {
    owner: Rc<RefCell<OwnerValues>>,
    kind: rinzu_runtime::callback_entry::LengthKind,
    diagnostic_source: Option<SourcePosition>,
    counters: Arc<Counters>,
}
struct LengthEntry {
    entry: PointerU64Callback,
    _context: Box<LengthContext>,
}
impl LengthEntry {
    fn new(
        owner: &Rc<RefCell<OwnerValues>>,
        target: &str,
        expected: &SourceAbiType,
        counters: &Arc<Counters>,
    ) -> Result<Self> {
        let Some((kind, length)) = crate::function_reference::core_length(target) else {
            bail!("managed/native callback target requires the R4 body backend: {target}")
        };
        let kind = match kind {
            rinzu_compiler::CoreLengthKind::Bytes => {
                rinzu_runtime::callback_entry::LengthKind::Bytes(length.unwrap())
            }
            rinzu_compiler::CoreLengthKind::Utf8 => rinzu_runtime::callback_entry::LengthKind::Utf8,
        };
        let parameter = match kind {
            rinzu_runtime::callback_entry::LengthKind::Bytes(length) => {
                SourceAbiType::Bytes(length)
            }
            _ => SourceAbiType::String,
        };
        ensure!(
            expected
                == &SourceAbiType::Callback {
                    parameters: vec![parameter],
                    result: Box::new(SourceAbiType::Scalar(rinzu_compiler::AbiScalar::U64))
                },
            "length callback target disagrees with native signature"
        );
        let mut context = Box::new(LengthContext {
            owner: Rc::clone(owner),
            kind,
            diagnostic_source: None,
            counters: Arc::clone(counters),
        });
        // SAFETY: this context is pinned through close/settlement, and the entry
        // gate checks the physical Owner thread before touching its Rc/heap.
        let entry = unsafe {
            PointerU64Callback::prepare_deferred(
                length_handler,
                std::ptr::from_mut(&mut *context).cast(),
            )
        };
        Ok(Self {
            entry,
            _context: context,
        })
    }
}
impl Drop for LengthEntry {
    fn drop(&mut self) {
        if !self.entry.close() {
            native_fatal(86);
        }
    }
}
unsafe extern "C" fn length_handler(raw: *mut core::ffi::c_void, input: *const u8) -> u64 {
    // SAFETY: admission pins context and checked the Origin physical thread.
    let context = unsafe { &*raw.cast::<LengthContext>() };
    let _transition = context.owner.borrow().callback_scope();
    let _activity = context.owner.borrow().prepare_activity(
        rinzu_runtime::native_diagnostics::ActivityKind::Managed,
        match context.kind {
            rinzu_runtime::callback_entry::LengthKind::Utf8 => "rinzu.core::utf8ByteLength",
            _ => "rinzu.core::byteLength",
        },
        context.diagnostic_source,
    );
    _activity.start();
    // SAFETY: the selected ABI supplies exactly the fixed Bytes/String contract.
    let length = unsafe {
        context
            .owner
            .borrow_mut()
            .native_length(context.kind, input)
    }
    .unwrap_or_else(|_| native_fatal(86));
    context
        .counters
        .callback_entries
        .fetch_add(1, Ordering::Relaxed);
    context
        .counters
        .copied_bytes
        .fetch_add(length as usize, Ordering::Relaxed);
    rinzu_runtime::resource_accounting::record_copy(length);
    length
}

struct ManagedCallbackContext {
    owner: Rc<RefCell<OwnerValues>>,
    engine: NativeEngine,
    target: String,
    capture: Option<Value>,
    diagnostic_source: Option<SourcePosition>,
    parameters: Vec<SourceAbiType>,
    result: SourceAbiType,
    userdata: Option<NativeUserDataScope>,
    fallback: Option<u64>,
    fallback_storage_bytes: Option<usize>,
    return_scope: Option<Arc<rinzu_runtime::callback_return::ReturnScope>>,
    cancellation: CancellationState,
    main: Option<MainTaskContext>,
}
struct ManagedCallback {
    entry: FrameCallback,
    _context: Box<ManagedCallbackContext>,
}
enum CallbackEntry {
    Length(LengthEntry),
    Managed(ManagedCallback),
    Raw(usize),
}
impl CallbackEntry {
    fn bind_diagnostics(&self, owner: &OwnerValues, target: &str, source: Option<SourcePosition>) {
        let target = match self {
            Self::Length(callback) => match callback._context.kind {
                rinzu_runtime::callback_entry::LengthKind::Utf8 => "rinzu.core::utf8ByteLength",
                _ => "rinzu.core::byteLength",
            },
            _ => target,
        };
        let symbol = std::ffi::CString::new(target).unwrap_or_else(|_| native_fatal(86));
        let source = source.unwrap_or(SourcePosition {
            file: 0,
            start_byte: 0,
            end_byte: 0,
        });
        let (identity, depth) = owner.activity_metadata();
        // SAFETY: the call/capsule pins Origin's monitor through close and every
        // admitted receipt. C copies the name and source before Open.
        unsafe {
            match self {
                Self::Length(callback) => callback.entry.bind_diagnostics(
                    owner.diagnostic_monitor(),
                    &symbol,
                    source,
                    identity,
                    depth,
                ),
                Self::Managed(callback) => callback.entry.bind_diagnostics(
                    owner.diagnostic_monitor(),
                    &symbol,
                    source,
                    identity,
                    depth,
                ),
                Self::Raw(_) => (),
            }
        }
    }
    fn bind_foreign(
        &self,
        queue: &rinzu_runtime::foreign_queue::ForeignQueue,
        generation: u64,
        eligible: bool,
    ) {
        // SAFETY: the prepared call/capsule pins its Main dispatcher, signature
        // and handler context through every accepted native result receipt.
        unsafe {
            match self {
                Self::Length(callback) => callback.entry.bind_foreign(queue, generation, eligible),
                Self::Managed(callback) => callback.entry.bind_foreign(queue, generation, eligible),
                Self::Raw(_) => (),
            }
        }
    }
    fn prepare(
        owner: &Rc<RefCell<OwnerValues>>,
        engine: &NativeEngine,
        target: &str,
        expected: &SourceAbiType,
        capture: Option<Value>,
        resources: CallbackResources<'_>,
        origin: CallbackOrigin<'_>,
    ) -> Result<Self> {
        let CallbackResources {
            userdata,
            return_scope,
        } = resources;
        if crate::function_reference::core_length(target).is_some() {
            return LengthEntry::new(owner, target, expected, &engine.bindings.counters)
                .map(Self::Length);
        }
        let SourceAbiType::Callback { parameters, result } = expected else {
            bail!("callback plan lost its signature")
        };
        if let Some((signature, address)) = engine.bindings.entries.get(target) {
            ensure!(
                !signature.is_async
                    && !signature.is_supplant
                    && signature.parameters == *parameters
                    && signature.result == **result,
                "raw native callback lost its exact ABI"
            );
            return Ok(Self::Raw(*address));
        }
        let module = engine
            .bindings
            .module
            .as_ref()
            .ok_or_else(|| anyhow!("callback module is unavailable"))?;
        let function = &module.functions[function_index(module, target)?];
        let mut managed_parameters = parameters
            .iter()
            .map(crate::native_call::callback_type)
            .collect::<Result<Vec<_>>>()?;
        if capture.is_some() {
            managed_parameters.insert(
                0,
                function
                    .parameter_types
                    .first()
                    .cloned()
                    .ok_or_else(|| anyhow!("capture adapter has no parameter"))?,
            );
        }
        ensure!(
            !function.is_async
                && function.parameter_types == managed_parameters
                && function.result_type == crate::native_call::callback_type(result)?,
            "managed callback lost its exact target ABI"
        );
        ensure!(
            matches!(
                **result,
                SourceAbiType::Scalar(_)
                    | SourceAbiType::Unit
                    | SourceAbiType::String
                    | SourceAbiType::Bytes(_)
            ),
            "unsupported managed callback result"
        );
        let (fallback, buffer) = crate::native_call::callback_fallback(
            function.callback_fallback.as_ref(),
            return_scope,
        )?;
        if let Some(buffer) = &buffer {
            engine.bindings.counters.returned_buffer(buffer, true);
        }
        let mut context = Box::new(ManagedCallbackContext {
            owner: Rc::clone(owner),
            engine: engine.clone(),
            target: target.into(),
            capture,
            diagnostic_source: None,
            parameters: parameters.clone(),
            result: (**result).clone(),
            userdata,
            fallback,
            fallback_storage_bytes: buffer.as_ref().map(|buffer| buffer.storage_bytes()),
            return_scope: return_scope.cloned(),
            cancellation: origin.cancellation.callback(),
            main: origin.main.cloned().map(|mut task| {
                task.callback_admission = true;
                task
            }),
        });
        // SAFETY: the pinned context owns its code/module/engine/Owner through
        // close/settlement. The C gate checks origin before reading any Rc.
        let entry = unsafe {
            FrameCallback::prepare_deferred(
                managed_callback_handler,
                std::ptr::from_mut(&mut *context).cast(),
            )
        };
        Ok(Self::Managed(ManagedCallback {
            entry,
            _context: context,
        }))
    }
    fn word(&self) -> u64 {
        match self {
            Self::Length(callback) => callback.entry.entry() as usize as u64,
            Self::Managed(callback) => callback.entry.entry() as u64,
            Self::Raw(address) => *address as u64,
        }
    }
    fn open(&self) {
        match self {
            Self::Length(callback) => callback.entry.open(),
            Self::Managed(callback) => callback.entry.open(),
            Self::Raw(_) => (),
        }
    }
    fn close(&self) -> bool {
        match self {
            Self::Length(callback) => callback.entry.close(),
            Self::Managed(callback) => callback.entry.close(),
            Self::Raw(_) => true,
        }
    }
    fn settled(&self) -> bool {
        match self {
            Self::Length(callback) => callback.entry.settled(),
            Self::Managed(callback) => callback.entry.settled(),
            Self::Raw(_) => true,
        }
    }
    unsafe fn retire(&mut self) -> bool {
        match self {
            // SAFETY: Call proves whole capability closure and physical return.
            Self::Length(callback) => unsafe { callback.entry.retire() },
            // SAFETY: managed children have independent code/root pins, no control.
            Self::Managed(callback) => unsafe { callback.entry.retire() },
            Self::Raw(_) => true,
        }
    }
}
impl Drop for ManagedCallback {
    fn drop(&mut self) {
        if !self.entry.close() {
            native_fatal(86)
        }
    }
}
struct UserDataAdmission {
    owner: Rc<RefCell<OwnerValues>>,
    invocation: NativeUserDataInvocation,
}
impl Drop for UserDataAdmission {
    fn drop(&mut self) {
        self.owner
            .borrow_mut()
            .native_userdata_settle(self.invocation)
    }
}
impl ManagedCallbackContext {
    unsafe fn parameter(
        &self,
        ty: &SourceAbiType,
        word: u64,
        admission: Option<&UserDataAdmission>,
    ) -> std::result::Result<Value, ErrorKind> {
        Ok(match ty {
            SourceAbiType::Scalar(scalar) => Value::Scalar(
                ScalarValue::from_bits(crate::scalar_backend::kind(*scalar), word)
                    .map_err(|_| ErrorKind::Conversion)?,
            ),
            SourceAbiType::UserData(identity) => self.owner.borrow_mut().native_userdata_resolve(
                admission.unwrap_or_else(|| native_fatal(86)).invocation,
                identity,
                word,
            )?,
            SourceAbiType::String | SourceAbiType::Bytes(_) => {
                // SAFETY: this admitted frame has this validated pointer input;
                // the synchronous native caller retains it until return.
                unsafe {
                    self.owner.borrow_mut().receive_native_data(
                        &crate::native_call::callback_type(ty).unwrap_or_else(|_| native_fatal(86)),
                        word,
                    )
                }
                .map_err(|error| match error {
                    rinzu_runtime::data::NativeInputError::NullString
                    | rinzu_runtime::data::NativeInputError::NullByteArray => {
                        ErrorKind::NullNativeValue
                    }
                    rinzu_runtime::data::NativeInputError::InvalidUtf8 => ErrorKind::InvalidUtf8,
                    rinzu_runtime::data::NativeInputError::Data(_) => native_fatal(86),
                })?
            }
            SourceAbiType::Record(identity) => {
                let layout = self
                    .engine
                    .bindings
                    .records
                    .get(identity)
                    .unwrap_or_else(|| native_fatal(86));
                // SAFETY: the exact validated record remains readable through copy.
                unsafe { self.owner.borrow_mut().receive_native_record(layout, word) }.map_err(
                    |error| match error {
                        NativeRecordError::Null => ErrorKind::NullNativeValue,
                        NativeRecordError::Conversion => ErrorKind::Conversion,
                        _ => native_fatal(86),
                    },
                )?
            }
            SourceAbiType::Opaque(identity) => {
                if word == 0 {
                    return Err(ErrorKind::NullNativeValue);
                }
                let mut owner = self.owner.borrow_mut();
                let (ty, mut reservation) = owner
                    .reserve_native_opaque(identity)
                    .unwrap_or_else(|_| native_fatal(86));
                owner
                    .receive_native_opaque(ty, &mut reservation, word)
                    .unwrap_or_else(|_| native_fatal(86))
            }
            _ => native_fatal(86),
        })
    }
    unsafe fn invoke(&self, frame: &CallbackFrame) -> std::result::Result<u64, ErrorKind> {
        // userdata admission precedes any token examination, including NULL.
        let admission = self.userdata.map(|scope| UserDataAdmission {
            owner: Rc::clone(&self.owner),
            invocation: self.owner.borrow_mut().native_userdata_admit(scope),
        });
        let roots = OwnerFrame::new(&self.owner, &[]);
        let mut arguments = Vec::new();
        if let Some(capture) = &self.capture {
            roots.hold(capture);
            arguments.push(capture.clone());
        }
        arguments
            .try_reserve_exact(self.parameters.len())
            .unwrap_or_else(|_| native_fatal(71));
        let mut cursor = CallbackAbiCursor::default();
        for ty in &self.parameters {
            let scalar = if let SourceAbiType::Scalar(scalar) = ty {
                Some(crate::scalar_backend::kind(*scalar))
            } else {
                None
            };
            // SAFETY: parameters are the validated fixed native signature and
            // the bridge frame/original native stack remain live under admission.
            let word = unsafe { frame.word(&mut cursor, scalar) };
            // SAFETY: one exact physical input is copied/resolved before body.
            let value = unsafe { self.parameter(ty, word, admission.as_ref()) }?;
            roots.hold(&value);
            let copied = match ty {
                SourceAbiType::Bytes(length) => *length as usize,
                SourceAbiType::String => match self
                    .owner
                    .borrow()
                    .export(&value)
                    .unwrap_or_else(|_| native_fatal(86))
                {
                    Value::String(text) => text.len(),
                    _ => native_fatal(86),
                },
                _ => 0,
            };
            self.engine
                .bindings
                .counters
                .copied_bytes
                .fetch_add(copied, Ordering::Relaxed);
            rinzu_runtime::resource_accounting::record_copy(copied as u64);
            arguments.push(value);
            self.owner
                .borrow_mut()
                .collect()
                .unwrap_or_else(|_| native_fatal(86));
        }
        let mut observed = Vec::new();
        let module = self.engine.bindings.module.as_ref().unwrap();
        self.engine
            .bindings
            .counters
            .callback_entries
            .fetch_add(1, Ordering::Relaxed);
        let result = evaluate_sync_context(
            module,
            &self.target,
            arguments,
            Some(OwnerEvaluation {
                domain: None,
                callback: true,
                owner: &self.owner,
                source_native: &self.engine,
            }),
            &self.cancellation,
            &mut observed,
            self.main.as_ref().map(|task| MainEvaluation { task }),
        );
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                if let Some(failure) = error.downcast_ref::<ManagedFailure>()
                    && std::iter::once(&failure.0.primary)
                        .chain(&failure.0.cleanup)
                        .all(|diagnostic| {
                            matches!(
                                diagnostic.cause,
                                FailureCause::Error(_)
                                    | FailureCause::ArithmeticTrap
                                    | FailureCause::Panic
                            )
                        })
                {
                    return Ok(self.fallback.unwrap_or_else(|| native_fatal(86)));
                }
                native_fatal(86)
            }
        };
        let result = self
            .owner
            .borrow()
            .export(&result)
            .unwrap_or_else(|_| native_fatal(86));
        let buffer = match (&self.result, &result) {
            (SourceAbiType::String, Value::String(text)) => {
                Some(rinzu_runtime::callback_return::ProducedBuffer::string(
                    text,
                    false,
                    self.return_scope.as_ref(),
                ))
            }
            (SourceAbiType::Bytes(length), Value::Bytes(bytes))
                if bytes.len() as u64 == *length =>
            {
                Some(rinzu_runtime::callback_return::ProducedBuffer::bytes(
                    bytes,
                    *length,
                    false,
                    self.return_scope.as_ref(),
                ))
            }
            _ => None,
        };
        if let Some(buffer) = buffer {
            self.engine
                .bindings
                .counters
                .returned_buffer(&buffer, false);
            if !buffer.is_scoped() {
                rinzu_runtime::resource_accounting::record_return_site(
                    false,
                    buffer.storage_bytes(),
                    self.diagnostic_source,
                    &self.target,
                );
            }
            return Ok(buffer.bits());
        }
        match (&self.result, result) {
            (SourceAbiType::Unit, Value::Unit) => Ok(0),
            (SourceAbiType::Scalar(scalar), Value::Scalar(value))
                if value.kind() == crate::scalar_backend::kind(*scalar) =>
            {
                Ok(value.bits())
            }
            _ => native_fatal(86),
        }
    }
}
unsafe extern "C" fn managed_callback_handler(
    raw: *mut core::ffi::c_void,
    frame: *const CallbackFrame,
) -> u64 {
    // SAFETY: admission pins the correct context/frame and checked origin.
    let (context, frame) = unsafe { (&*raw.cast::<ManagedCallbackContext>(), &*frame) };
    let _transition = context.owner.borrow().callback_scope();
    let _activity = context.owner.borrow().prepare_activity(
        rinzu_runtime::native_diagnostics::ActivityKind::Managed,
        &context.target,
        context.diagnostic_source,
    );
    _activity.start();
    // SAFETY: the exact validated signature and native inputs remain live.
    unsafe { context.invoke(frame) }
        .unwrap_or_else(|_| context.fallback.unwrap_or_else(|| native_fatal(86)))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Prepared,
    Returned,
    Closed,
    Settled,
    Read,
    Released,
}

/// Own preparation independently of the published call ledger. A later failed
/// argument or Busy lease closes every earlier token without a synthetic return.
struct UserDataScopeGuard {
    owner: Rc<RefCell<OwnerValues>>,
    scope: Option<NativeUserDataScope>,
    opened: bool,
    result: bool,
    returned: bool,
}
impl UserDataScopeGuard {
    fn new(owner: &Rc<RefCell<OwnerValues>>) -> Self {
        let scope = owner.borrow_mut().native_userdata_enter();
        Self {
            owner: Rc::clone(owner),
            scope: Some(scope),
            opened: false,
            result: false,
            returned: false,
        }
    }
    fn prepare(&self, identity: &str, value: &Value) -> Result<u64> {
        self.owner
            .borrow_mut()
            .native_userdata_prepare(self.scope.unwrap(), identity, value)
    }
    fn expect_result(&mut self, identity: &str) -> Result<()> {
        self.owner
            .borrow_mut()
            .native_userdata_expect_result(self.scope.unwrap(), identity)?;
        self.result = true;
        Ok(())
    }
    fn open(&mut self) {
        self.owner
            .borrow_mut()
            .native_userdata_open(self.scope.unwrap());
        self.opened = true;
    }
    fn returned(&mut self) {
        self.owner
            .borrow_mut()
            .native_userdata_returned(self.scope.unwrap());
        self.returned = true;
        if !self.result {
            self.scope = None;
        }
    }
}
impl Drop for UserDataScopeGuard {
    fn drop(&mut self) {
        if let Some(scope) = self.scope {
            if self.result && self.returned {
                self.owner.borrow_mut().native_userdata_finish_result(scope);
            } else if self.opened {
                native_fatal(86);
            } else {
                self.owner.borrow_mut().native_userdata_discard(scope);
            }
        }
    }
}
struct CallbackOrigin<'a> {
    cancellation: &'a CancellationState,
    main: Option<&'a MainTaskContext>,
}
struct CallbackResources<'a> {
    userdata: Option<NativeUserDataScope>,
    return_scope: Option<&'a Arc<rinzu_runtime::callback_return::ReturnScope>>,
}

#[derive(Clone, Copy)]
enum RecordPhase {
    Prepared,
    Entered,
    Returned,
    Committed,
}
struct RecordLoanGuard {
    owner: Rc<RefCell<OwnerValues>>,
    loan: NativeRecordLoan,
    count: usize,
    phase: RecordPhase,
}
impl RecordLoanGuard {
    fn prepare<'a>(
        owner: &Rc<RefCell<OwnerValues>>,
        arguments: impl Iterator<Item = (&'a Arc<NativeRecordLayout>, ManagedHandle)> + Clone,
        diagnostic: Diagnostic,
    ) -> Result<Self> {
        let result = owner.borrow_mut().native_record_prepare(arguments);
        let (loan, count) = match result {
            Ok(value) => value,
            Err(NativeRecordLoanError::Busy) => {
                return Err(
                    ManagedFailure(owner.borrow_mut().native_record_busy(diagnostic)?).into(),
                );
            }
            Err(_) => native_fatal(86),
        };
        Ok(Self {
            owner: Rc::clone(owner),
            loan,
            count,
            phase: RecordPhase::Prepared,
        })
    }
    fn enter(&mut self) {
        self.owner.borrow_mut().native_record_enter(self.loan);
        self.phase = RecordPhase::Entered;
    }
    fn returned(&mut self) {
        self.owner.borrow_mut().native_record_returned(self.loan);
        self.phase = RecordPhase::Returned;
    }
    fn commit(&mut self) {
        self.owner.borrow_mut().native_record_commit(self.loan);
        self.phase = RecordPhase::Committed;
    }
}
impl Drop for RecordLoanGuard {
    fn drop(&mut self) {
        match self.phase {
            RecordPhase::Prepared => self.owner.borrow_mut().native_record_discard(self.loan),
            RecordPhase::Committed => self.owner.borrow_mut().native_record_release(self.loan),
            RecordPhase::Entered | RecordPhase::Returned => native_fatal(86),
        }
    }
}
struct CallbackPreparation(Vec<CallbackEntry>);
impl Drop for CallbackPreparation {
    fn drop(&mut self) {
        for callback in &mut self.0 {
            if !callback.close() {
                native_fatal(86);
            }
            // SAFETY: preparation has not published a capability or entered C.
            if !unsafe { callback.retire() } {
                native_fatal(86);
            }
        }
    }
}
pub(super) struct NativeCall {
    _accounting: rinzu_runtime::resource_accounting::Allocation,
    signature: SourceNativeSignature,
    activity: Option<rinzu_runtime::native_diagnostics::Activity>,
    words: Vec<u64>,
    _buffers: Vec<Box<[u8]>>,
    record_loan: Option<RecordLoanGuard>,
    record_count: usize,
    record_pending: bool,
    bindings: Arc<NativeBindings>,
    busy_diagnostic: Diagnostic,
    callbacks: Vec<CallbackEntry>,
    _roots: OwnerFrame,
    opaque_result: Option<(usize, rinzu_runtime::data::OpaqueReservation)>,
    record_result: Option<Arc<NativeRecordLayout>>,
    userdata: Option<UserDataScopeGuard>,
    phase: Phase,
    result: u64,
    counters: Arc<Counters>,
    return_scope: Option<Arc<rinzu_runtime::callback_return::ReturnScope>>,
}
impl NativeCall {
    fn record_targets<'a>(
        signature: &'a SourceNativeSignature,
        words: &'a [u64],
        bindings: &'a NativeBindings,
    ) -> impl Iterator<Item = (&'a Arc<NativeRecordLayout>, ManagedHandle)> + Clone {
        signature
            .parameters
            .iter()
            .zip(words)
            .filter_map(|(ty, bits)| {
                let SourceAbiType::Record(identity) = ty else {
                    return None;
                };
                Some((
                    bindings
                        .records
                        .get(identity)
                        .unwrap_or_else(|| native_fatal(86)),
                    ManagedHandle::from_bits(*bits).unwrap_or_else(|| native_fatal(86)),
                ))
            })
    }
    fn bind_foreign(&self, generation: u64) -> Result<()> {
        let dispatcher = self
            ._roots
            .owner
            .borrow()
            .dispatcher
            .clone()
            .ok_or_else(|| anyhow!("supplant Main dispatcher is unavailable"))?;
        for (callback, signature) in self.callbacks.iter().zip(
            self.signature
                .parameters
                .iter()
                .filter(|ty| matches!(ty, SourceAbiType::Callback { .. })),
        ) {
            callback.bind_foreign(
                dispatcher.queue(),
                generation,
                signature.foreign_callback_eligible(),
            );
        }
        Ok(())
    }
    fn prepare(
        signature: &SourceNativeSignature,
        plans: &[CallbackArgumentPlan],
        arguments: Vec<Value>,
        owner: &Rc<RefCell<OwnerValues>>,
        engine: &NativeEngine,
        busy: Diagnostic,
        origin: CallbackOrigin<'_>,
    ) -> Result<Self> {
        let (bound, _) = engine
            .bindings
            .entries
            .get(&signature.name)
            .ok_or_else(|| {
                anyhow!(
                    "source native binding is unavailable for {}",
                    signature.name
                )
            })?;
        ensure!(
            bound == signature && arguments.len() == signature.parameters.len(),
            "native preparation lost its exact signature"
        );
        ensure!(
            matches!(
                signature.result,
                SourceAbiType::Scalar(_)
                    | SourceAbiType::Unit
                    | SourceAbiType::Opaque(_)
                    | SourceAbiType::String
                    | SourceAbiType::Bytes(_)
                    | SourceAbiType::ByteArrayResult
                    | SourceAbiType::Record(_)
                    | SourceAbiType::UserData(_)
            ),
            "native managed result requires the R3 receiver backend"
        );
        let roots = OwnerFrame::new(owner, &arguments);
        let return_scope = engine
            .bindings
            .consumers
            .contains(&signature.name)
            .then(|| Arc::new(rinzu_runtime::callback_return::ReturnScope::new()));
        let mut words = Vec::new();
        let mut buffers = Vec::new();
        let mut has_records = false;
        let mut callbacks = CallbackPreparation(Vec::new());
        let mut userdata = (matches!(signature.result, SourceAbiType::UserData(_)) || signature
            .parameters
            .iter()
            .any(|ty| matches!(ty, SourceAbiType::UserData(_)) || matches!(ty,SourceAbiType::Callback {parameters,..} if parameters.iter().any(|parameter|matches!(parameter,SourceAbiType::UserData(_))))))
            .then(|| UserDataScopeGuard::new(owner));
        if let SourceAbiType::UserData(identity) = &signature.result {
            userdata.as_mut().unwrap().expect_result(identity)?;
        }
        for (index, (ty, value)) in signature.parameters.iter().zip(&arguments).enumerate() {
            if let SourceAbiType::UserData(identity) = ty {
                words.push(userdata.as_ref().unwrap().prepare(identity, value)?);
                continue;
            }
            if let SourceAbiType::Record(identity) = ty {
                let layout = engine
                    .bindings
                    .records
                    .get(identity)
                    .ok_or_else(|| anyhow!("missing native record layout: {identity}"))?;
                let Value::Managed { ty, handle } = value else {
                    native_fatal(86);
                };
                if *ty != layout.ty {
                    native_fatal(86);
                }
                has_records = true;
                words.push(handle.bits());
                continue;
            }
            if let SourceAbiType::Opaque(identity) = ty {
                words.push(owner.borrow().native_opaque_pointer(value, identity)?);
                continue;
            }
            if matches!(ty, SourceAbiType::Callback { .. }) {
                let plan = plans
                    .iter()
                    .find(|plan| plan.index == index)
                    .ok_or_else(|| anyhow!("missing callback parameter plan"))?;
                let (target, capture) = if let Some(expected) = &plan.capture {
                    let variant = owner
                        .borrow_mut()
                        .callback_capture_variant(value, expected)?;
                    (
                        plan.targets
                            .get(variant)
                            .ok_or_else(|| anyhow!("capture target variant is outside its plan"))?
                            .clone(),
                        Some(value.clone()),
                    )
                } else {
                    let Value::FunctionReference(target) = value else {
                        bail!("callback is not a function reference")
                    };
                    ensure!(
                        plan.targets.contains(target),
                        "callback target is outside its closed set"
                    );
                    (target.clone(), None)
                };
                let mut callback = CallbackEntry::prepare(
                    owner,
                    engine,
                    &target,
                    ty,
                    capture,
                    CallbackResources {
                        userdata: userdata.as_ref().and_then(|scope| scope.scope),
                        return_scope: return_scope.as_ref(),
                    },
                    CallbackOrigin {
                        cancellation: origin.cancellation,
                        main: origin.main,
                    },
                )?;
                match &mut callback {
                    CallbackEntry::Managed(managed) => {
                        managed._context.diagnostic_source = busy.source;
                        if let Some(bytes) = managed._context.fallback_storage_bytes
                            && return_scope.is_none()
                        {
                            rinzu_runtime::resource_accounting::record_return_site(
                                true,
                                bytes,
                                busy.source,
                                &target,
                            );
                        }
                    }
                    CallbackEntry::Length(length) => {
                        length._context.diagnostic_source = busy.source
                    }
                    _ => (),
                }
                callback.bind_diagnostics(&owner.borrow(), &target, busy.source);
                let pointer = callback.word();
                callbacks.0.push(callback);
                words.push(pointer);
                continue;
            }
            let value = owner.borrow().export(value)?;
            let word = match (ty, value) {
                (SourceAbiType::Scalar(scalar), Value::Scalar(value)) => {
                    ensure!(
                        value.kind() == crate::scalar_backend::kind(*scalar),
                        "native scalar argument kind mismatch"
                    );
                    value.bits()
                }
                (SourceAbiType::Bytes(length), Value::Bytes(mut bytes)) => {
                    ensure!(
                        bytes.len() as u64 == *length,
                        "native Bytes argument length mismatch"
                    );
                    if bytes.is_empty() {
                        bytes.push(0);
                    }
                    let bytes = bytes.into_boxed_slice();
                    let pointer = bytes.as_ptr() as u64;
                    buffers.push(bytes);
                    pointer
                }
                (SourceAbiType::ByteArray, Value::Bytes(mut bytes)) => {
                    if bytes.is_empty() {
                        bytes.push(0);
                    }
                    let bytes = bytes.into_boxed_slice();
                    let pointer = bytes.as_ptr() as u64;
                    buffers.push(bytes);
                    pointer
                }
                (SourceAbiType::String, Value::String(text)) => {
                    let mut bytes = text.into_bytes();
                    bytes.push(0);
                    let bytes = bytes.into_boxed_slice();
                    let pointer = bytes.as_ptr() as u64;
                    buffers.push(bytes);
                    pointer
                }
                _ => bail!("native argument requires the remaining R3 data backend"),
            };
            words.push(word);
        }
        let record_result =
            if let SourceAbiType::Record(identity) = &signature.result {
                Some(Arc::clone(
                    engine.bindings.records.get(identity).ok_or_else(|| {
                        anyhow!("missing native record result layout: {identity}")
                    })?,
                ))
            } else {
                None
            };
        let opaque_result = if let SourceAbiType::Opaque(identity) = &signature.result {
            let (ty, reservation) = owner.borrow_mut().reserve_native_opaque(identity)?;
            roots.hold(&Value::Managed {
                ty,
                handle: reservation.root().unwrap(),
            });
            Some((ty, reservation))
        } else {
            None
        };
        let record_loan = if !has_records || signature.is_supplant {
            None
        } else {
            let targets = Self::record_targets(signature, &words, &engine.bindings);
            match RecordLoanGuard::prepare(owner, targets, busy) {
                Ok(loan) => {
                    for (argument, (index, _)) in signature
                        .parameters
                        .iter()
                        .enumerate()
                        .filter(|(_, ty)| matches!(ty, SourceAbiType::Record(_)))
                        .enumerate()
                    {
                        words[index] = owner.borrow().native_record_pointer(loan.loan, argument);
                    }
                    Some(loan)
                }
                Err(error) => {
                    return Err(error);
                }
            }
        };
        let record_count = record_loan.as_ref().map_or(0, |loan| loan.count);
        let counters = Arc::clone(&engine.bindings.counters);
        counters.prepared.fetch_add(1, Ordering::Relaxed);
        counters.live_calls.fetch_add(1, Ordering::Relaxed);
        counters
            .live_buffers
            .fetch_add(buffers.len() + record_count, Ordering::Relaxed);
        counters
            .live_callbacks
            .fetch_add(callbacks.0.len(), Ordering::Relaxed);
        Ok(Self {
            signature: signature.clone(),
            _accounting: rinzu_runtime::resource_accounting::Allocation::new(
                rinzu_runtime::resource_accounting::Kind::NativeCalls,
                1,
            ),
            activity: Some(owner.borrow().prepare_activity(
                rinzu_runtime::native_diagnostics::ActivityKind::Native,
                match &signature.binding {
                    rinzu_compiler::ast::NativeBinding::Symbol(symbol) => symbol,
                    _ => &signature.name,
                },
                busy.source,
            )),
            words,
            _buffers: buffers,
            record_loan,
            record_count,
            record_pending: has_records && signature.is_supplant,
            bindings: Arc::clone(&engine.bindings),
            busy_diagnostic: busy,
            callbacks: std::mem::take(&mut callbacks.0),
            _roots: roots,
            opaque_result,
            record_result,
            userdata,
            phase: Phase::Prepared,
            result: 0,
            counters,
            return_scope,
        })
    }
    /// A queued supplant retains references; snapshot their current contents
    /// only after the synchronous managed chain has returned and before entry.
    fn prepare_records_at_entry(&mut self) -> Result<()> {
        if !self.record_pending {
            return Ok(());
        }
        ensure!(
            self.phase == Phase::Prepared && self.record_loan.is_none(),
            "native record entry preparation out of order"
        );
        let targets = Self::record_targets(&self.signature, &self.words, &self.bindings);
        let loan = RecordLoanGuard::prepare(&self._roots.owner, targets, self.busy_diagnostic)?;
        for (argument, (index, _)) in self
            .signature
            .parameters
            .iter()
            .enumerate()
            .filter(|(_, ty)| matches!(ty, SourceAbiType::Record(_)))
            .enumerate()
        {
            self.words[index] = self
                ._roots
                .owner
                .borrow()
                .native_record_pointer(loan.loan, argument);
        }
        self.record_count = loan.count;
        self.counters
            .live_buffers
            .fetch_add(self.record_count, Ordering::Relaxed);
        self.record_loan = Some(loan);
        self.record_pending = false;
        Ok(())
    }
    fn enter(&mut self, engine: &NativeEngine) -> Result<()> {
        ensure!(
            self.phase == Phase::Prepared,
            "native physical entry out of order"
        );
        ensure!(
            !self.record_pending,
            "native entry skipped record preparation"
        );
        let (_, address) = engine.bindings.entries.get(&self.signature.name).unwrap();
        if let Some(loan) = &mut self.record_loan {
            loan.enter();
        }
        if let Some(scope) = &self.return_scope {
            ensure!(scope.enter(), "return owner entered twice");
        }
        if let Some(userdata) = &mut self.userdata {
            userdata.open();
        }
        let transition = self._roots.native_scope();
        let dispatcher = self._roots.owner.borrow().dispatcher.clone();
        if !self.signature.is_supplant
            && let Some(dispatcher) = &dispatcher
        {
            dispatcher.queue().enter_bounded();
        }
        for callback in &self.callbacks {
            callback.open();
        }
        self.activity
            .as_ref()
            .unwrap_or_else(|| native_fatal(86))
            .start();
        // SAFETY: binding was checked against the complete source ABI; native
        // buffers/callback contexts and the exact retained root frame are live.
        self.result = unsafe {
            engine
                .stubs
                .as_ref()
                .unwrap()
                .invoke(&self.signature.name, *address, &self.words)
        }?;
        if !self.signature.is_supplant
            && let Some(dispatcher) = &dispatcher
        {
            dispatcher.queue().leave_bounded();
        }
        drop(self.activity.take());
        drop(transition);
        self.phase = Phase::Returned;
        if let Some(loan) = &mut self.record_loan {
            loan.returned();
        }
        if let Some(userdata) = &mut self.userdata {
            userdata.returned();
        }
        self.counters.returned.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    fn close(&mut self) -> Result<()> {
        ensure!(
            self.phase == Phase::Returned,
            "native close before physical return"
        );
        for callback in &self.callbacks {
            callback.close();
        }
        self.phase = Phase::Closed;
        self.counters.closed.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    fn abandon_before_entry(&mut self) -> Result<()> {
        ensure!(
            self.phase == Phase::Prepared,
            "supplant abandonment after physical entry"
        );
        for callback in &self.callbacks {
            ensure!(
                callback.close() && callback.settled(),
                "prepared callback admitted before physical entry"
            );
        }
        for callback in &mut self.callbacks {
            ensure!(
                // SAFETY: this call was cancelled before publishing C capabilities.
                unsafe { callback.retire() },
                "prepared callback control not retired"
            );
        }
        self.record_loan.take();
        self.userdata.take();
        Ok(())
    }
    fn settle(&mut self) -> Result<()> {
        let dispatcher = self._roots.owner.borrow().dispatcher.clone();
        if self.signature.is_supplant
            && let Some(dispatcher) = dispatcher
        {
            while !self.callbacks.iter().all(CallbackEntry::settled)
                || !dispatcher.queue().stats().is_baseline()
            {
                dispatcher.pump();
                std::thread::yield_now();
            }
        }
        ensure!(
            self.phase == Phase::Closed && self.callbacks.iter().all(CallbackEntry::settled),
            "native callback admission not settled"
        );
        if let Some(loan) = &mut self.record_loan {
            loan.commit();
        }
        self.phase = Phase::Settled;
        if let Some(scope) = &self.return_scope {
            // SAFETY: exact image/body proof closes all payload future uses;
            // physical return and all callback result receipts are settled.
            ensure!(unsafe { scope.consume() }, "return owner consumed twice");
            for callback in &mut self.callbacks {
                // SAFETY: exact closed-consumer.v1 includes whole capability
                // flow; this Origin Call returned physically and receipts ended.
                ensure!(unsafe { callback.retire() }, "callback control not retired");
            }
        }
        self.counters.settled.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    fn read(&mut self, mut null_diagnostic: Diagnostic) -> Result<Value> {
        ensure!(
            self.phase == Phase::Settled,
            "native read before callback settlement"
        );
        // An Error still completes the actual result-read phase. Its IR error
        // edge releases the ledger before any managed catch or finally runs.
        self.phase = Phase::Read;
        self.counters.read.fetch_add(1, Ordering::Relaxed);
        let value = match &self.signature.result {
            SourceAbiType::UserData(identity) => {
                let scope = self
                    .userdata
                    .as_ref()
                    .and_then(|guard| guard.scope)
                    .unwrap_or_else(|| native_fatal(86));
                match self
                    ._roots
                    .owner
                    .borrow_mut()
                    .native_userdata_receive_result(scope, identity, self.result)
                {
                    Ok(value) => value,
                    Err(_) => return Err(ManagedFailure(Failure::new(null_diagnostic)).into()),
                }
            }
            SourceAbiType::Unit => Value::Unit,
            SourceAbiType::Scalar(scalar) => Value::Scalar(
                ScalarValue::from_bits(crate::scalar_backend::kind(*scalar), self.result)
                    .unwrap_or_else(|_| native_fatal(86)),
            ),
            SourceAbiType::Opaque(_) => {
                let (ty, reservation) =
                    self.opaque_result.as_mut().expect("prepared opaque result");
                match self._roots.owner.borrow_mut().receive_native_opaque(
                    *ty,
                    reservation,
                    self.result,
                ) {
                    Ok(value) => value,
                    Err(rinzu_runtime::data::NativeOpaqueError::Null) => {
                        return Err(ManagedFailure(Failure::new(null_diagnostic)).into());
                    }
                    Err(error) => bail!("native opaque result invariant: {error:?}"),
                }
            }
            SourceAbiType::String | SourceAbiType::Bytes(_) | SourceAbiType::ByteArrayResult => {
                let ty = match self.signature.result {
                    SourceAbiType::String => rinzu_compiler::Type::String,
                    SourceAbiType::Bytes(length) => rinzu_compiler::Type::FixedBytes(length),
                    SourceAbiType::ByteArrayResult => rinzu_compiler::Type::Bytes,
                    _ => unreachable!(),
                };
                // SAFETY: the physical result was returned with this exact
                // validated String, fixed Bytes, or ByteArray descriptor pointer.
                let received = unsafe {
                    self._roots
                        .owner
                        .borrow_mut()
                        .receive_native_data(&ty, self.result)
                };
                match received {
                    Ok(value) => value,
                    Err(
                        rinzu_runtime::data::NativeInputError::NullString
                        | rinzu_runtime::data::NativeInputError::NullByteArray,
                    ) => {
                        return Err(ManagedFailure(Failure::new(null_diagnostic)).into());
                    }
                    Err(rinzu_runtime::data::NativeInputError::InvalidUtf8) => {
                        null_diagnostic.cause = FailureCause::Error(ErrorKind::InvalidUtf8);
                        return Err(ManagedFailure(Failure::new(null_diagnostic)).into());
                    }
                    Err(error) => bail!("native data result invariant: {error:?}"),
                }
            }
            SourceAbiType::Record(_) => {
                let layout = self
                    .record_result
                    .as_ref()
                    .expect("prepared native record layout");
                // SAFETY: the exact validated record pointer was physically
                // returned and remains stable through reception under the SDK contract.
                let received = unsafe {
                    self._roots
                        .owner
                        .borrow_mut()
                        .receive_native_record(layout, self.result)
                };
                match received {
                    Ok(value) => value,
                    Err(rinzu_runtime::data::NativeRecordError::Null) => {
                        return Err(ManagedFailure(Failure::new(null_diagnostic)).into());
                    }
                    Err(rinzu_runtime::data::NativeRecordError::Conversion) => {
                        null_diagnostic.cause = FailureCause::Error(ErrorKind::Conversion);
                        return Err(ManagedFailure(Failure::new(null_diagnostic)).into());
                    }
                    Err(error) => bail!("native record result invariant: {error:?}"),
                }
            }
            _ => unreachable!("result checked before entry"),
        };
        Ok(value)
    }
}
impl Drop for NativeCall {
    fn drop(&mut self) {
        if let Some((_, reservation)) = &mut self.opaque_result
            && reservation.root().is_some()
        {
            self._roots
                .owner
                .borrow_mut()
                .discard_native_opaque(reservation)
                .unwrap_or_else(|_| native_fatal(86));
        }
        self.counters.live_calls.fetch_sub(1, Ordering::Relaxed);
        self.counters
            .live_buffers
            .fetch_sub(self._buffers.len() + self.record_count, Ordering::Relaxed);
        self.counters
            .live_callbacks
            .fetch_sub(self.callbacks.len(), Ordering::Relaxed);
        if self.phase == Phase::Released {
            self.counters.released.fetch_add(1, Ordering::Relaxed);
        } else {
            self.counters.abandoned.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl Frame {
    pub(super) fn native_operation(
        &mut self,
        operation: &rinzu_compiler::Operation,
        engine: &NativeEngine,
        cancellation: &CancellationState,
        main: Option<&MainTaskContext>,
    ) -> Result<Option<Value>> {
        let value = match &operation.kind {
            OperationKind::PrepareNativeCall {
                call,
                signature,
                arguments,
                callbacks,
                ..
            } => {
                self.values
                    .try_reserve(1)
                    .unwrap_or_else(|_| native_fatal(86));
                ensure!(
                    !self.native_calls.contains_key(call),
                    "native call ledger identity reused"
                );
                let arguments = arguments
                    .iter()
                    .map(|id| {
                        self.values
                            .get(id)
                            .cloned()
                            .ok_or_else(|| anyhow!("missing native argument"))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let owner = &self
                    .data_roots
                    .as_ref()
                    .ok_or_else(|| anyhow!("native call has no Origin heap"))?
                    .owner;
                self.native_calls.insert(
                    *call,
                    NativeCall::prepare(
                        signature,
                        callbacks,
                        arguments,
                        owner,
                        engine,
                        self.failure.diagnostic(
                            FailureCause::Error(ErrorKind::NativeRecordBusy),
                            operation.range,
                        ),
                        CallbackOrigin { cancellation, main },
                    )?,
                );
                Value::Unit
            }
            OperationKind::SourceNativeCall {
                call: Some(call),
                signature,
                ..
            } => {
                let record = self
                    .native_calls
                    .get_mut(call)
                    .ok_or_else(|| anyhow!("missing prepared native call"))?;
                ensure!(
                    record.signature == *signature,
                    "physical call signature changed after preparation"
                );
                record.enter(engine)?;
                Value::Unit
            }
            OperationKind::CloseNativeCall { call } => {
                self.native_call(*call)?.close()?;
                Value::Unit
            }
            OperationKind::SettleNativeAdmissions { call, .. } => {
                self.native_call(*call)?.settle()?;
                Value::Unit
            }
            OperationKind::ReadNativeResult { call, error, .. } => {
                let diagnostic = self.failure.diagnostic(
                    FailureCause::Error(ErrorKind::NullNativeValue),
                    operation.range,
                );
                match self.native_call(*call)?.read(diagnostic) {
                    Ok(value) => value,
                    Err(failure) => {
                        if failure.downcast_ref::<ManagedFailure>().is_some() {
                            for call in &error.release_native_calls {
                                self.release_native(*call)?;
                            }
                        }
                        return Err(failure);
                    }
                }
            }
            OperationKind::ReleaseNativeCall { call } => {
                self.release_native(*call)?;
                Value::Unit
            }
            _ => return Ok(None),
        };
        Ok(Some(value))
    }
    fn native_call(&mut self, call: NativeCallId) -> Result<&mut NativeCall> {
        self.native_calls
            .get_mut(&call)
            .ok_or_else(|| anyhow!("missing native call ledger"))
    }
    fn release_native(&mut self, call: NativeCallId) -> Result<()> {
        ensure!(
            self.native_call(call)?.phase == Phase::Read,
            "native release before result read"
        );
        self.native_call(call)?.phase = Phase::Released;
        self.native_calls.remove(&call);
        Ok(())
    }
}

#[cfg(test)]
mod record_tests {
    use super::*;
    use rinzu_compiler::{FileId, SourceFile, ValueId};
    use rinzu_runtime::data::NativeRecordField;

    #[test]
    fn queued_records_retain_references_and_copy_current_fields_only_at_entry() {
        let artifact = rinzu_compiler::compile_source(&SourceFile::new(
            FileId(192),
            "queued-record.rinzu",
            r#"
            native {record Pair{x:Int} supplant fn call(value:Pair)->Int symbol "never_entered";}
            async fn main(){try{let value=Pair(x:1);let result=await call(value);}catch errors{}}
        "#,
        ))
        .unwrap();
        let function = artifact
            .ir
            .functions
            .iter()
            .find(|function| function.name == "main")
            .unwrap();
        let owner = OwnerValues::new(&artifact.ir).unwrap();
        let mut frame = Frame::new(&artifact.ir, 0, function, vec![]).with_owner(&owner);
        let mut construct=function.blocks.iter().flat_map(|block|&block.operations)
            .find(|operation|matches!(operation.kind,OperationKind::Construct{ref target,..} if target=="Pair")).unwrap().clone();
        if let OperationKind::Construct { arguments, .. } = &mut construct.kind {
            arguments[0] = ValueId(100);
        }
        frame.values.insert(ValueId(100), Value::int(1));
        let value = frame.data_operation(&construct).unwrap().unwrap();
        frame.data_roots.as_ref().unwrap().hold(&value);
        let Value::Managed { ty, handle } = value else {
            panic!("expected shared record")
        };
        let layout = Arc::new(
            NativeRecordLayout::new(
                ty,
                8,
                8,
                vec![NativeRecordField {
                    kind: ScalarKind::I64,
                    offset: 0,
                }],
            )
            .unwrap(),
        );
        let signature = artifact
            .ir
            .source_native_signatures
            .iter()
            .find(|signature| signature.is_supplant)
            .unwrap()
            .clone();
        let engine = NativeEngine {
            bindings: Arc::new(NativeBindings {
                module: Some(Arc::new(artifact.ir.clone())),
                entries: BTreeMap::from([(signature.name.clone(), (signature.clone(), 1))]),
                records: BTreeMap::from([("Pair".into(), Arc::clone(&layout))]),
                counters: Arc::default(),
                consumers: Default::default(),
            }),
            stubs: None,
        };
        let diagnostic = frame.failure.diagnostic(
            FailureCause::Error(ErrorKind::NativeRecordBusy),
            construct.range,
        );
        let cancellation = CancellationState::default();
        let mut queued = NativeCall::prepare(
            &signature,
            &[],
            vec![Value::Managed { ty, handle }],
            &owner,
            &engine,
            diagnostic,
            CallbackOrigin {
                cancellation: &cancellation,
                main: None,
            },
        )
        .unwrap();
        assert!(queued.record_loan.is_none());
        assert_eq!(queued.record_count, 0);
        // Another ordinary call can borrow and update the captured target while queued.
        let mut update =
            RecordLoanGuard::prepare(&owner, std::iter::once((&layout, handle)), diagnostic)
                .unwrap();
        let pointer = owner.borrow().native_record_pointer(update.loan, 0) as *mut i64;
        update.enter();
        // SAFETY: simulate a native write to the owned, initialized loan buffer.
        unsafe {
            pointer.write(21);
        }
        update.returned();
        update.commit();
        drop(update);
        queued.prepare_records_at_entry().unwrap();
        // SAFETY: entry preparation now owns the initialized current input copy.
        assert_eq!(unsafe { (queued.words[0] as *const i64).read() }, 21);
        queued.abandon_before_entry().unwrap();
        drop(queued);
        assert!(engine.bindings.evidence().is_baseline());
        drop(frame);
        let finished = owner.borrow_mut().finish().unwrap();
        assert_eq!((finished.objects, finished.roots), (0, 0));
    }
}
