//! Owner-thread ledger helpers. Physical calls happen in generated code, after
//! these helpers release their runtime borrow and before the return notification.
use super::*;
use rinzu_compiler::SourceAbiType;
use rinzu_runtime::callback_entry::{
    CallbackAbiCursor, CallbackFrame, FrameCallback, LengthKind, PointerU64Callback,
};
use rinzu_runtime::data::NativeUserDataScope;
use rinzu_runtime::data::{NativeRecordLayout, NativeRecordLoan, NativeRecordLoanError, RootLease};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Prepared,
    Returned,
    Closed,
    Settled,
    Read,
}
struct LengthContext {
    runtime: *mut BaselineRuntime,
    kind: LengthKind,
}
struct Callback {
    entry: PointerU64Callback,
    _context: Box<LengthContext>,
}
enum CallbackEntry {
    Length(Callback),
    Managed(ManagedCallback),
}
impl CallbackEntry {
    fn open(&self) {
        match self {
            Self::Length(callback) => callback.entry.open(),
            Self::Managed(callback) => callback.entry.open(),
        }
    }
    fn close(&self) -> bool {
        match self {
            Self::Length(callback) => callback.entry.close(),
            Self::Managed(callback) => callback.entry.close(),
        }
    }
    fn settled(&self) -> bool {
        match self {
            Self::Length(callback) => callback.entry.settled(),
            Self::Managed(callback) => callback.entry.settled(),
        }
    }
    unsafe fn retire(&mut self) -> bool {
        match self {
            // SAFETY: the Call owner supplies the complete capability/reader proof.
            Self::Length(callback) => unsafe { callback.entry.retire() },
            // SAFETY: managed children pin their own code and cannot read control.
            Self::Managed(callback) => unsafe { callback.entry.retire() },
        }
    }
}
struct ManagedContext {
    runtime: *mut BaselineRuntime,
    parameters: Vec<SourceAbiType>,
    result: SourceAbiType,
    userdata: Option<NativeUserDataScope>,
    fallback: Option<u64>,
    version: Arc<CodeVersion>,
    _pin: CodeFrame,
    stubs: std::rc::Rc<crate::source_native_abi::NativeAbiStubs>,
    stub_name: String,
    diagnostic_source: Option<SourcePosition>,
    symbol: String,
    capture: Option<ManagedHandle>,
    return_scope: Option<Arc<rinzu_runtime::callback_return::ReturnScope>>,
}
struct ManagedCallback {
    entry: FrameCallback,
    _context: Box<ManagedContext>,
}
impl Drop for ManagedCallback {
    fn drop(&mut self) {
        check(self.entry.close());
    }
}
impl Drop for Callback {
    fn drop(&mut self) {
        if !self.entry.close() {
            rinzu_runtime::native_fatal::invariant();
        }
    }
}
struct Call {
    _accounting: rinzu_runtime::resource_accounting::Allocation,
    plan: usize,
    words: Vec<u64>,
    buffers: Vec<Box<[u8]>>,
    record_loan: Option<NativeRecordLoan>,
    record_count: usize,
    callbacks: Vec<CallbackEntry>,
    roots: Vec<RootLease>,
    opaque_result: Option<(usize, rinzu_runtime::data::OpaqueReservation)>,
    record_result: Option<std::sync::Arc<NativeRecordLayout>>,
    userdata_scope: Option<NativeUserDataScope>,
    userdata_result: Option<usize>,
    phase: Phase,
    result: u64,
    native_scope: Option<rinzu_runtime::transition::NativeScope>,
    return_scope: Option<Arc<rinzu_runtime::callback_return::ReturnScope>>,
}
#[derive(Default)]
pub(super) struct NativeLedger {
    pub plans: Vec<Arc<crate::NativeCallPlan>>,
    pub targets: Vec<String>,
    pub bindings: BTreeMap<String, (rinzu_compiler::SourceNativeSignature, usize)>,
    pub records: BTreeMap<String, std::sync::Arc<NativeRecordLayout>>,
    pub functions: Vec<rinzu_compiler::IrFunction>,
    managed_stubs: Option<std::rc::Rc<crate::source_native_abi::NativeAbiStubs>>,
    calls: BTreeMap<(usize, usize), Box<Call>>,
    pub evidence: crate::SourceNativeCallEvidence,
    pub consumers: std::collections::BTreeSet<String>,
}
impl NativeLedger {
    pub fn prepare_managed_stubs(&mut self) -> Result<()> {
        let mut arities = std::collections::BTreeSet::new();
        for target in self
            .plans
            .iter()
            .flat_map(|plan| &plan.callbacks)
            .flat_map(|site| &site.targets)
        {
            if let Some(function) = self
                .functions
                .iter()
                .find(|function| function.name == *target)
            {
                arities.insert(function.parameter_types.len());
            }
        }
        if !arities.is_empty() {
            let signatures = arities
                .into_iter()
                .map(|arity| rinzu_compiler::SourceNativeSignature {
                    name: format!("managed:{arity}"),
                    binding: rinzu_compiler::ast::NativeBinding::Symbol("internal_managed".into()),
                    is_async: false,
                    is_supplant: false,
                    parameters: vec![
                        SourceAbiType::Scalar(rinzu_compiler::AbiScalar::U64);
                        arity + 1
                    ],
                    result: SourceAbiType::Scalar(rinzu_compiler::AbiScalar::U64),
                })
                .collect::<Vec<_>>();
            self.managed_stubs = Some(std::rc::Rc::new(
                crate::source_native_abi::NativeAbiStubs::new(&signatures)?,
            ));
        }
        Ok(())
    }
}

fn require<T>(result: Option<T>) -> T {
    result.unwrap_or_else(|| rinzu_runtime::native_fatal::invariant())
}
fn valid<T, E>(result: std::result::Result<T, E>) -> T {
    result.unwrap_or_else(|_| rinzu_runtime::native_fatal::invariant())
}
fn check(value: bool) {
    if !value {
        rinzu_runtime::native_fatal::invariant();
    }
}
fn key(runtime: &BaselineRuntime, plan: i64) -> (usize, usize) {
    (
        runtime.data_roots.frame_count(),
        valid(usize::try_from(plan)),
    )
}

fn discard_prepared(runtime: &mut BaselineRuntime, call: &mut Call) {
    check(call.phase == Phase::Prepared && call.native_scope.is_none());
    if let Some(loan) = call.record_loan.take() {
        valid(
            runtime
                .data
                .discard_prepared_native_records(&mut runtime.heap, loan),
        );
    }
    if let Some(scope) = call.userdata_scope.take() {
        runtime
            .userdata
            .discard_prepared(&runtime.heap, &mut runtime.data_roots, scope);
    }
    for lease in call.roots.drain(..) {
        valid(runtime.data_roots.unpin(lease));
    }
    if let Some((_, reservation)) = &mut call.opaque_result {
        valid(
            runtime
                .data
                .discard_native_opaque(&mut runtime.heap, reservation),
        );
    }
    for callback in &mut call.callbacks {
        check(callback.close());
        // SAFETY: the prepared call never entered native code or published any
        // callback capability; no template, bridge or receipt reader exists.
        check(unsafe { callback.retire() });
    }
}

pub(super) extern "C" fn prepare(raw: *mut BaselineRuntime, plan_id: i64, arguments: *const u64) {
    // SAFETY: generated code supplies its live Owner context; no physical call
    // or callback occurs while this mutable borrow is held.
    let runtime = unsafe { &mut *raw };
    let key = key(runtime, plan_id);
    check(!runtime.source_native.calls.contains_key(&key));
    let plan = require(runtime.source_native.plans.get(key.1)).clone();
    let bound = require(runtime.source_native.bindings.get(&plan.signature.name));
    check(bound.0 == plan.signature);
    check(matches!(
        plan.signature.result,
        SourceAbiType::Scalar(_)
            | SourceAbiType::Unit
            | SourceAbiType::Opaque(_)
            | SourceAbiType::String
            | SourceAbiType::Bytes(_)
            | SourceAbiType::ByteArrayResult
            | SourceAbiType::Record(_)
            | SourceAbiType::UserData(_)
    ));
    let arguments = if plan.signature.parameters.is_empty() {
        &[][..]
    } else {
        check(!arguments.is_null());
        // SAFETY: this exact call-site plan controls the generated stack array.
        unsafe { std::slice::from_raw_parts(arguments, plan.signature.parameters.len()) }
    };
    let mut words = Vec::new();
    if words
        .try_reserve_exact(plan.signature.parameters.len())
        .is_err()
    {
        rinzu_runtime::native_fatal::resource();
    }
    let mut call = Call {
        _accounting: rinzu_runtime::resource_accounting::Allocation::new(rinzu_runtime::resource_accounting::Kind::NativeCalls, 1),
        plan: key.1,
        words,
        buffers: Vec::new(),
        record_loan: None,
        record_count: 0,
        callbacks: Vec::new(),
        roots: Vec::new(),
        opaque_result: None,
        record_result: None,
        userdata_result: None,
        userdata_scope: (matches!(plan.signature.result, SourceAbiType::UserData(_)) || plan
            .signature
            .parameters
            .iter()
            .any(|ty| matches!(ty, SourceAbiType::UserData(_)) || matches!(ty,SourceAbiType::Callback {parameters,..} if parameters.iter().any(|parameter|matches!(parameter,SourceAbiType::UserData(_))))))
            .then(|| runtime.userdata.enter_scope(&runtime.heap, &runtime.data)),
        phase: Phase::Prepared,
        result: 0,
        native_scope: None,
        return_scope: runtime.source_native.consumers.contains(&plan.signature.name)
            .then(|| Arc::new(rinzu_runtime::callback_return::ReturnScope::new())),
    };
    if let SourceAbiType::UserData(identity) = &plan.signature.result {
        let ty = valid(
            runtime
                .data_schema
                .index(&rinzu_compiler::Type::Named(identity.clone())),
        );
        runtime.userdata.expect_result(
            &runtime.heap,
            &runtime.data,
            require(call.userdata_scope),
            ty,
        );
        call.userdata_result = Some(ty);
    }
    let mut has_records = false;
    for (index, (ty, bits)) in plan.signature.parameters.iter().zip(arguments).enumerate() {
        let word = match ty {
            SourceAbiType::Scalar(scalar) => {
                valid(rinzu_runtime::scalar::ScalarValue::from_bits(
                    crate::scalar_backend::kind(*scalar),
                    *bits,
                ));
                *bits
            }
            SourceAbiType::Bytes(_) | SourceAbiType::ByteArray | SourceAbiType::String => {
                let handle = require(ManagedHandle::from_bits(*bits));
                call.roots.push(valid(runtime.data_roots.pin(handle)));
                let mut bytes = match ty {
                    SourceAbiType::Bytes(length) => {
                        valid(runtime.data.bytes(&runtime.heap, handle, *length))
                    }
                    SourceAbiType::ByteArray => {
                        valid(runtime.data.dynamic_bytes(&runtime.heap, handle))
                    }
                    _ => valid(runtime.data.string(&runtime.heap, handle)).into_bytes(),
                };
                if matches!(ty, SourceAbiType::String) || bytes.is_empty() {
                    bytes.push(0);
                }
                let bytes = bytes.into_boxed_slice();
                let address = bytes.as_ptr() as u64;
                call.buffers.push(bytes);
                address
            }
            SourceAbiType::Opaque(identity) => {
                let ty = valid(runtime.data_schema.index(&rinzu_compiler::Type::Native {
                    name: identity.clone(),
                    call_only: false,
                }));
                let handle = require(ManagedHandle::from_bits(*bits));
                let pointer = valid(
                    runtime
                        .data
                        .native_opaque_pointer(&runtime.heap, handle, ty),
                );
                call.roots.push(valid(runtime.data_roots.pin(handle)));
                pointer
            }
            SourceAbiType::UserData(identity) => {
                let ty = valid(
                    runtime
                        .data_schema
                        .index(&rinzu_compiler::Type::Named(identity.clone())),
                );
                runtime
                    .userdata
                    .prepare(
                        &runtime.heap,
                        &runtime.data,
                        &mut runtime.data_roots,
                        require(call.userdata_scope),
                        ty,
                        require(ManagedHandle::from_bits(*bits)),
                    )
                    .bits()
            }
            SourceAbiType::Record(identity) => {
                require(runtime.source_native.records.get(identity));
                let handle = require(ManagedHandle::from_bits(*bits));
                call.roots.push(valid(runtime.data_roots.pin(handle)));
                has_records = true;
                0
            }
            SourceAbiType::Callback { parameters, result } => {
                let site = require(plan.callbacks.iter().find(|site| site.index == index));
                let (target, capture) = if let Some(capture_type) = &site.capture {
                    let handle = require(ManagedHandle::from_bits(*bits));
                    let ty = valid(runtime.data_schema.index(capture_type));
                    let child_types = valid(runtime.data_schema.child_types(ty, None, 1));
                    let rinzu_runtime::data::ManagedSlot::Reference(environment) =
                        valid(runtime.data.project(&mut runtime.heap, handle, ty, None, 0))
                    else {
                        rinzu_runtime::native_fatal::invariant()
                    };
                    let variant = valid(runtime.data.enum_variant(
                        &runtime.heap,
                        environment,
                        child_types[0],
                    )) as usize;
                    call.roots.push(valid(runtime.data_roots.pin(handle)));
                    (require(site.targets.get(variant)).clone(), Some(handle))
                } else {
                    let target = require(
                        runtime.source_native.targets.get(
                            valid(usize::try_from(*bits))
                                .checked_sub(1)
                                .unwrap_or_else(|| rinzu_runtime::native_fatal::invariant()),
                        ),
                    )
                    .clone();
                    check(site.targets.contains(&target));
                    (target, None)
                };
                let target = &target;
                if let Some((signature, address)) = runtime.source_native.bindings.get(target) {
                    check(
                        !signature.is_async
                            && !signature.is_supplant
                            && signature.parameters == *parameters
                            && signature.result == **result,
                    );
                    call.words.push(*address as u64);
                    continue;
                }
                let Some((kind, length)) = crate::function_reference::core_length(target) else {
                    let function = valid(
                        runtime
                            .function_names
                            .iter()
                            .position(|name| name == target)
                            .ok_or("callback target"),
                    );
                    let body = &runtime.source_native.functions[function];
                    let mut managed_parameters = valid(
                        parameters
                            .iter()
                            .map(crate::native_call::callback_type)
                            .collect::<Result<Vec<_>>>(),
                    );
                    if let Some(ty) = &site.capture {
                        managed_parameters.insert(0, ty.clone());
                    }
                    check(
                        !body.is_async
                            && body.parameter_types == managed_parameters
                            && body.result_type == valid(crate::native_call::callback_type(result)),
                    );
                    check(matches!(
                        **result,
                        SourceAbiType::Scalar(_)
                            | SourceAbiType::Unit
                            | SourceAbiType::String
                            | SourceAbiType::Bytes(_)
                    ));
                    let target = valid(runtime.dispatch_cells[function].load());
                    let version = Arc::clone(require(
                        runtime.code_versions.get(&(function, target.version)),
                    ));
                    let (fallback, buffer) = valid(crate::native_call::callback_fallback(
                        body.callback_fallback.as_ref(),
                        call.return_scope.as_ref(),
                    ));
                    if let Some(buffer) = &buffer {
                        if !buffer.is_scoped() {
                            rinzu_runtime::resource_accounting::record_return_site(
                                true,
                                buffer.storage_bytes(),
                                runtime.current_source,
                                &body.name,
                            );
                        }
                        runtime
                            .source_native
                            .evidence
                            .callback_returns
                            .record(buffer, true);
                    }
                    let mut context = Box::new(ManagedContext {
                        runtime: raw,
                        parameters: parameters.clone(),
                        result: (**result).clone(),
                        userdata: call.userdata_scope,
                        fallback,
                        _pin: valid(version.enter()),
                        version,
                        stubs: std::rc::Rc::clone(require(
                            runtime.source_native.managed_stubs.as_ref(),
                        )),
                        stub_name: format!(
                            "managed:{}",
                            parameters.len() + usize::from(capture.is_some())
                        ),
                        capture,
                        diagnostic_source: runtime.current_source,
                        symbol: body.name.clone(),
                        return_scope: call.return_scope.clone(),
                    });
                    // SAFETY: the context/version is pinned through close and
                    // settlement; the entry checks its physical Owner first.
                    let entry = unsafe {
                        FrameCallback::prepare_deferred(
                            managed_body,
                            std::ptr::from_mut(&mut *context).cast(),
                        )
                    };
                    let word = entry.entry() as u64;
                    call.callbacks.push(CallbackEntry::Managed(ManagedCallback {
                        entry,
                        _context: context,
                    }));
                    call.words.push(word);
                    continue;
                };
                let (kind, parameter) = match kind {
                    rinzu_compiler::CoreLengthKind::Bytes => {
                        let length = require(length);
                        (LengthKind::Bytes(length), SourceAbiType::Bytes(length))
                    }
                    _ => (LengthKind::Utf8, SourceAbiType::String),
                };
                check(
                    parameters == &[parameter]
                        && **result == SourceAbiType::Scalar(rinzu_compiler::AbiScalar::U64),
                );
                let mut context = Box::new(LengthContext { runtime: raw, kind });
                // SAFETY: the context stays pinned until all admissions settle;
                // the entry gate validates the actual physical Owner thread.
                let entry = unsafe {
                    PointerU64Callback::prepare_deferred(
                        length_body,
                        std::ptr::from_mut(&mut *context).cast(),
                    )
                };
                let address = entry.entry() as usize as u64;
                call.callbacks.push(CallbackEntry::Length(Callback {
                    entry,
                    _context: context,
                }));
                address
            }
            _ => rinzu_runtime::native_fatal::invariant(),
        };
        call.words.push(word);
    }
    if let SourceAbiType::Opaque(identity) = &plan.signature.result {
        let ty = valid(runtime.data_schema.index(&rinzu_compiler::Type::Native {
            name: identity.clone(),
            call_only: false,
        }));
        let reservation = valid(runtime.data.reserve_native_opaque(&mut runtime.heap, ty));
        call.roots
            .push(valid(runtime.data_roots.pin(require(reservation.root()))));
        runtime.peak_heap_objects = runtime.peak_heap_objects.max(runtime.heap.object_count());
        call.opaque_result = Some((ty, reservation));
    }
    if let SourceAbiType::Record(identity) = &plan.signature.result {
        call.record_result = Some(std::sync::Arc::clone(require(
            runtime.source_native.records.get(identity),
        )));
    }
    if has_records {
        let targets = plan
            .signature
            .parameters
            .iter()
            .zip(arguments)
            .filter_map(|(ty, bits)| {
                let SourceAbiType::Record(identity) = ty else {
                    return None;
                };
                Some((
                    require(runtime.source_native.records.get(identity)),
                    require(ManagedHandle::from_bits(*bits)),
                ))
            });
        let before = runtime.data.native_record_loan_stats().targets;
        let loan = match runtime
            .data
            .prepare_native_records_iter(&mut runtime.heap, targets)
        {
            Ok(loan) => loan,
            Err(NativeRecordLoanError::Busy) => {
                discard_prepared(runtime, &mut call);
                drop(call);
                runtime.data_result::<()>(Err(DataError::Thrown(
                    rinzu_runtime::error_value::ErrorValue::new(
                        4,
                        "native record is already in use",
                    ),
                )));
                return;
            }
            Err(_) => rinzu_runtime::native_fatal::invariant(),
        };
        call.record_count = runtime.data.native_record_loan_stats().targets - before;
        for (argument, (index, _)) in plan
            .signature
            .parameters
            .iter()
            .enumerate()
            .filter(|(_, ty)| matches!(ty, SourceAbiType::Record(_)))
            .enumerate()
        {
            call.words[index] = valid(runtime.data.native_record_argument_pointer(
                &runtime.heap,
                loan,
                argument,
            )) as u64;
        }
        call.record_loan = Some(loan);
    }
    let evidence = &mut runtime.source_native.evidence;
    evidence.prepared += 1;
    evidence.live_calls += 1;
    evidence.live_buffers += call.buffers.len() + call.record_count;
    evidence.live_callbacks += call.callbacks.len();
    runtime.source_native.calls.insert(key, Box::new(call));
}

struct PreparedInputs {
    runtime: *mut BaselineRuntime,
    roots: Vec<RootLease>,
    invocation: Option<rinzu_runtime::data::NativeUserDataInvocation>,
}
impl Drop for PreparedInputs {
    fn drop(&mut self) {
        // SAFETY: physical admission keeps the same Owner runtime live through
        // parameter/body settlement. No compiled code executes under this borrow.
        let runtime = unsafe { &mut *self.runtime };
        for root in &self.roots {
            valid(runtime.data_roots.unpin(*root));
        }
        if let Some(invocation) = self.invocation.take() {
            runtime
                .userdata
                .settle(&runtime.heap, &mut runtime.data_roots, invocation);
        }
    }
}
impl ManagedContext {
    unsafe fn parameter(
        &self,
        runtime: &mut BaselineRuntime,
        ty: &SourceAbiType,
        word: u64,
        invocation: Option<rinzu_runtime::data::NativeUserDataInvocation>,
    ) -> std::result::Result<(u64, Option<ManagedHandle>, usize), ErrorKind> {
        if let SourceAbiType::Scalar(scalar) = ty {
            return rinzu_runtime::scalar::ScalarValue::from_bits(
                crate::scalar_backend::kind(*scalar),
                word,
            )
            .map(|value| (value.bits(), None, 0))
            .map_err(|_| ErrorKind::Conversion);
        }
        let (handle, copied) = match ty {
            SourceAbiType::UserData(identity) => {
                let ty = valid(
                    runtime
                        .data_schema
                        .index(&rinzu_compiler::Type::Named(identity.clone())),
                );
                let handle = runtime
                    .userdata
                    .resolve(
                        &runtime.heap,
                        &runtime.data,
                        require(invocation),
                        ty,
                        word as *const u8,
                    )
                    .map_err(|rinzu_runtime::data::NativeUserDataError::Null| {
                        ErrorKind::NullNativeValue
                    })?;
                (handle, 0)
            }
            SourceAbiType::String | SourceAbiType::Bytes(_) => {
                // SAFETY: this admitted validated parameter is exactly a
                // NUL-terminated UTF-8 pointer or fixed-N readable buffer.
                let handle = unsafe {
                    match ty {
                        SourceAbiType::String => runtime
                            .data
                            .receive_native_string(&mut runtime.heap, word as *const u8),
                        SourceAbiType::Bytes(length) => runtime.data.receive_native_bytes(
                            &mut runtime.heap,
                            *length,
                            word as *const u8,
                        ),
                        _ => unreachable!(),
                    }
                }
                .map_err(|error| match error {
                    rinzu_runtime::data::NativeInputError::NullString
                    | rinzu_runtime::data::NativeInputError::NullByteArray => {
                        ErrorKind::NullNativeValue
                    }
                    rinzu_runtime::data::NativeInputError::InvalidUtf8 => ErrorKind::InvalidUtf8,
                    _ => rinzu_runtime::native_fatal::invariant(),
                })?;
                let length = match ty {
                    SourceAbiType::Bytes(length) => *length,
                    _ => valid(runtime.data.string_length(&runtime.heap, handle)) as u64,
                };
                (handle, valid(usize::try_from(length)))
            }
            SourceAbiType::Record(identity) => {
                let layout = require(runtime.source_native.records.get(identity));
                // SAFETY: native retains this exact struct during complete copy.
                let handle = unsafe {
                    runtime
                        .data
                        .receive_native_record(&mut runtime.heap, layout, word as *const u8)
                }
                .map_err(|error| match error {
                    rinzu_runtime::data::NativeRecordError::Null => ErrorKind::NullNativeValue,
                    rinzu_runtime::data::NativeRecordError::Conversion => ErrorKind::Conversion,
                    _ => rinzu_runtime::native_fatal::invariant(),
                })?;
                (handle, 0)
            }
            SourceAbiType::Opaque(identity) => {
                if word == 0 {
                    return Err(ErrorKind::NullNativeValue);
                }
                let ty = valid(runtime.data_schema.index(&rinzu_compiler::Type::Native {
                    name: identity.clone(),
                    call_only: false,
                }));
                let mut reservation =
                    valid(runtime.data.reserve_native_opaque(&mut runtime.heap, ty));
                let handle = valid(runtime.data.receive_native_opaque(
                    &mut runtime.heap,
                    &mut reservation,
                    word,
                ));
                (handle, 0)
            }
            _ => rinzu_runtime::native_fatal::invariant(),
        };
        Ok((handle.bits(), Some(handle), copied))
    }
    unsafe fn invoke(&self, frame: &CallbackFrame) -> std::result::Result<u64, ErrorKind> {
        let mut prepared = PreparedInputs {
            runtime: self.runtime,
            roots: Vec::new(),
            invocation: None,
        };
        prepared
            .roots
            .try_reserve_exact(self.parameters.len())
            .unwrap_or_else(|_| rinzu_runtime::native_fatal::invariant());
        let mut words = Vec::new();
        words
            .try_reserve_exact(self.parameters.len() + 1)
            .unwrap_or_else(|_| rinzu_runtime::native_fatal::invariant());
        words.push(self.runtime as u64);
        if let Some(capture) = self.capture {
            words.push(capture.bits());
        }
        {
            // SAFETY: only the admitted physical Owner accesses this runtime;
            // the borrow ends before calling any compiled managed body.
            let runtime = unsafe { &mut *self.runtime };
            prepared.invocation = self
                .userdata
                .map(|scope| runtime.userdata.admit(&runtime.heap, scope));
            let mut cursor = CallbackAbiCursor::default();
            for ty in &self.parameters {
                let scalar = if let SourceAbiType::Scalar(scalar) = ty {
                    Some(crate::scalar_backend::kind(*scalar))
                } else {
                    None
                };
                // SAFETY: decoder follows the exact fixed physical signature.
                let word = unsafe { frame.word(&mut cursor, scalar) };
                // SAFETY: every pointer has this signature-selected input contract.
                let (bits, handle, copied) =
                    unsafe { self.parameter(runtime, ty, word, prepared.invocation) }?;
                if let Some(handle) = handle {
                    prepared.roots.push(valid(runtime.data_roots.pin(handle)));
                }
                words.push(bits);
                runtime.source_native.evidence.copied_bytes += copied;
                rinzu_runtime::resource_accounting::record_copy(copied as u64);
                runtime.peak_heap_objects =
                    runtime.peak_heap_objects.max(runtime.heap.object_count());
                valid(
                    runtime
                        .data
                        .retain_roots(&mut runtime.heap, runtime.data_roots.live()),
                );
                valid(runtime.heap.collect(CollectionKind::Full));
                runtime.collection_count += 1;
            }
        }
        let (pending, source, depth, dispatch_depth) = {
            // SAFETY: no compiled code runs while reading/preparing its failure scope.
            let runtime = unsafe { &mut *self.runtime };
            runtime.callback_depth = runtime
                .callback_depth
                .checked_add(1)
                .unwrap_or_else(|| rinzu_runtime::native_fatal::invariant());
            runtime.callback_dispatches += 1;
            runtime.source_native.evidence.callback_entries += 1;
            (
                std::mem::take(&mut runtime.pending),
                runtime.current_source.take(),
                runtime.data_roots.frame_count(),
                runtime.managed_frames.len(),
            )
        };
        // SAFETY: the word stub has precisely context + declared internal u64
        // parameters, and the captured version's call-lifetime frame pins code.
        // Neither the runtime nor its fields are borrowed across this reentry.
        let mut result = valid(unsafe {
            self.stubs
                .invoke(&self.stub_name, self.version.address(), &words)
        });
        {
            // SAFETY: actual compiled return ended the managed activation and
            // this invocation owns the Owner's failure/root settlement turn.
            let runtime = unsafe { &mut *self.runtime };
            check(
                runtime.data_roots.frame_count() == depth + 1
                    && runtime.managed_frames.len() == dispatch_depth,
            );
            let failure = valid(std::mem::take(&mut runtime.pending).finish());
            if failure.is_none() {
                let buffer =
                    match self.result {
                        SourceAbiType::String => {
                            Some(rinzu_runtime::callback_return::ProducedBuffer::string(
                                &valid(runtime.data.string(
                                    &runtime.heap,
                                    require(ManagedHandle::from_bits(result)),
                                )),
                                false,
                                self.return_scope.as_ref(),
                            ))
                        }
                        SourceAbiType::Bytes(length) => {
                            Some(rinzu_runtime::callback_return::ProducedBuffer::bytes(
                                &valid(runtime.data.bytes(
                                    &runtime.heap,
                                    require(ManagedHandle::from_bits(result)),
                                    length,
                                )),
                                length,
                                false,
                                self.return_scope.as_ref(),
                            ))
                        }
                        _ => None,
                    };
                if let Some(buffer) = buffer {
                    if !buffer.is_scoped() {
                        rinzu_runtime::resource_accounting::record_return_site(
                            false,
                            buffer.storage_bytes(),
                            self.diagnostic_source,
                            &self.symbol,
                        );
                    }
                    runtime
                        .source_native
                        .evidence
                        .callback_returns
                        .record(&buffer, false);
                    result = buffer.bits();
                }
            }
            runtime.release_return_root();
            valid(runtime.data_roots.leave_frame());
            runtime.pending = pending;
            runtime.current_source = source;
            runtime.callback_depth -= 1;
            if let Some(failure) = failure {
                check(
                    std::iter::once(&failure.primary)
                        .chain(&failure.cleanup)
                        .all(|diagnostic| {
                            matches!(
                                diagnostic.cause,
                                FailureCause::Error(_)
                                    | FailureCause::ArithmeticTrap
                                    | FailureCause::Panic
                            )
                        }),
                );
                return Ok(require(self.fallback));
            }
        }
        if let SourceAbiType::Scalar(scalar) = self.result {
            valid(rinzu_runtime::scalar::ScalarValue::from_bits(
                crate::scalar_backend::kind(scalar),
                result,
            ));
        }
        Ok(result)
    }
}
unsafe extern "C" fn managed_body(raw: *mut c_void, frame: *const CallbackFrame) -> u64 {
    // SAFETY: the control gate pins context and checked its Owner before entry.
    let (context, frame) = unsafe { (&*raw.cast::<ManagedContext>(), &*frame) };
    // SAFETY: admission retains this runtime. Clone only the machine and end
    // the shared borrow before any mutable Owner access or compiled reentry.
    let machine = Arc::clone(&unsafe { &*context.runtime }.transitions);
    let _transition = valid(machine.callback_scope());
    // SAFETY: native storage and the exact declared frame live through return.
    unsafe { context.invoke(frame) }.unwrap_or_else(|_| require(context.fallback))
}

unsafe extern "C" fn length_body(raw: *mut c_void, input: *const u8) -> u64 {
    // SAFETY: the closed/thread gate admitted this pinned connection.
    let context = unsafe { &*raw.cast::<LengthContext>() };
    // SAFETY: physical native entry ran after the preparation helper released
    // its mutable borrow. The synchronous callback is now this heap's Owner.
    let runtime = unsafe { &mut *context.runtime };
    let _transition = valid(runtime.transitions.callback_scope());
    // SAFETY: external input is received by its exact signature-selected type.
    let handle = valid(unsafe {
        match context.kind {
            LengthKind::Bytes(length) => {
                runtime
                    .data
                    .receive_native_bytes(&mut runtime.heap, length, input)
            }
            LengthKind::Utf8 => runtime.data.receive_native_string(&mut runtime.heap, input),
        }
    });
    runtime.data_roots.enter_frame();
    valid(runtime.data_roots.publish([handle]));
    valid(
        runtime
            .data
            .retain_roots(&mut runtime.heap, runtime.data_roots.live()),
    );
    valid(runtime.heap.collect(CollectionKind::Full));
    let length = match context.kind {
        LengthKind::Bytes(length) => valid(runtime.data.byte_length(&runtime.heap, handle, length)),
        _ => valid(runtime.data.string_length(&runtime.heap, handle)) as u64,
    };
    valid(runtime.data_roots.leave_frame());
    valid(
        runtime
            .data
            .retain_roots(&mut runtime.heap, runtime.data_roots.live()),
    );
    valid(runtime.heap.collect(CollectionKind::Full));
    runtime.collection_count += 2;
    runtime.source_native.evidence.callback_entries += 1;
    runtime.source_native.evidence.copied_bytes += length as usize;
    rinzu_runtime::resource_accounting::record_copy(length);
    length
}

pub(super) extern "C" fn address(raw: *mut BaselineRuntime, plan: i64) -> u64 {
    // SAFETY: generated code supplies the live Owner context.
    let runtime = unsafe { &mut *raw };
    let call = require(runtime.source_native.calls.get(&key(runtime, plan)));
    check(call.phase == Phase::Prepared);
    require(
        runtime
            .source_native
            .bindings
            .get(&runtime.source_native.plans[call.plan].signature.name),
    )
    .1 as u64
}
pub(super) extern "C" fn words(raw: *mut BaselineRuntime, plan: i64) -> *const u64 {
    // SAFETY: generated code supplies the live Owner context.
    let runtime = unsafe { &mut *raw };
    let key = key(runtime, plan);
    let call = require(runtime.source_native.calls.get_mut(&key));
    check(call.phase == Phase::Prepared);
    if let Some(loan) = call.record_loan {
        valid(runtime.data.enter_native_records(&runtime.heap, loan));
    }
    if let Some(scope) = &call.return_scope {
        check(scope.enter());
    }
    if let Some(scope) = call.userdata_scope {
        runtime.userdata.open(&runtime.heap, scope);
    }
    check(call.native_scope.is_none());
    call.native_scope = Some(valid(runtime.transitions.native_scope(
        rinzu_runtime::transition::RootPublication {
            stack_map_id: runtime.data_roots.frame_count() as u64,
            root_count: runtime.data_roots.live().count(),
        },
    )));
    for callback in &call.callbacks {
        callback.open();
    }
    call.words.as_ptr()
}
pub(super) extern "C" fn returned(raw: *mut BaselineRuntime, plan: i64, result: u64) {
    // SAFETY: only generated code after physical return supplies this notification.
    let runtime = unsafe { &mut *raw };
    let key = key(runtime, plan);
    let call = require(runtime.source_native.calls.get_mut(&key));
    check(call.phase == Phase::Prepared);
    drop(require(call.native_scope.take()));
    call.result = result;
    call.phase = Phase::Returned;
    if let Some(loan) = call.record_loan {
        valid(runtime.data.return_native_records(&runtime.heap, loan));
    }
    if let Some(scope) = call.userdata_scope {
        runtime
            .userdata
            .returned(&runtime.heap, &mut runtime.data_roots, scope);
        if call.userdata_result.is_none() {
            call.userdata_scope = None;
        }
    }
    runtime.source_native.evidence.returned += 1;
}
pub(super) extern "C" fn close(raw: *mut BaselineRuntime, plan: i64) {
    // SAFETY: generated code supplies the live Owner context.
    let runtime = unsafe { &mut *raw };
    let key = key(runtime, plan);
    let call = require(runtime.source_native.calls.get_mut(&key));
    check(call.phase == Phase::Returned);
    for callback in &call.callbacks {
        check(callback.close());
    }
    call.phase = Phase::Closed;
    runtime.source_native.evidence.closed += 1;
}
pub(super) extern "C" fn settle(raw: *mut BaselineRuntime, plan: i64) {
    // SAFETY: generated code supplies the live Owner context.
    let runtime = unsafe { &mut *raw };
    let key = key(runtime, plan);
    let call = require(runtime.source_native.calls.get_mut(&key));
    check(call.phase == Phase::Closed && call.callbacks.iter().all(CallbackEntry::settled));
    if let Some(scope) = &call.return_scope {
        // SAFETY: the validated closed consumer physically returned, and all
        // callback factories/result receipts settled. No payload reader remains.
        check(unsafe { scope.consume() });
        for callback in &mut call.callbacks {
            // SAFETY: closed-consumer.v1 validates ALL callback capabilities;
            // this Origin Call physically returned and every receipt settled.
            check(unsafe { callback.retire() });
        }
    }
    if let Some(loan) = call.record_loan {
        runtime.data.commit_native_records(&mut runtime.heap, loan);
    }
    call.phase = Phase::Settled;
    runtime.source_native.evidence.settled += 1;
}
pub(super) extern "C" fn read(raw: *mut BaselineRuntime, plan: i64) -> u64 {
    // SAFETY: generated code supplies the live Owner context.
    let runtime = unsafe { &mut *raw };
    let key = key(runtime, plan);
    let call = require(runtime.source_native.calls.get_mut(&key));
    check(call.phase == Phase::Settled);
    if let SourceAbiType::Scalar(scalar) = runtime.source_native.plans[call.plan].signature.result {
        valid(rinzu_runtime::scalar::ScalarValue::from_bits(
            crate::scalar_backend::kind(scalar),
            call.result,
        ));
    }
    call.phase = Phase::Read;
    runtime.source_native.evidence.read += 1;
    if let Some(ty) = call.userdata_result {
        match runtime.userdata.receive_result(
            &runtime.heap,
            &runtime.data,
            require(call.userdata_scope),
            ty,
            call.result as *const u8,
        ) {
            Ok(handle) => {
                call.roots.push(valid(runtime.data_roots.pin(handle)));
                return handle.bits();
            }
            Err(rinzu_runtime::data::NativeUserDataError::Null) => {
                runtime.pending.fail(Diagnostic {
                    cause: FailureCause::Error(ErrorKind::NullNativeValue),
                    source: runtime.current_source,
                });
                return 0;
            }
        }
    }
    if let Some((_, reservation)) = &mut call.opaque_result {
        match runtime
            .data
            .receive_native_opaque(&mut runtime.heap, reservation, call.result)
        {
            Ok(handle) => return handle.bits(),
            Err(rinzu_runtime::data::NativeOpaqueError::Null) => {
                runtime.pending.fail(Diagnostic {
                    cause: FailureCause::Error(ErrorKind::NullNativeValue),
                    source: runtime.current_source,
                });
                return 0;
            }
            Err(_) => rinzu_runtime::native_fatal::invariant(),
        }
    }
    let result = &runtime.source_native.plans[call.plan].signature.result;
    if let Some(layout) = &call.record_result {
        // SAFETY: validated physical record result, stable until all field bytes
        // have been copied. Invalid native reads are fatal in the receiver.
        let received = unsafe {
            runtime
                .data
                .receive_native_record(&mut runtime.heap, layout, call.result as *const u8)
        };
        match received {
            Ok(handle) => {
                call.roots.push(valid(runtime.data_roots.pin(handle)));
                runtime.peak_heap_objects =
                    runtime.peak_heap_objects.max(runtime.heap.object_count());
                return handle.bits();
            }
            Err(error) => {
                let kind = match error {
                    rinzu_runtime::data::NativeRecordError::Null => ErrorKind::NullNativeValue,
                    rinzu_runtime::data::NativeRecordError::Conversion => ErrorKind::Conversion,
                    _ => rinzu_runtime::native_fatal::invariant(),
                };
                runtime.pending.fail(Diagnostic {
                    cause: FailureCause::Error(kind),
                    source: runtime.current_source,
                });
                return 0;
            }
        }
    }
    if matches!(
        result,
        SourceAbiType::String | SourceAbiType::Bytes(_) | SourceAbiType::ByteArrayResult
    ) {
        let pointer = call.result as usize as *const u8;
        // SAFETY: the validated physical result is one pointer. The selected
        // receiver copies through NUL, the fixed N, or the descriptor's length.
        let received = unsafe {
            match result {
                SourceAbiType::String => runtime
                    .data
                    .receive_native_string(&mut runtime.heap, pointer),
                SourceAbiType::Bytes(length) => {
                    runtime
                        .data
                        .receive_native_bytes(&mut runtime.heap, *length, pointer)
                }
                SourceAbiType::ByteArrayResult => runtime
                    .data
                    .receive_native_byte_array(&mut runtime.heap, pointer.cast()),
                _ => unreachable!(),
            }
        };
        match received {
            Ok(handle) => {
                call.roots.push(valid(runtime.data_roots.pin(handle)));
                runtime.peak_heap_objects =
                    runtime.peak_heap_objects.max(runtime.heap.object_count());
                return handle.bits();
            }
            Err(error) => {
                let kind = match error {
                    rinzu_runtime::data::NativeInputError::NullString
                    | rinzu_runtime::data::NativeInputError::NullByteArray => {
                        ErrorKind::NullNativeValue
                    }
                    rinzu_runtime::data::NativeInputError::InvalidUtf8 => ErrorKind::InvalidUtf8,
                    _ => rinzu_runtime::native_fatal::invariant(),
                };
                runtime.pending.fail(Diagnostic {
                    cause: FailureCause::Error(kind),
                    source: runtime.current_source,
                });
                return 0;
            }
        }
    }
    call.result
}
pub(super) extern "C" fn release(raw: *mut BaselineRuntime, plan: i64) {
    // SAFETY: generated code supplies the live Owner context.
    let runtime = unsafe { &mut *raw };
    let key = key(runtime, plan);
    let call = require(runtime.source_native.calls.remove(&key));
    check(call.phase == Phase::Read);
    if let Some(scope) = call.userdata_scope {
        check(call.userdata_result.is_some());
        runtime
            .userdata
            .finish_result(&runtime.heap, &mut runtime.data_roots, scope);
    }
    if let Some(loan) = call.record_loan {
        valid(runtime.data.release_native_records(&mut runtime.heap, loan));
    }
    for lease in &call.roots {
        valid(runtime.data_roots.unpin(*lease));
    }
    let evidence = &mut runtime.source_native.evidence;
    evidence.released += 1;
    evidence.live_calls -= 1;
    evidence.live_buffers -= call.buffers.len() + call.record_count;
    evidence.live_callbacks -= call.callbacks.len();
    drop(call);
}
