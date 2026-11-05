use std::collections::{BTreeMap, HashMap, HashSet};

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use core::ffi::c_void;

use anyhow::{Context, Result, anyhow, bail, ensure};
use rinzu_abi::{ABI_VERSION, NativeInvocationContextV1};
use rinzu_compiler::{
    BasicBlock, BinaryOperator, BlockId, ConstantValue, IrFunction, IrModule, OperationKind,
    StackRoot, Terminator, UnaryOperator, ValueId,
};
use rinzu_runtime::code::{CodeFrame, CodeVersion, CodeVersionId, CodeVersionState};
use rinzu_runtime::data::{DataError, ManagedData, ManagedHandle, ManagedRoots, ManagedSlot};
use rinzu_runtime::dispatch::VersionedDispatchCell;
use rinzu_runtime::failure::{
    Diagnostic, ErrorKind, Failure, FailureCause, PendingFailure, SourcePosition,
};
use rinzu_runtime::heap::{CollectionKind, DomainHeap, HeapError};
use std::sync::Arc;

use crate::{CompileEvidence, LoadedNativePackages};
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod native;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use native::NativeLedger;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod errors;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BaselineFailure {
    Arithmetic,
    Conversion,
    Error(ErrorKind),
    Cancelled,
    Overloaded,
    Shutdown,
    Unsupported,
    RuntimePanic,
    Panic,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BaselineOptions {
    pub capture_stdout: bool,
    pub capture_error_trace: bool,
    pub collect_every_allocation: bool,
    pub cancel_after_polls: Option<usize>,
    pub max_allocations: Option<usize>,
    pub shutdown_after_polls: Option<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaselineRun {
    pub stdout: Vec<u8>,
    pub result: std::result::Result<i64, BaselineFailure>,
    pub failure_details: Option<Failure>,
    pub source_files: BTreeMap<u64, String>,
    pub caught_errors: Vec<Failure>,
    pub root_publications: usize,
    pub peak_root_frames: usize,
    pub function_trace: Vec<String>,
    pub poll_count: usize,
    pub allocation_count: usize,
    pub collection_count: usize,
    pub heap_objects: usize,
    pub peak_heap_objects: usize,
    pub dispatch_resolutions: usize,
    pub callback_dispatches: usize,
    pub dispatch_versions: Vec<u64>,
    pub published_code_versions: usize,
    pub retired_code_versions: usize,
    pub peak_live_code_frames: usize,
    pub executable_mappings: usize,
    pub icache_flushes: usize,
    pub rx_transitions: usize,
    pub rwx_mappings: usize,
    pub compile: CompileEvidence,
    pub memory_policy: &'static str,
    pub host_trace: Option<rinzu_trace::TraceDocument>,
    pub native_calls: crate::SourceNativeCallEvidence,
    pub native_process: crate::NativeProcessEvidence,
    pub native_userdata: rinzu_runtime::data::NativeUserDataEvidence,
    pub native_record_scratch: rinzu_runtime::data::NativeRecordScratchStats,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AotFunctionMetadata {
    pub source: SourcePosition,
    pub index: usize,
    pub name: String,
    pub symbol: String,
    pub parameter_count: usize,
    pub parameter_types: Vec<rinzu_compiler::Type>,
    pub result_type: rinzu_compiler::Type,
    pub is_async: bool,
    pub callback_fallback: Option<rinzu_compiler::FallbackConstant>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AotDataType {
    pub ty: rinzu_compiler::Type,
    pub index: usize,
    pub managed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaselineAotObject {
    pub object: Vec<u8>,
    /// Whole synchronous functions compiled with the generated C runtime.
    pub c_function_source: String,
    pub c_functions: Vec<String>,
    pub c_fallbacks: BTreeMap<String, String>,
    pub entry_index: usize,
    pub entry_symbol: String,
    pub functions: Vec<AotFunctionMetadata>,
    pub source_files: BTreeMap<u64, String>,
    pub source_locations_c: String,
    /// Private typed data catalog emitted from the same IR as the native object.
    pub data_schema_c: String,
    pub data_types: Vec<AotDataType>,
    /// Handle n+1 identifies this exact target; never a raw native address.
    pub reference_targets: Vec<String>,
    pub native_call_plans: Vec<crate::NativeCallPlan>,
    pub native_signatures: Vec<rinzu_compiler::SourceNativeSignature>,
    pub native_record_schemas: BTreeMap<String, Vec<(String, rinzu_compiler::AbiScalar)>>,
    pub compile: CompileEvidence,
}

impl BaselineAotObject {
    pub fn data_type(&self, ty: &rinzu_compiler::Type) -> Option<&AotDataType> {
        let ty = crate::data_schema::canonical(ty);
        self.data_types.iter().find(|entry| entry.ty == ty)
    }
}

/// Compiled async managed code uses the same word ABI and verified stack maps
/// as AOT. The host must keep this module alive through all continuations and
/// callback admissions, then retire it only after runtime settlement.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub struct CompiledAsyncJit {
    module: Option<cranelift_jit::JITModule>,
    pub symbols: BTreeMap<String, usize>,
    pub compile: CompileEvidence,
    pub executable_mappings: usize,
    pub executable_bytes: usize,
    pub icache_flushes: usize,
    pub rx_transitions: usize,
    pub rwx_mappings: usize,
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
impl CompiledAsyncJit {
    /// # Safety
    /// Every continuation, Worker and callback using these entry addresses
    /// must have returned and released its code pins before retirement.
    pub unsafe fn retire(mut self) {
        if let Some(module) = self.module.take() {
            // SAFETY: the caller proves runtime settlement for all entries.
            unsafe { module.free_memory() };
        }
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub fn compile_source_async_jit(
    module: &IrModule,
    external_symbols: &BTreeMap<String, usize>,
    native_imports: &HashMap<String, u64>,
) -> Result<CompiledAsyncJit> {
    crate::ensure_source_async_backend(module)?;
    target::compile_async_jit(module, external_symbols, native_imports)
}

pub(crate) fn source_identities(module: &IrModule) -> (HashMap<u32, u64>, BTreeMap<u64, String>) {
    let mut files: Vec<_> = module.source_files.iter().collect();
    files.sort_by(|left, right| left.1.cmp(right.1));
    let mut identities = HashMap::new();
    let mut names = BTreeMap::new();
    for (index, (parse_id, name)) in files.into_iter().enumerate() {
        let identity = index as u64 + 1;
        identities.insert(*parse_id, identity);
        names.insert(identity, name.clone());
    }
    (identities, names)
}

struct BaselineRuntime {
    stdout: rinzu_runtime::stdio::Output,
    transitions: Arc<rinzu_runtime::transition::TransitionMachine>,
    heap: DomainHeap,
    data_schema: crate::data_schema::DataSchema,
    pending: PendingFailure,
    error_groups: rinzu_runtime::error_groups::ErrorGroups,
    current_source: Option<SourcePosition>,
    caught_errors: Vec<Failure>,
    data: ManagedData,
    data_roots: ManagedRoots,
    root_publications: usize,
    peak_root_frames: usize,
    return_roots: BTreeMap<usize, rinzu_runtime::data::RootLease>,
    function_names: Vec<String>,
    function_trace: Vec<String>,
    poll_count: usize,
    allocation_count: usize,
    collection_count: usize,
    cancel_requested: bool,
    callback_depth: usize,
    shutdown_requested: bool,
    cancel_after_polls: Option<usize>,
    max_allocations: Option<usize>,
    shutdown_after_polls: Option<usize>,
    collect_every_allocation: bool,
    peak_heap_objects: usize,
    dispatch_cells: Vec<VersionedDispatchCell>,
    dispatch_resolutions: usize,
    callback_dispatches: usize,
    code_versions: HashMap<(usize, u64), Arc<CodeVersion>>,
    managed_frames: Vec<CodeFrame>,
    trace_frames: Vec<rinzu_runtime::error_trace::TraceGuard>,
    host_trace: Option<rinzu_trace::TraceDocument>,
    native_packages: Option<LoadedNativePackages>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    source_native: NativeLedger,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    userdata: rinzu_runtime::data::NativeUserDataRegistry,
}

impl BaselineRuntime {
    fn new(
        function_names: Vec<String>,
        options: BaselineOptions,
        native_packages: Option<LoadedNativePackages>,
        data_schema: crate::data_schema::DataSchema,
    ) -> Self {
        let mut heap = DomainHeap::new(0x4a49_5401);
        if options.collect_every_allocation {
            heap.enable_gc_stress();
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let userdata = rinzu_runtime::data::NativeUserDataRegistry::new(&heap);
        let mut data = ManagedData::new(0x4a49_5401);
        data.configure_values(&heap, data_schema.value.clone())
            .expect("fresh Owner data accepts a validated schema");
        let mut pending = PendingFailure::default();
        pending.set_tracer(rinzu_runtime::error_trace::TraceContext::from_environment(
            options.capture_error_trace,
        ));
        let dispatch_cells = (0..function_names.len())
            .map(|_| VersionedDispatchCell::new())
            .collect();
        let transitions = Arc::new(rinzu_runtime::transition::TransitionMachine::new());
        transitions
            .activate_managed()
            .expect("new Owner is inactive");
        Self {
            stdout: rinzu_runtime::stdio::Output::new(options.capture_stdout),
            transitions,
            heap,
            data_schema,
            pending,
            error_groups: Default::default(),
            current_source: None,
            caught_errors: Vec::new(),
            data,
            data_roots: ManagedRoots::default(),
            root_publications: 0,
            peak_root_frames: 0,
            return_roots: BTreeMap::new(),
            function_names,
            function_trace: Vec::new(),
            poll_count: 0,
            allocation_count: 0,
            collection_count: 0,
            cancel_requested: false,
            callback_depth: 0,
            shutdown_requested: false,
            cancel_after_polls: options.cancel_after_polls,
            max_allocations: options.max_allocations,
            shutdown_after_polls: options.shutdown_after_polls,
            collect_every_allocation: options.collect_every_allocation,
            peak_heap_objects: 0,
            dispatch_cells,
            dispatch_resolutions: 0,
            callback_dispatches: 0,
            code_versions: HashMap::new(),
            managed_frames: Vec::new(),
            trace_frames: Vec::new(),
            host_trace: None,
            native_packages,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            source_native: NativeLedger::default(),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            userdata,
        }
    }

    fn fail(&mut self, failure: BaselineFailure) {
        let cause = match failure {
            BaselineFailure::Arithmetic => FailureCause::ArithmeticTrap,
            BaselineFailure::Conversion => FailureCause::Error(ErrorKind::Conversion),
            BaselineFailure::Error(kind) => FailureCause::Error(kind),
            BaselineFailure::Cancelled => FailureCause::Cancelled,
            BaselineFailure::Overloaded => FailureCause::Overloaded,
            BaselineFailure::Shutdown => FailureCause::Shutdown,
            BaselineFailure::Unsupported => FailureCause::Unsupported,
            BaselineFailure::RuntimePanic => FailureCause::RuntimePanic,
            BaselineFailure::Panic => FailureCause::Panic,
        };
        self.pending.fail(Diagnostic {
            cause,
            source: self.current_source,
        });
    }

    fn data_result<T>(&mut self, result: std::result::Result<T, DataError>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(DataError::Panic(message)) => {
                self.fail(BaselineFailure::Panic);
                let native = self
                    .transitions
                    .native_scope(rinzu_runtime::transition::RootPublication {
                        stack_map_id: self.data_roots.frame_count() as u64,
                        root_count: self.data_roots.live().count(),
                    })
                    .unwrap_or_else(|_| std::process::abort());
                rinzu_runtime::stdio::write_panic(&message);
                drop(native);
                None
            }
            Err(DataError::Thrown(value)) => {
                let handle = self.data.create_error_value(&mut self.heap, &value);
                if let Some(handle) = self.data_result(handle) {
                    let thrown = errors::throw_error(self, handle);
                    self.data_result(thrown);
                }
                None
            }
            Err(DataError::CollectionOverflow {
                operation,
                current,
                added,
            }) => {
                let handle = self.data_schema.collection_overflow(
                    &mut self.data,
                    &mut self.heap,
                    operation,
                    current,
                    added,
                );
                if let Some(handle) = self.data_result(handle) {
                    let thrown = errors::throw_error(self, handle);
                    self.data_result(thrown);
                }
                None
            }
            Err(
                DataError::AddressSpaceExhausted
                | DataError::Heap(HeapError::AddressSpaceExhausted),
            ) => {
                // Known allocation failure cannot enter managed cleanup or fallback.
                rinzu_runtime::allocation::fatal();
            }
            Err(_) => {
                self.fail(BaselineFailure::RuntimePanic);
                None
            }
        }
    }

    fn data_allocated(&mut self, count: usize) {
        self.allocation_count += count;
        self.peak_heap_objects = self.peak_heap_objects.max(self.heap.object_count());
        if self.collect_every_allocation {
            let result = self
                .heap
                .collect(CollectionKind::Minor)
                .map_err(DataError::from);
            if self.data_result(result).is_some() {
                self.collection_count += 1;
            }
        }
    }
}

// The emitter checks both 32-bit offsets and the file id before packing.
extern "C" fn baseline_set_source(context: *mut BaselineRuntime, file: u64, span: u64) {
    with_runtime(context, (), |runtime| {
        runtime.current_source = Some(SourcePosition {
            file,
            start_byte: span as u32,
            end_byte: (span >> 32) as u32,
        });
        runtime.pending.tracer().source(runtime.current_source);
    });
}

extern "C" fn baseline_observe_source(context: *mut BaselineRuntime) {
    with_runtime(context, (), |runtime| {
        runtime.pending.tracer().source(runtime.current_source);
    });
}

fn with_runtime<T: Copy>(
    context: *mut BaselineRuntime,
    fallback: T,
    operation: impl FnOnce(&mut BaselineRuntime) -> T,
) -> T {
    if context.is_null() {
        return fallback;
    }
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: generated code receives the live context pointer for this call.
        operation(unsafe { &mut *context })
    }))
    .unwrap_or_else(|_| {
        // SAFETY: the pointer was checked above and remains live for the JIT call.
        unsafe { &mut *context }.fail(BaselineFailure::RuntimePanic);
        fallback
    })
}

extern "C" fn baseline_poll(context: *mut BaselineRuntime, _site: i64) -> i64 {
    with_runtime(context, 1, |runtime| {
        runtime.poll_count += 1;
        if runtime.pending.cleanup_depth() != 0 {
            return 0;
        }
        if runtime
            .shutdown_after_polls
            .is_some_and(|limit| runtime.poll_count >= limit)
        {
            runtime.shutdown_requested = true;
        }
        if runtime
            .cancel_after_polls
            .is_some_and(|limit| runtime.poll_count >= limit)
        {
            runtime.cancel_requested = true;
        }
        // A synchronous admitted callback records parent cancellation without
        // throwing it through the native frame. Ordinary GC polls still run.
        if runtime.callback_depth != 0 {
            return i64::from(runtime.pending.current().is_some());
        }
        if runtime.shutdown_requested {
            runtime.fail(BaselineFailure::Shutdown);
            return 1;
        }
        if runtime.cancel_requested {
            runtime.fail(BaselineFailure::Cancelled);
            1
        } else if runtime.pending.current().is_some() {
            1
        } else {
            0
        }
    })
}

extern "C" fn baseline_has_failure(context: *mut BaselineRuntime) -> i64 {
    with_runtime(context, 1, |runtime| {
        i64::from(runtime.pending.current().is_some())
    })
}

extern "C" fn baseline_has_error(context: *mut BaselineRuntime) -> i64 {
    with_runtime(context, 0, |runtime| {
        i64::from(runtime.pending.current().is_some_and(Failure::recoverable))
    })
}
extern "C" fn baseline_catch_error(context: *mut BaselineRuntime) -> i64 {
    with_runtime(context, 0, |runtime| {
        if let Some(error) = runtime.pending.observe_error() {
            let value = runtime.data.create_error(&mut runtime.heap, &error);
            let Some(value) = runtime.data_result(value) else {
                return 0;
            };
            runtime.caught_errors.push(error.diagnostics_only());
            runtime.data_allocated(2);
            value.bits() as i64
        } else {
            runtime.fail(BaselineFailure::Unsupported);
            0
        }
    })
}

extern "C" fn baseline_string(context: *mut BaselineRuntime, bytes: *const u8, length: i64) -> i64 {
    with_runtime(context, 0, |runtime| {
        let Ok(length) = usize::try_from(length) else {
            runtime.fail(BaselineFailure::RuntimePanic);
            return 0;
        };
        if length > isize::MAX as usize || (length != 0 && bytes.is_null()) {
            runtime.fail(BaselineFailure::RuntimePanic);
            return 0;
        }
        let bytes = if length == 0 {
            &[][..]
        } else {
            // SAFETY: lowering passes immutable UTF-8 module data with this length.
            unsafe { std::slice::from_raw_parts(bytes, length) }
        };
        let Ok(text) = std::str::from_utf8(bytes) else {
            runtime.fail(BaselineFailure::RuntimePanic);
            return 0;
        };
        let value = runtime.data.create_string(&mut runtime.heap, text);
        let Some(value) = runtime.data_result(value) else {
            return 0;
        };
        runtime.data_allocated(1);
        value.bits() as i64
    })
}

extern "C" fn baseline_error_message(context: *mut BaselineRuntime, error: i64) -> i64 {
    with_runtime(context, 0, |runtime| {
        let Some(error) = ManagedHandle::from_bits(error as u64) else {
            runtime.fail(BaselineFailure::RuntimePanic);
            return 0;
        };
        let message = runtime.data.error_message(&mut runtime.heap, error);
        runtime
            .data_result(message)
            .map_or(0, |value| value.bits() as i64)
    })
}

extern "C" fn baseline_format_string(
    context: *mut BaselineRuntime,
    bits: *const u64,
    types: *const u64,
    count: i64,
) -> i64 {
    with_runtime(context, 0, |runtime| {
        let Ok(count) = usize::try_from(count) else {
            runtime.fail(BaselineFailure::RuntimePanic);
            return 0;
        };
        if count > isize::MAX as usize / 8 || (count != 0 && (bits.is_null() || types.is_null())) {
            runtime.fail(BaselineFailure::RuntimePanic);
            return 0;
        }
        let (bits, types) = if count == 0 {
            (&[][..], &[][..])
        } else {
            // SAFETY: verified lowering supplies two initialized u64 stack arrays
            // of count elements, alive for the duration of this synchronous call.
            unsafe {
                (
                    std::slice::from_raw_parts(bits, count),
                    std::slice::from_raw_parts(types, count),
                )
            }
        };
        let mut parts = Vec::with_capacity(count);
        for (&bits, &ty) in bits.iter().zip(types) {
            let Ok(ty) = usize::try_from(ty) else {
                runtime.fail(BaselineFailure::RuntimePanic);
                return 0;
            };
            let slot = runtime.data_schema.slot(ty, bits);
            let Some(slot) = runtime.data_result(slot) else {
                return 0;
            };
            parts.push((ty, slot));
        }
        let result =
            runtime
                .data
                .format_string(&mut runtime.heap, &runtime.data_schema.display, &parts);
        let Some(value) = runtime.data_result(result) else {
            return 0;
        };
        runtime.data_allocated(1);
        value.bits() as i64
    })
}

extern "C" fn baseline_stdout(
    context: *mut BaselineRuntime,
    bits: i64,
    ty: i64,
    newline: i64,
) -> i64 {
    with_runtime(context, 0, |runtime| {
        let Ok(ty) = usize::try_from(ty) else {
            runtime.fail(BaselineFailure::RuntimePanic);
            return 0;
        };
        if !matches!(newline, 0 | 1) {
            runtime.fail(BaselineFailure::RuntimePanic);
            return 0;
        }
        let slot = runtime.data_schema.slot(ty, bits as u64);
        let Some(slot) = runtime.data_result(slot) else {
            return 0;
        };
        let bytes = runtime
            .data
            .display(&runtime.heap, &runtime.data_schema.display, ty, slot);
        let Some(bytes) = runtime.data_result(bytes) else {
            return 0;
        };
        let native = runtime
            .transitions
            .native_scope(rinzu_runtime::transition::RootPublication {
                stack_map_id: runtime.data_roots.frame_count() as u64,
                root_count: runtime.data_roots.live().count(),
            })
            .unwrap_or_else(|_| std::process::abort());
        let result = runtime.stdout.write(&bytes, newline != 0);
        drop(native);
        if result.is_err() {
            runtime.fail(BaselineFailure::Error(ErrorKind::Io));
        }
        0
    })
}

extern "C" fn baseline_task_control(
    context: *mut BaselineRuntime,
    _kind: i64,
    _future: i64,
) -> i64 {
    with_runtime(context, 0, |runtime| {
        runtime.fail(BaselineFailure::Unsupported);
        0
    })
}

extern "C" fn baseline_utf8_length(context: *mut BaselineRuntime, string: i64) -> i64 {
    with_runtime(context, 0, |runtime| {
        let Some(string) = ManagedHandle::from_bits(string as u64) else {
            runtime.fail(BaselineFailure::RuntimePanic);
            return 0;
        };
        let length = runtime.data.string_length(&runtime.heap, string);
        runtime
            .data_result(length)
            .map_or(0, |length| length as i64)
    })
}

extern "C" fn baseline_string_compare(
    context: *mut BaselineRuntime,
    operation: i64,
    left: i64,
    right: i64,
) -> i64 {
    with_runtime(context, 0, |runtime| {
        let handles =
            ManagedHandle::from_bits(left as u64).zip(ManagedHandle::from_bits(right as u64));
        let Some((left, right)) = handles.filter(|_| operation == 0 || operation == 1) else {
            runtime.fail(BaselineFailure::RuntimePanic);
            return 0;
        };
        let equal = runtime.data.strings_equal(&runtime.heap, left, right);
        runtime
            .data_result(equal)
            .map_or(0, |equal| i64::from(equal ^ (operation == 1)))
    })
}

extern "C" fn baseline_scalar_convert(
    context: *mut BaselineRuntime,
    source: i64,
    target: i64,
    bits: i64,
) -> i64 {
    use rinzu_runtime::scalar::{ScalarKind, ScalarValue};
    with_runtime(context, 0, |runtime| {
        let converted = u32::try_from(source)
            .ok()
            .and_then(ScalarKind::from_tag)
            .zip(u32::try_from(target).ok().and_then(ScalarKind::from_tag))
            .and_then(|(source, target)| {
                ScalarValue::from_bits(source, bits as u64)
                    .ok()
                    .map(|value| value.convert(target))
            });
        match converted {
            Some(Ok(value)) => value.bits() as i64,
            Some(Err(_)) => {
                runtime.fail(BaselineFailure::Conversion);
                0
            }
            None => {
                runtime.fail(BaselineFailure::Unsupported);
                0
            }
        }
    })
}

extern "C" fn baseline_begin_cleanup(context: *mut BaselineRuntime, region: i64) {
    with_runtime(context, (), |runtime| {
        runtime
            .pending
            .begin_cleanup_scoped(runtime.data_roots.frame_count() as u64, region as u64);
    });
}

extern "C" fn baseline_end_cleanup(context: *mut BaselineRuntime, region: i64) {
    with_runtime(context, (), |runtime| {
        if runtime
            .pending
            .end_cleanup_scoped(runtime.data_roots.frame_count() as u64, region as u64)
            .is_err()
        {
            runtime.fail(BaselineFailure::RuntimePanic);
        }
    });
}

extern "C" fn baseline_resolve_dispatch(context: *mut BaselineRuntime, function: i64) -> i64 {
    with_runtime(context, 0, |runtime| {
        let Some(function) = usize::try_from(function).ok() else {
            runtime.fail(BaselineFailure::Unsupported);
            return 0;
        };
        let Some(cell) = runtime.dispatch_cells.get(function) else {
            runtime.fail(BaselineFailure::Unsupported);
            return 0;
        };
        let Ok(target) = cell.load() else {
            runtime.fail(BaselineFailure::Unsupported);
            return 0;
        };
        let Some(version) = runtime.code_versions.get(&(function, target.version)) else {
            runtime.fail(BaselineFailure::Unsupported);
            return 0;
        };
        let Ok(frame) = version.enter() else {
            runtime.fail(BaselineFailure::Unsupported);
            return 0;
        };
        runtime.managed_frames.push(frame);
        runtime.dispatch_resolutions += 1;
        i64::try_from(target.address).unwrap_or_else(|_| {
            runtime.fail(BaselineFailure::Unsupported);
            0
        })
    })
}

extern "C" fn baseline_leave_dispatch(context: *mut BaselineRuntime) {
    with_runtime(context, (), |runtime| {
        if runtime.pending.tracer().enabled() && runtime.trace_frames.pop().is_none() {
            std::process::abort();
        }
        runtime.release_return_root();

        if runtime.data_roots.leave_frame().is_err() {
            runtime.fail(BaselineFailure::RuntimePanic);
        }
        if runtime.managed_frames.pop().is_none() {
            runtime.fail(BaselineFailure::RuntimePanic);
        }
    });
}

impl BaselineRuntime {
    fn release_return_root(&mut self) {
        if let Some(lease) = self.return_roots.remove(&self.data_roots.frame_count()) {
            self.data_roots
                .unpin(lease)
                .unwrap_or_else(|_| std::process::abort());
        }
    }
}
extern "C" fn baseline_retain_return(context: *mut BaselineRuntime, bits: u64) {
    with_runtime(context, (), |runtime| {
        let handle = ManagedHandle::from_bits(bits).unwrap_or_else(|| std::process::abort());
        let lease = runtime
            .data_roots
            .pin(handle)
            .unwrap_or_else(|_| std::process::abort());
        if runtime
            .return_roots
            .insert(runtime.data_roots.frame_count(), lease)
            .is_some()
        {
            std::process::abort();
        }
    });
}

extern "C" fn baseline_function_entry(context: *mut BaselineRuntime, function: i64) -> i64 {
    with_runtime(context, 0, |runtime| {
        let Some(name) = usize::try_from(function)
            .ok()
            .and_then(|index| runtime.function_names.get(index))
        else {
            runtime.fail(BaselineFailure::Unsupported);
            return 0;
        };
        runtime.function_trace.push(name.clone());
        if runtime.pending.tracer().enabled() {
            runtime
                .trace_frames
                .push(runtime.pending.tracer().enter(name, None));
        }
        runtime.data_roots.enter_frame();
        runtime.peak_root_frames = runtime
            .peak_root_frames
            .max(runtime.data_roots.frame_count());
        // Rust reference emission keeps its typed source/tracer helpers.
        0
    })
}

extern "C" fn baseline_publish_roots(
    context: *mut BaselineRuntime,
    roots: *const u64,
    count: i64,
) -> i64 {
    with_runtime(context, 1, |runtime| {
        let Ok(count) = usize::try_from(count) else {
            runtime.fail(BaselineFailure::Unsupported);
            return 1;
        };
        if count > isize::MAX as usize / core::mem::size_of::<u64>()
            || (count != 0 && roots.is_null())
        {
            runtime.fail(BaselineFailure::Unsupported);
            return 1;
        }
        let bits = if count == 0 {
            &[][..]
        } else {
            // SAFETY: generated code passes an aligned stack array with this verified length.
            unsafe { std::slice::from_raw_parts(roots, count) }
        };
        if runtime
            .data_roots
            .publish(
                bits.iter()
                    .filter_map(|bits| ManagedHandle::from_bits(*bits)),
            )
            .is_err()
        {
            runtime.fail(BaselineFailure::RuntimePanic);
            return 1;
        }
        baseline_roots_safepoint(runtime)
    })
}

extern "C" fn baseline_prepare_roots(context: *mut BaselineRuntime, count: i64) -> *mut u64 {
    if context.is_null() {
        // SAFETY: a broken compiler/runtime protocol cannot resume with a null buffer.
        unsafe { libc::_exit(70) }
    }
    // SAFETY: generated code holds this live, exclusively owned runtime context.
    let runtime = unsafe { &mut *context };
    let Ok(count) = usize::try_from(count) else {
        // SAFETY: invalid compiler-produced root counts are fatal.
        unsafe { libc::_exit(70) }
    };
    if runtime.data.validate_roots(&runtime.heap, []).is_err() {
        // SAFETY: roots must be written by the Domain's Owner.
        unsafe { libc::_exit(70) }
    }
    runtime
        .data_roots
        .prepare_publication(count)
        .unwrap_or_else(|_| {
            // SAFETY: never return an invalid buffer to unchecked generated stores.
            unsafe { libc::_exit(70) }
        })
}

extern "C" fn baseline_commit_roots(context: *mut BaselineRuntime, count: i64) -> i64 {
    with_runtime(context, 1, |runtime| {
        let Ok(count) = usize::try_from(count) else {
            // SAFETY: a broken private publication protocol cannot be recovered.
            unsafe { libc::_exit(70) }
        };
        if runtime.data_roots.commit_publication(count).is_err() {
            // SAFETY: collection is forbidden while a publication is incomplete.
            unsafe { libc::_exit(70) }
        }
        baseline_roots_safepoint(runtime)
    })
}

extern "C" fn baseline_publish_roots_poll(
    context: *mut BaselineRuntime,
    roots: *const u64,
    count: i64,
) -> i64 {
    baseline_publish_roots(context, roots, count);
    baseline_poll(context, 0);
    baseline_has_failure(context)
}

extern "C" fn baseline_commit_roots_poll(context: *mut BaselineRuntime, count: i64) -> i64 {
    baseline_commit_roots(context, count);
    baseline_poll(context, 0);
    baseline_has_failure(context)
}

fn baseline_roots_safepoint(runtime: &mut BaselineRuntime) -> i64 {
    let had_data = runtime.data.root_count() != 0;
    if runtime
        .data
        .validate_roots(&runtime.heap, runtime.data_roots.current_live())
        .is_err()
    {
        runtime.fail(BaselineFailure::RuntimePanic);
        return 1;
    }
    if had_data && runtime.heap.collection_due() {
        if runtime
            .data
            .retain_roots(&mut runtime.heap, runtime.data_roots.live())
            .is_err()
        {
            runtime.fail(BaselineFailure::RuntimePanic);
            return 1;
        }
        if runtime.heap.collect(CollectionKind::Minor).is_err() {
            runtime.fail(BaselineFailure::RuntimePanic);
            return 1;
        }
        runtime.collection_count += 1;
    }
    runtime.root_publications += 1;
    0
}

// Keep the reference backend's stress collection and allocation accounting
// equivalent to a non-allocating DataOperation after extracting its scalar.
fn baseline_length(
    context: *mut BaselineRuntime,
    operation: impl FnOnce(&mut BaselineRuntime) -> std::result::Result<i64, DataError>,
) -> i64 {
    with_runtime(context, 0, |runtime| {
        let outcome = operation(runtime);
        let value = runtime.data_result(outcome);
        runtime.data_allocated(0);
        value.unwrap_or(0)
    })
}
extern "C" fn baseline_bytes_int_length(context: *mut BaselineRuntime, bits: i64) -> i64 {
    baseline_length(context, |runtime| {
        let handle = ManagedHandle::from_bits(bits as u64).ok_or(DataError::InvalidEncoding)?;
        let length = runtime.data.byte_length(&runtime.heap, handle, u64::MAX)?;
        i64::try_from(length).map_err(|_| DataError::AddressSpaceExhausted)
    })
}
extern "C" fn baseline_string_scalar_length(context: *mut BaselineRuntime, bits: i64) -> i64 {
    baseline_length(context, |runtime| {
        let handle = ManagedHandle::from_bits(bits as u64).ok_or(DataError::InvalidEncoding)?;
        let text = runtime.data.string(&runtime.heap, handle)?;
        let length = if text.is_ascii() {
            text.len()
        } else {
            text.chars().count()
        };
        i64::try_from(length).map_err(|_| DataError::AddressSpaceExhausted)
    })
}
extern "C" fn baseline_collection_length(context: *mut BaselineRuntime, bits: i64, ty: i64) -> i64 {
    baseline_length(context, |runtime| {
        let handle = ManagedHandle::from_bits(bits as u64).ok_or(DataError::InvalidEncoding)?;
        let ty = usize::try_from(ty).map_err(|_| DataError::InvalidEncoding)?;
        let length = runtime.data.collection_length(&runtime.heap, handle, ty)?;
        i64::try_from(length).map_err(|_| DataError::AddressSpaceExhausted)
    })
}

extern "C" fn baseline_data_operation(
    context: *mut BaselineRuntime,
    op: i64,
    result: i64,
    types: *const u64,
    bits: *const u64,
    count: i64,
) -> i64 {
    with_runtime(context, 0, |runtime| {
        let before = runtime.heap.object_count();
        let mut logical = None;
        let outcome = (|| {
            let op = u32::try_from(op)
                .ok()
                .and_then(rinzu_compiler::DataOperation::from_code)
                .ok_or(DataError::InvalidEncoding)?;
            let count = usize::try_from(count).map_err(|_| DataError::InvalidEncoding)?;
            if count != op.arity() || bits.is_null() || types.is_null() {
                return Err(DataError::InvalidEncoding);
            }
            // SAFETY: generated code supplies two aligned arrays with the verified arity.
            let (raw_types, bits) = unsafe {
                (
                    std::slice::from_raw_parts(types, count),
                    std::slice::from_raw_parts(bits, count),
                )
            };
            let mut types = [0; 3];
            let mut slots = [rinzu_runtime::data::ManagedSlot::Unit; 3];
            for index in 0..count {
                types[index] =
                    usize::try_from(raw_types[index]).map_err(|_| DataError::InvalidEncoding)?;
                slots[index] = runtime.data_schema.slot(types[index], bits[index])?;
            }
            let output = runtime.data.operation(
                &mut runtime.heap,
                op,
                usize::try_from(result).map_err(|_| DataError::InvalidEncoding)?,
                &types[..count],
                &slots[..count],
            )?;
            use rinzu_compiler::DataOperation::*;
            logical = match op {
                ListBufferAppend | MapBufferInsert => Some(1),
                CollectionFinish => Some(0),
                _ => None,
            };
            Ok(output)
        })();
        let slot = runtime.data_result(outcome);
        runtime.data_allocated(logical.unwrap_or_else(|| runtime.heap.object_count() - before));
        let Some(slot) = slot else {
            return 0;
        };
        match slot {
            rinzu_runtime::data::ManagedSlot::Scalar(value) => value.bits() as i64,
            rinzu_runtime::data::ManagedSlot::Reference(handle) => handle.bits() as i64,
            rinzu_runtime::data::ManagedSlot::Unit => 0,
            rinzu_runtime::data::ManagedSlot::Opaque(bits) => bits as i64,
        }
    })
}

extern "C" fn baseline_construct(
    context: *mut BaselineRuntime,
    type_id: i64,
    variant: i64,
    bits: *const u64,
    field_count: i64,
) -> i64 {
    with_runtime(context, 0, |runtime| {
        if runtime
            .max_allocations
            .is_some_and(|limit| runtime.allocation_count >= limit)
        {
            runtime.fail(BaselineFailure::Overloaded);
            return 0;
        }
        let result = (|| {
            let type_id = usize::try_from(type_id).map_err(|_| DataError::InvalidEncoding)?;
            let count = usize::try_from(field_count).map_err(|_| DataError::InvalidEncoding)?;
            let variant = if variant == -1 {
                None
            } else {
                Some(u32::try_from(variant).map_err(|_| DataError::InvalidEncoding)?)
            };
            if count > isize::MAX as usize / size_of::<u64>() || (count != 0 && bits.is_null()) {
                return Err(DataError::InvalidEncoding);
            }
            let types = runtime.data_schema.child_types(type_id, variant, count)?;
            let bits = if count == 0 {
                &[][..]
            } else {
                // SAFETY: generated code supplies an aligned stack array of this length.
                unsafe { std::slice::from_raw_parts(bits, count) }
            };
            let slots = types
                .into_iter()
                .zip(bits)
                .map(|(ty, bits)| runtime.data_schema.slot(ty, *bits))
                .collect::<std::result::Result<Vec<_>, _>>()?;
            runtime
                .data
                .create_composite(&mut runtime.heap, type_id, variant, &slots)
        })();
        let Some(handle) = runtime.data_result(result) else {
            return 0;
        };
        runtime.data_allocated(1);
        handle.bits() as i64
    })
}

extern "C" fn baseline_bytes(context: *mut BaselineRuntime, bytes: *const u8, length: i64) -> i64 {
    with_runtime(context, 0, |runtime| {
        let result = (|| {
            let length = usize::try_from(length).map_err(|_| DataError::InvalidEncoding)?;
            if length > isize::MAX as usize || (length != 0 && bytes.is_null()) {
                return Err(DataError::InvalidEncoding);
            }
            let bytes = if length == 0 {
                &[][..]
            } else {
                // SAFETY: literal data in the executable has the declared length.
                unsafe { std::slice::from_raw_parts(bytes, length) }
            };
            runtime
                .data
                .create_bytes(&mut runtime.heap, length as u64, bytes)
        })();
        let Some(handle) = runtime.data_result(result) else {
            return 0;
        };
        runtime.data_allocated(1);
        handle.bits() as i64
    })
}

extern "C" fn baseline_bytes_pad(
    context: *mut BaselineRuntime,
    bits: i64,
    from: i64,
    to: i64,
) -> i64 {
    with_runtime(context, 0, |runtime| {
        let result = (|| {
            let (from, to) = (from as u64, to as u64);
            if to < from {
                return Err(DataError::InvalidEncoding);
            }
            let handle = ManagedHandle::from_bits(bits as u64).ok_or(DataError::InvalidEncoding)?;
            let mut bytes = runtime.data.bytes(&runtime.heap, handle, from)?;
            let length = usize::try_from(to).map_err(|_| DataError::AddressSpaceExhausted)?;
            bytes
                .try_reserve_exact(length - bytes.len())
                .map_err(|_| DataError::AddressSpaceExhausted)?;
            bytes.resize(length, 0);
            runtime.data.create_bytes(&mut runtime.heap, to, &bytes)
        })();
        let Some(handle) = runtime.data_result(result) else {
            return 0;
        };
        runtime.data_allocated(1);
        handle.bits() as i64
    })
}

extern "C" fn baseline_bytes_length(
    context: *mut BaselineRuntime,
    bits: i64,
    expected: i64,
) -> i64 {
    with_runtime(context, 0, |runtime| {
        let result = ManagedHandle::from_bits(bits as u64)
            .ok_or(DataError::InvalidEncoding)
            .and_then(|handle| {
                runtime
                    .data
                    .byte_length(&runtime.heap, handle, expected as u64)
            });
        runtime.data_result(result).unwrap_or(0) as i64
    })
}

extern "C" fn baseline_project(
    context: *mut BaselineRuntime,
    bits: i64,
    type_id: i64,
    variant: i64,
    field: i64,
) -> i64 {
    with_runtime(context, 0, |runtime| {
        let result = (|| {
            let handle = ManagedHandle::from_bits(bits as u64).ok_or(DataError::InvalidEncoding)?;
            let ty = usize::try_from(type_id).map_err(|_| DataError::InvalidEncoding)?;
            let field = usize::try_from(field).map_err(|_| DataError::InvalidEncoding)?;
            let variant = if variant == -1 {
                None
            } else {
                Some(u32::try_from(variant).map_err(|_| DataError::InvalidEncoding)?)
            };
            runtime
                .data
                .project(&mut runtime.heap, handle, ty, variant, field)
        })();
        runtime
            .data_result(result)
            .map(|slot| match slot {
                ManagedSlot::Scalar(value) => value.bits(),
                ManagedSlot::Unit => 0,
                ManagedSlot::Reference(handle) => handle.bits(),
                ManagedSlot::Opaque(bits) => bits,
            })
            .unwrap_or(0) as i64
    })
}

extern "C" fn baseline_store_field(
    context: *mut BaselineRuntime,
    bits: i64,
    type_id: i64,
    field: i64,
    value: i64,
) -> i64 {
    with_runtime(context, 0, |runtime| {
        let result = (|| {
            let handle = ManagedHandle::from_bits(bits as u64).ok_or(DataError::InvalidEncoding)?;
            let ty = usize::try_from(type_id).map_err(|_| DataError::InvalidEncoding)?;
            let index = usize::try_from(field).map_err(|_| DataError::InvalidEncoding)?;
            let child = match runtime.data_schema.value.ty(ty)? {
                rinzu_runtime::value::ValueType::State { fields, .. } => {
                    *fields.get(index).ok_or(DataError::InvalidEncoding)?
                }
                _ => return Err(DataError::WrongKind),
            };
            let slot = runtime.data_schema.slot(child, value as u64)?;
            runtime
                .data
                .set_state_field(&mut runtime.heap, handle, ty, index, slot)
        })();
        runtime.data_result(result);
        0
    })
}

extern "C" fn baseline_enum_variant(context: *mut BaselineRuntime, bits: i64, type_id: i64) -> i64 {
    with_runtime(context, 0, |runtime| {
        let result = (|| {
            let handle = ManagedHandle::from_bits(bits as u64).ok_or(DataError::InvalidEncoding)?;
            let ty = usize::try_from(type_id).map_err(|_| DataError::InvalidEncoding)?;
            runtime.data.enum_variant(&runtime.heap, handle, ty)
        })();
        runtime.data_result(result).map(i64::from).unwrap_or(-1)
    })
}

extern "C" fn baseline_value_compare(
    context: *mut BaselineRuntime,
    type_id: i64,
    operation: i64,
    left: i64,
    right: i64,
) -> i64 {
    with_runtime(context, 0, |runtime| {
        let result = (|| {
            if operation != 0 && operation != 1 {
                return Err(DataError::InvalidEncoding);
            }
            let ty = usize::try_from(type_id).map_err(|_| DataError::InvalidEncoding)?;
            let left = runtime.data_schema.slot(ty, left as u64)?;
            let right = runtime.data_schema.slot(ty, right as u64)?;
            runtime.data.values_equal(&runtime.heap, ty, left, right)
        })();
        runtime
            .data_result(result)
            .map(|equal| i64::from(equal ^ (operation == 1)))
            .unwrap_or(0)
    })
}

extern "C" fn baseline_scalar_arithmetic(
    context: *mut BaselineRuntime,
    kind: i64,
    operation: i64,
    left: i64,
    right: i64,
) -> i64 {
    with_runtime(context, 0, |runtime| {
        use rinzu_runtime::scalar::{Arithmetic as A, ScalarKind, ScalarValue};
        let evaluated = (|| {
            let kind = u32::try_from(kind).ok().and_then(ScalarKind::from_tag)?;
            let operation = match operation {
                0 => A::Negate,
                1 => A::Add,
                2 => A::Subtract,
                3 => A::Multiply,
                4 => A::Divide,
                5 => A::Remainder,
                _ => return None,
            };
            Some(ScalarValue::from_bits(kind, left as u64).and_then(|left| {
                ScalarValue::from_bits(kind, right as u64)
                    .and_then(|right| left.arithmetic(operation, right))
            }))
        })();
        match evaluated {
            Some(Ok(value)) => value.bits() as i64,
            Some(Err(rinzu_runtime::scalar::ScalarError::Arithmetic)) => {
                runtime.fail(BaselineFailure::Arithmetic);
                0
            }
            _ => {
                runtime.fail(BaselineFailure::Unsupported);
                0
            }
        }
    })
}
extern "C" fn baseline_scalar_compare(
    context: *mut BaselineRuntime,
    kind: i64,
    operation: i64,
    left: i64,
    right: i64,
) -> i64 {
    with_runtime(context, 0, |runtime| {
        use rinzu_runtime::scalar::{Comparison as C, ScalarKind, ScalarValue};
        let evaluated = (|| {
            let kind = u32::try_from(kind).ok().and_then(ScalarKind::from_tag)?;
            let operation = match operation {
                0 => C::Equal,
                1 => C::NotEqual,
                2 => C::Less,
                3 => C::LessEqual,
                4 => C::Greater,
                5 => C::GreaterEqual,
                _ => return None,
            };
            Some(ScalarValue::from_bits(kind, left as u64).and_then(|left| {
                ScalarValue::from_bits(kind, right as u64)
                    .and_then(|right| left.compare(operation, right))
            }))
        })();
        match evaluated {
            Some(Ok(value)) => i64::from(value),
            _ => {
                runtime.fail(BaselineFailure::Unsupported);
                0
            }
        }
    })
}

const OP_NEGATE: i64 = 0;
const OP_ADD: i64 = 1;
const OP_SUBTRACT: i64 = 2;
const OP_MULTIPLY: i64 = 3;
const OP_DIVIDE: i64 = 4;
const OP_REMAINDER: i64 = 5;

extern "C" fn baseline_checked_arithmetic(
    context: *mut BaselineRuntime,
    operation: i64,
    left: i64,
    right: i64,
) -> i64 {
    with_runtime(context, 0, |runtime| {
        let result = match operation {
            OP_NEGATE => left.checked_neg(),
            OP_ADD => left.checked_add(right),
            OP_SUBTRACT => left.checked_sub(right),
            OP_MULTIPLY => left.checked_mul(right),
            OP_DIVIDE => left.checked_div(right),
            OP_REMAINDER => left.checked_rem(right),
            _ => {
                runtime.fail(BaselineFailure::Unsupported);
                return 0;
            }
        };
        result.unwrap_or_else(|| {
            runtime.fail(BaselineFailure::Arithmetic);
            0
        })
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
unsafe extern "C" fn invoke_native_handler(context: *mut c_void, handler: u64) -> i64 {
    let Ok(handler) = usize::try_from(handler) else {
        with_runtime(context.cast(), (), |runtime| {
            runtime.fail(BaselineFailure::Unsupported);
        });
        return 0;
    };
    // SAFETY: the package call is synchronous and the validated JIT runtime,
    // dispatch table, and code mapping remain live until it returns.
    match unsafe { target::invoke_callback_trampoline(context.cast(), handler) } {
        Ok(value) => value,
        Err(_) => {
            with_runtime(context.cast(), (), |runtime| {
                runtime.fail(BaselineFailure::Unsupported);
            });
            0
        }
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[allow(clippy::too_many_arguments)]
extern "C" fn baseline_native_invoke(
    context: *mut BaselineRuntime,
    import_key: i64,
    argument_count: i64,
    argument_0: i64,
    argument_1: i64,
    argument_2: i64,
    argument_3: i64,
    argument_4: i64,
    argument_5: i64,
    argument_6: i64,
    argument_7: i64,
) -> i64 {
    with_runtime(context, 0, |runtime| {
        let Ok(argument_count) = usize::try_from(argument_count) else {
            runtime.fail(BaselineFailure::Unsupported);
            return 0;
        };
        if argument_count > 8 {
            runtime.fail(BaselineFailure::Unsupported);
            return 0;
        }
        let arguments = [
            argument_0, argument_1, argument_2, argument_3, argument_4, argument_5, argument_6,
            argument_7,
        ];
        let invocation = NativeInvocationContextV1 {
            struct_size: size_of::<NativeInvocationContextV1>() as u32,
            abi_version: ABI_VERSION,
            runtime_context: context.cast(),
            invoke_handler: Some(invoke_native_handler),
        };
        let Some(packages) = runtime.native_packages.as_ref() else {
            runtime.fail(BaselineFailure::Unsupported);
            return 0;
        };
        match packages.invoke(import_key as u64, &invocation, &arguments[..argument_count]) {
            Ok(result) => result,
            Err(_) => {
                runtime.fail(BaselineFailure::Unsupported);
                0
            }
        }
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod target {
    use super::*;
    use cranelift_codegen::ir::{
        AbiParam, InstBuilder, UserFuncName, Value, condcodes::IntCC, types,
    };
    use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Switch, Variable};
    use cranelift_jit::{JITBuilder, JITModule};
    use cranelift_module::{DataDescription, FuncId, Linkage, Module, default_libcall_names};
    use cranelift_object::{ObjectBuilder, ObjectModule};

    use crate::macos_jit_memory::MacOsArm64JitMemory;
    use rinzu_compiler::Type;

    const SOURCE_SYMBOL: &str = "rinzu_baseline_set_source_packed";
    const OBSERVE_SOURCE_SYMBOL: &str = "rinzu_baseline_observe_source";
    const FAILURE_SYMBOL: &str = "rinzu_baseline_has_failure";
    const ROOTS_SYMBOL: &str = "rinzu_baseline_publish_roots_poll";
    const PREPARE_ROOTS_SYMBOL: &str = "rinzu_baseline_prepare_roots";
    const COMMIT_ROOTS_SYMBOL: &str = "rinzu_baseline_commit_roots_poll";
    const COMMIT_PROVEN_ROOTS_SYMBOL: &str = "rinzu_baseline_commit_proven_roots_poll";
    const COMMIT_PARTIAL_ROOTS_SYMBOL: &str = "rinzu_baseline_commit_partial_roots_poll";
    const ENTRY_SYMBOL: &str = "rinzu_baseline_function_entry";
    const CONSTRUCT_SYMBOL: &str = "rinzu_baseline_construct";
    const DATA_OPERATION_SYMBOL: &str = "rinzu_baseline_data_operation";
    const BYTES_SLICE_PAYLOAD_SYMBOL: &str = "rinzu_baseline_bytes_slice_payload";
    const BYTES_INT_LENGTH_SYMBOL: &str = "rinzu_baseline_bytes_int_length";
    const STRING_SCALAR_LENGTH_SYMBOL: &str = "rinzu_baseline_string_scalar_length";
    const COLLECTION_LENGTH_SYMBOL: &str = "rinzu_baseline_collection_length";
    const BYTES_SYMBOL: &str = "rinzu_baseline_bytes";
    const BYTES_PAD_SYMBOL: &str = "rinzu_baseline_bytes_pad";
    const BYTES_LENGTH_SYMBOL: &str = "rinzu_baseline_bytes_length";
    const STORE_FIELD_SYMBOL: &str = "rinzu_baseline_store_field";
    const PROJECT_SYMBOL: &str = "rinzu_baseline_project";
    const ENUM_VARIANT_SYMBOL: &str = "rinzu_baseline_enum_variant";
    const VALUE_COMPARE_SYMBOL: &str = "rinzu_baseline_value_compare";
    const SCALAR_ARITHMETIC_SYMBOL: &str = "rinzu_baseline_scalar_arithmetic";
    const SCALAR_CONVERT_SYMBOL: &str = "rinzu_baseline_scalar_convert";
    const HAS_ERROR_SYMBOL: &str = "rinzu_baseline_has_error";
    const CATCH_ERROR_SYMBOL: &str = "rinzu_baseline_catch_error";
    const STRING_SYMBOL: &str = "rinzu_baseline_string";
    const ERROR_MESSAGE_SYMBOL: &str = "rinzu_baseline_error_message";
    const ERROR_DATA_SYMBOL: &str = "rinzu_baseline_error_data";
    const STDOUT_SYMBOL: &str = "rinzu_baseline_stdout";
    const FORMAT_STRING_SYMBOL: &str = "rinzu_baseline_format_string";
    const TASK_CONTROL_SYMBOL: &str = "rinzu_baseline_task_control";
    const UTF8_LENGTH_SYMBOL: &str = "rinzu_baseline_utf8_length";
    const STRING_COMPARE_SYMBOL: &str = "rinzu_baseline_string_compare";
    const SCALAR_COMPARE_SYMBOL: &str = "rinzu_baseline_scalar_compare";
    const ARITHMETIC_SYMBOL: &str = "rinzu_baseline_checked_arithmetic";
    const BEGIN_CLEANUP_SYMBOL: &str = "rinzu_baseline_begin_cleanup";
    const END_CLEANUP_SYMBOL: &str = "rinzu_baseline_end_cleanup";
    const RESOLVE_DISPATCH_SYMBOL: &str = "rinzu_baseline_resolve_dispatch";
    const LEAVE_DISPATCH_SYMBOL: &str = "rinzu_baseline_leave_dispatch";
    const NATIVE_INVOKE_SYMBOL: &str = "rinzu_baseline_native_invoke";

    struct AsyncRuntimeFunctions {
        take_task_errors: FuncId,
        task_error_origin: FuncId,
        task_scope: FuncId,
        spawn: FuncId,
        await_value: FuncId,
        call: FuncId,
        service: FuncId,
        supplant_reserve: FuncId,
        supplant_start: FuncId,
        supplant_settle: FuncId,
    }

    struct RuntimeFunctions {
        owned_imports: HashSet<FuncId>,
        proven_owner_operations: HashMap<String, HashSet<(BlockId, usize)>>,
        owner_helpers: BTreeMap<&'static str, FuncId>,
        proven_frame_roots: HashMap<String, HashSet<(BlockId, usize)>>,
        proven_frame_memberships: HashMap<String, HashMap<(BlockId, usize), u64>>,
        fresh_return_consuming_awaits: HashMap<String, HashSet<(BlockId, usize)>>,
        data_schema: crate::data_schema::DataSchema,
        reference_targets: crate::function_reference::ReferenceTargets,
        native_plans: crate::native_call::NativeCallPlans,
        native_helpers: BTreeMap<&'static str, FuncId>,
        native_stubs: BTreeMap<String, FuncId>,
        source_async: Option<AsyncRuntimeFunctions>,
        set_source: FuncId,
        observe_source: FuncId,
        failure: FuncId,
        entry: FuncId,
        publish_roots: FuncId,
        prepare_roots: FuncId,
        commit_roots: FuncId,
        commit_proven_roots: Option<FuncId>,
        commit_partial_roots: Option<FuncId>,
        construct: FuncId,
        data_operation: FuncId,
        bytes_slice_payload: Option<FuncId>,
        bytes_int_length: FuncId,
        string_scalar_length: FuncId,
        collection_length: FuncId,
        bytes: FuncId,
        bytes_pad: FuncId,
        bytes_length: FuncId,
        store_field: FuncId,
        project: FuncId,
        enum_variant: FuncId,
        value_compare: FuncId,
        arithmetic: FuncId,
        scalar_arithmetic: FuncId,
        scalar_compare: FuncId,
        scalar_convert: FuncId,
        has_error: FuncId,
        catch_error: FuncId,
        source_ids: HashMap<u32, u64>,
        string: FuncId,
        error_message: FuncId,
        error_data: FuncId,
        stdout: FuncId,
        format_string: FuncId,
        utf8_length: FuncId,
        task_control: FuncId,
        string_compare: FuncId,
        begin_cleanup: FuncId,
        end_cleanup: FuncId,
        resolve_dispatch: FuncId,
        leave_dispatch: FuncId,
        retain_return: FuncId,
        native_invoke: FuncId,
    }

    struct FunctionInfo {
        id: FuncId,
        index: usize,
        symbol: String,
        parameter_count: usize,
        is_async: bool,
        signature: cranelift_codegen::ir::Signature,
    }

    pub(super) fn run(
        module_ir: &IrModule,
        entry: &str,
        options: BaselineOptions,
        native_packages: Option<LoadedNativePackages>,
        source_packages: Option<crate::source_package::LoadedSourcePackages>,
    ) -> Result<BaselineRun> {
        let native_plans = crate::native_call::NativeCallPlans::new(module_ir)?;
        let source_bindings = source_packages
            .as_ref()
            .map(|packages| packages.source_bindings(module_ir))
            .transpose()?
            .unwrap_or_default();
        let source_consumers = source_packages
            .as_ref()
            .map(|packages| packages.return_consumers(&module_ir.source_native_signatures))
            .transpose()?
            .unwrap_or_default();
        let source_records = source_packages
            .as_ref()
            .map(|packages| crate::source_native_record_layouts(module_ir, packages.descriptors()))
            .transpose()?
            .unwrap_or_default()
            .into_iter()
            .map(|(identity, layout)| (identity, std::sync::Arc::new(layout)))
            .collect();
        for plan in &native_plans.records {
            ensure!(
                source_bindings.contains_key(&plan.signature.name),
                "source native binding is unavailable for {}",
                plan.signature.name
            );
            ensure!(
                matches!(
                    plan.signature.result,
                    rinzu_compiler::SourceAbiType::Scalar(_)
                        | rinzu_compiler::SourceAbiType::Unit
                        | rinzu_compiler::SourceAbiType::Opaque(_)
                        | rinzu_compiler::SourceAbiType::String
                        | rinzu_compiler::SourceAbiType::Bytes(_)
                        | rinzu_compiler::SourceAbiType::ByteArrayResult
                        | rinzu_compiler::SourceAbiType::Record(_)
                        | rinzu_compiler::SourceAbiType::UserData(_)
                ),
                "native managed result requires the R3 receiver backend"
            );
        }
        let isa = super::super::native_isa(false)?;
        let mut jit_builder = JITBuilder::with_isa(isa, default_libcall_names());
        jit_builder.symbol(SOURCE_SYMBOL, baseline_set_source as *const u8);
        jit_builder.symbol(OBSERVE_SOURCE_SYMBOL, baseline_observe_source as *const u8);
        jit_builder.symbol(FAILURE_SYMBOL, baseline_has_failure as *const u8);
        jit_builder.symbol(ENTRY_SYMBOL, baseline_function_entry as *const u8);
        jit_builder.symbol(ROOTS_SYMBOL, baseline_publish_roots_poll as *const u8);
        jit_builder.symbol(PREPARE_ROOTS_SYMBOL, baseline_prepare_roots as *const u8);
        jit_builder.symbol(COMMIT_ROOTS_SYMBOL, baseline_commit_roots_poll as *const u8);
        jit_builder.symbol(CONSTRUCT_SYMBOL, baseline_construct as *const u8);
        jit_builder.symbol(DATA_OPERATION_SYMBOL, baseline_data_operation as *const u8);
        jit_builder.symbol(
            BYTES_INT_LENGTH_SYMBOL,
            baseline_bytes_int_length as *const u8,
        );
        jit_builder.symbol(
            STRING_SCALAR_LENGTH_SYMBOL,
            baseline_string_scalar_length as *const u8,
        );
        jit_builder.symbol(
            COLLECTION_LENGTH_SYMBOL,
            baseline_collection_length as *const u8,
        );
        jit_builder.symbol(BYTES_SYMBOL, baseline_bytes as *const u8);
        jit_builder.symbol(BYTES_PAD_SYMBOL, baseline_bytes_pad as *const u8);
        jit_builder.symbol(BYTES_LENGTH_SYMBOL, baseline_bytes_length as *const u8);
        jit_builder.symbol(STORE_FIELD_SYMBOL, baseline_store_field as *const u8);
        jit_builder.symbol(PROJECT_SYMBOL, baseline_project as *const u8);
        jit_builder.symbol(ENUM_VARIANT_SYMBOL, baseline_enum_variant as *const u8);
        jit_builder.symbol(VALUE_COMPARE_SYMBOL, baseline_value_compare as *const u8);
        jit_builder.symbol(ARITHMETIC_SYMBOL, baseline_checked_arithmetic as *const u8);
        jit_builder.symbol(
            SCALAR_ARITHMETIC_SYMBOL,
            baseline_scalar_arithmetic as *const u8,
        );
        jit_builder.symbol(SCALAR_COMPARE_SYMBOL, baseline_scalar_compare as *const u8);
        jit_builder.symbol(SCALAR_CONVERT_SYMBOL, baseline_scalar_convert as *const u8);
        jit_builder.symbol(HAS_ERROR_SYMBOL, baseline_has_error as *const u8);
        jit_builder.symbol(CATCH_ERROR_SYMBOL, baseline_catch_error as *const u8);
        jit_builder.symbol(STRING_SYMBOL, baseline_string as *const u8);
        jit_builder.symbol(ERROR_MESSAGE_SYMBOL, baseline_error_message as *const u8);
        jit_builder.symbol(ERROR_DATA_SYMBOL, errors::error_data as *const u8);
        jit_builder.symbol(STDOUT_SYMBOL, baseline_stdout as *const u8);
        jit_builder.symbol(FORMAT_STRING_SYMBOL, baseline_format_string as *const u8);
        jit_builder.symbol(TASK_CONTROL_SYMBOL, baseline_task_control as *const u8);
        jit_builder.symbol(UTF8_LENGTH_SYMBOL, baseline_utf8_length as *const u8);
        jit_builder.symbol(STRING_COMPARE_SYMBOL, baseline_string_compare as *const u8);
        jit_builder.symbol(BEGIN_CLEANUP_SYMBOL, baseline_begin_cleanup as *const u8);
        jit_builder.symbol(END_CLEANUP_SYMBOL, baseline_end_cleanup as *const u8);
        jit_builder.symbol(
            RESOLVE_DISPATCH_SYMBOL,
            baseline_resolve_dispatch as *const u8,
        );
        jit_builder.symbol(LEAVE_DISPATCH_SYMBOL, baseline_leave_dispatch as *const u8);
        jit_builder.symbol(
            "rinzu_baseline_retain_return",
            baseline_retain_return as *const u8,
        );
        jit_builder.symbol(NATIVE_INVOKE_SYMBOL, baseline_native_invoke as *const u8);
        for (name, address) in [
            ("prepare", native::prepare as *const u8),
            ("address", native::address as *const u8),
            ("words", native::words as *const u8),
            ("returned", native::returned as *const u8),
            ("close", native::close as *const u8),
            ("settle", native::settle as *const u8),
            ("read", native::read as *const u8),
            ("release", native::release as *const u8),
        ] {
            jit_builder.symbol(format!("rinzu_baseline_source_native_{name}"), address);
        }
        let (jit_memory, memory_audit) = MacOsArm64JitMemory::new_with_audit()?;
        jit_builder.memory_provider(Box::new(jit_memory));
        let mut module = JITModule::new(jit_builder);
        let runtime_functions = declare_runtime_functions(&mut module, module_ir, false, false)?;
        let functions = declare_managed_functions(&mut module, module_ir, Linkage::Local)?;
        let native_imports = native_packages
            .as_ref()
            .map(LoadedNativePackages::export_bindings)
            .unwrap_or_default();

        let mut compile = CompileEvidence {
            stack_map_count: 0,
            stack_map_entry_count: 0,
            code_size: 0,
        };
        let mut code_sizes = HashMap::new();
        for (function_index, function) in module_ir.functions.iter().enumerate() {
            let code_size = define_function(
                &mut module,
                function,
                function_index,
                &functions,
                &runtime_functions,
                &native_imports,
                false,
                &mut compile,
                None,
                None,
            )?;
            code_sizes.insert(function_index, code_size);
        }
        module
            .finalize_definitions()
            .context("finalize baseline JIT definitions")?;
        let memory_audit = memory_audit.snapshot();
        ensure!(
            memory_audit.executable_mappings > 0,
            "baseline JIT published no executable mapping"
        );
        ensure!(
            memory_audit.icache_flushes == memory_audit.executable_mappings
                && memory_audit.rx_transitions == memory_audit.executable_mappings
                && memory_audit.rwx_mappings == 0,
            "baseline JIT violated its W^X publication protocol"
        );
        let entry = functions
            .get(entry)
            .ok_or_else(|| anyhow!("missing JIT entry function {entry}"))?;
        ensure!(
            entry.parameter_count == 0,
            "JIT entry must not take arguments"
        );
        let function_names = module_ir
            .functions
            .iter()
            .map(|function| function.name.clone())
            .collect();
        let mut runtime = BaselineRuntime::new(
            function_names,
            options,
            native_packages,
            runtime_functions.data_schema.clone(),
        );
        runtime
            .data
            .configure_sources(&runtime.heap, crate::error_sources::catalog(module_ir))
            .map_err(|error| anyhow!("invalid source coordinates: {error:?}"))?;
        runtime.source_native.plans = native_plans.records.into_iter().map(Arc::new).collect();
        runtime.source_native.targets = runtime_functions.reference_targets.metadata();
        runtime.source_native.bindings = source_bindings;
        runtime.source_native.consumers = source_consumers;
        runtime.source_native.records = source_records;
        runtime.source_native.functions = module_ir.functions.clone();
        runtime.source_native.prepare_managed_stubs()?;
        for function in functions.values() {
            let address = module.get_finalized_function(function.id) as usize;
            let target = runtime.dispatch_cells[function.index]
                .publish(address)
                .context("publish finalized JIT dispatch target")?;
            let code_version = CodeVersion::published(
                CodeVersionId(u64::try_from(function.index)? + 1),
                address,
                code_sizes[&function.index],
            )?;
            runtime
                .code_versions
                .insert((function.index, target.version), code_version);
        }

        // SAFETY: the callback trampoline loads the finalized context-only entry
        // signature from its versioned dispatch cell.
        let value =
            unsafe { invoke_callback_trampoline(std::ptr::from_mut(&mut runtime), entry.index) }?;
        ensure!(
            runtime.data_roots.frame_count() == 0,
            "managed root frames leaked after JIT execution"
        );
        ensure!(
            runtime.source_native.evidence.is_baseline() && runtime.data_roots.lease_count() == 0,
            "native call ledger or roots leaked after JIT execution"
        );
        let native_userdata = runtime.userdata.finish(&runtime.heap);
        // The Domain is ending. A returned scalar slot needs no managed root;
        // Dispose provisional roots and unrooted reservations discarded by a
        // final NULL result. Removing their roots does not itself collect heap
        // storage, and the failing path may have no subsequent safepoint.
        if runtime.data.root_count() != 0 || runtime.heap.object_count() != 0 {
            runtime
                .data
                .release_all(&mut runtime.heap)
                .map_err(|error| anyhow!("release managed data: {error:?}"))?;
            runtime.heap.collect(CollectionKind::Full)?;
            runtime.collection_count += 1;
        }
        let failure_details = std::mem::take(&mut runtime.pending)
            .finish()
            .map_err(|_| anyhow!("finally regions leaked after JIT execution"))?;
        runtime
            .transitions
            .deactivate_managed()
            .context("JIT Owner ended with an active native or callback frame")?;
        let result = failure_details.as_ref().map_or(Ok(value), |failure| {
            Err(match failure.primary.cause {
                FailureCause::Error(ErrorKind::Conversion) => BaselineFailure::Conversion,
                FailureCause::Error(kind) => BaselineFailure::Error(kind),
                FailureCause::ArithmeticTrap => BaselineFailure::Arithmetic,
                FailureCause::Cancelled => BaselineFailure::Cancelled,
                FailureCause::Shutdown => BaselineFailure::Shutdown,
                FailureCause::Overloaded => BaselineFailure::Overloaded,
                FailureCause::Unsupported => BaselineFailure::Unsupported,
                FailureCause::RuntimePanic => BaselineFailure::RuntimePanic,
                FailureCause::Panic => BaselineFailure::Panic,
            })
        });
        let dispatch_versions = runtime
            .dispatch_cells
            .iter()
            .map(|cell| cell.load().map(|target| target.version))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ensure!(
            runtime.managed_frames.is_empty(),
            "managed dispatch frames leaked after JIT execution"
        );
        let published_code_versions = runtime.code_versions.len();
        let peak_live_code_frames = runtime
            .code_versions
            .values()
            .map(|version| version.peak_live_frames())
            .max()
            .unwrap_or(0);
        for version in runtime.code_versions.values() {
            version.request_retire();
        }
        let retired_code_versions = runtime
            .code_versions
            .values()
            .filter(|version| version.state() == CodeVersionState::Retired)
            .count();
        ensure!(
            retired_code_versions == published_code_versions,
            "JIT code cannot be unmapped while an old live frame remains"
        );
        let run = BaselineRun {
            stdout: runtime.stdout.take(),
            result,
            failure_details,
            source_files: source_identities(module_ir).1,
            caught_errors: runtime.caught_errors,
            root_publications: runtime.root_publications,
            peak_root_frames: runtime.peak_root_frames,
            function_trace: runtime.function_trace,
            poll_count: runtime.poll_count,
            allocation_count: runtime.allocation_count,
            collection_count: runtime.collection_count,
            heap_objects: runtime.heap.object_count(),
            peak_heap_objects: runtime.peak_heap_objects,
            dispatch_resolutions: runtime.dispatch_resolutions,
            callback_dispatches: runtime.callback_dispatches,
            dispatch_versions,
            published_code_versions,
            retired_code_versions,
            peak_live_code_frames,
            executable_mappings: memory_audit.executable_mappings,
            icache_flushes: memory_audit.icache_flushes,
            rx_transitions: memory_audit.rx_transitions,
            rwx_mappings: memory_audit.rwx_mappings,
            compile,
            memory_policy: "MAP_JIT RW -> instruction-cache flush -> RX",
            host_trace: runtime.host_trace,
            native_calls: runtime.source_native.evidence,
            native_process: crate::NativeProcessEvidence::capture(),
            native_userdata,
            native_record_scratch: runtime.data.native_record_scratch_stats(),
        };
        // SAFETY: no generated code pointer is used after this point.
        unsafe { module.free_memory() };
        Ok(run)
    }

    pub(super) fn compile_async_jit(
        ir: &IrModule,
        external_symbols: &BTreeMap<String, usize>,
        native_imports: &HashMap<String, u64>,
    ) -> Result<CompiledAsyncJit> {
        let isa = super::super::native_isa(false)?;
        let mut builder = JITBuilder::with_isa(isa, default_libcall_names());
        for (symbol, address) in external_symbols {
            ensure!(*address != 0, "null async runtime symbol");
            builder.symbol(symbol, address.to_owned() as *const u8);
        }
        let (memory, audit) = MacOsArm64JitMemory::new_with_audit()?;
        builder.memory_provider(Box::new(memory));
        let mut module = JITModule::new(builder);
        let runtime = declare_runtime_functions(&mut module, ir, true, true)?;
        let functions = declare_managed_functions(&mut module, ir, Linkage::Export)?;
        let mut compile = CompileEvidence {
            stack_map_count: 0,
            stack_map_entry_count: 0,
            code_size: 0,
        };
        for (index, function) in ir.functions.iter().enumerate() {
            define_function(
                &mut module,
                function,
                index,
                &functions,
                &runtime,
                native_imports,
                false,
                &mut compile,
                None,
                None,
            )?;
        }
        module
            .finalize_definitions()
            .context("finalize compiled async JIT")?;
        let memory = audit.snapshot();
        ensure!(
            memory.executable_mappings > 0
                && memory.icache_flushes == memory.executable_mappings
                && memory.rx_transitions == memory.executable_mappings
                && memory.rwx_mappings == 0,
            "compiled async JIT violated W^X publication"
        );
        let symbols = module
            .declarations()
            .get_functions()
            .filter(|(_, declaration)| declaration.linkage == Linkage::Export)
            .map(|(id, declaration)| {
                (
                    declaration.name.clone().expect("export has a name"),
                    module.get_finalized_function(id) as usize,
                )
            })
            .collect();
        Ok(CompiledAsyncJit {
            module: Some(module),
            symbols,
            compile,
            executable_mappings: memory.executable_mappings,
            executable_bytes: memory.executable_bytes,
            icache_flushes: memory.icache_flushes,
            rx_transitions: memory.rx_transitions,
            rwx_mappings: memory.rwx_mappings,
        })
    }

    pub(super) fn emit_aot(
        module_ir: &IrModule,
        entry: &str,
        native_imports: &HashMap<String, u64>,
        source_async: bool,
        c_backend: bool,
    ) -> Result<BaselineAotObject> {
        let isa = super::super::native_isa(true)?;
        let builder = ObjectBuilder::new(isa, "rinzu-baseline-aot", default_libcall_names())
            .context("create baseline AOT object backend")?;
        let mut module = ObjectModule::new(builder);
        let runtime_functions =
            declare_runtime_functions(&mut module, module_ir, source_async, true)?;
        // C-lowered definitions live in the companion translation unit. Keep
        // them imported in this object; native definitions are promoted below.
        let functions = declare_managed_functions(&mut module, module_ir, Linkage::Import)?;
        let entry = functions
            .get(entry)
            .ok_or_else(|| anyhow!("missing AOT entry function {entry}"))?;
        ensure!(
            entry.parameter_count == 0,
            "AOT entry must not take arguments"
        );
        let entry_index = entry.index;
        let entry_symbol = entry.symbol.clone();
        let mut compile = CompileEvidence {
            stack_map_count: 0,
            stack_map_entry_count: 0,
            code_size: 0,
        };
        let mut c_output = CFunctions::default();
        for (function_index, function) in module_ir.functions.iter().enumerate() {
            define_function(
                &mut module,
                function,
                function_index,
                &functions,
                &runtime_functions,
                native_imports,
                true,
                &mut compile,
                None,
                c_backend.then_some(&mut c_output),
            )?;
        }
        let source_ids = source_identities(module_ir).0;
        let mut metadata = functions
            .iter()
            .map(|(name, function)| AotFunctionMetadata {
                source: {
                    let range = module_ir.functions[function.index].range;
                    SourcePosition {
                        file: source_ids
                            .get(&range.file_id.0)
                            .copied()
                            .unwrap_or(u64::from(range.file_id.0)),
                        start_byte: range.start_byte,
                        end_byte: range.end_byte,
                    }
                },
                index: function.index,
                name: name.clone(),
                symbol: function.symbol.clone(),
                parameter_count: function.parameter_count,
                parameter_types: module_ir.functions[function.index].parameter_types.clone(),
                result_type: module_ir.functions[function.index].result_type.clone(),
                is_async: module_ir.functions[function.index].is_async,
                callback_fallback: module_ir.functions[function.index]
                    .callback_fallback
                    .clone(),
            })
            .collect::<Vec<_>>();
        metadata.sort_by_key(|function| function.index);
        let object = module
            .finish()
            .emit()
            .context("emit baseline AOT object bytes")?;
        ensure!(!object.is_empty(), "baseline AOT object is empty");
        Ok(BaselineAotObject {
            object,
            c_function_source: c_output.source,
            c_functions: c_output.names,
            c_fallbacks: c_output.fallbacks,
            entry_index,
            entry_symbol,
            functions: metadata,
            source_files: source_identities(module_ir).1,
            source_locations_c: crate::error_sources::c_source(module_ir),
            data_schema_c: runtime_functions.data_schema.c_source(),
            data_types: runtime_functions.data_schema.metadata(),
            reference_targets: runtime_functions.reference_targets.metadata(),
            native_call_plans: runtime_functions.native_plans.records,
            native_signatures: module_ir.source_native_signatures.clone(),
            native_record_schemas: module_ir.native_records.clone(),
            compile,
        })
    }

    fn declare_managed_functions<M: Module>(
        module: &mut M,
        module_ir: &IrModule,
        linkage: Linkage,
    ) -> Result<HashMap<String, FunctionInfo>> {
        let mut functions = HashMap::new();
        for (index, function) in module_ir.functions.iter().enumerate() {
            let parameter_count = parameter_count(function)?;
            let mut signature = module.make_signature();
            signature.call_conv = module.target_config().default_call_conv;
            signature.params.push(AbiParam::new(types::I64));
            signature
                .params
                .extend((0..parameter_count).map(|_| AbiParam::new(types::I64)));
            signature.returns.push(AbiParam::new(types::I64));
            let symbol = managed_symbol(index, &function.name);
            let id = module
                .declare_function(&symbol, linkage, &signature)
                .with_context(|| format!("declare baseline function {}", function.name))?;
            functions.insert(
                function.name.clone(),
                FunctionInfo {
                    id,
                    index,
                    symbol,
                    parameter_count,
                    is_async: function.is_async,
                    signature,
                },
            );
        }
        Ok(functions)
    }

    fn managed_symbol(index: usize, name: &str) -> String {
        let name = name
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || character == '_' {
                    character
                } else {
                    '_'
                }
            })
            .collect::<String>();
        format!("rinzu_managed_{index}_{name}")
    }

    pub(super) unsafe fn invoke_callback_trampoline(
        runtime: *mut BaselineRuntime,
        function: usize,
    ) -> Result<i64> {
        let (address, frame) = {
            // SAFETY: the caller guarantees a live baseline runtime for this
            // synchronous callback dispatch.
            let runtime = unsafe { &mut *runtime };
            let target = runtime
                .dispatch_cells
                .get(function)
                .ok_or_else(|| anyhow!("missing callback dispatch cell {function}"))?
                .load()
                .context("load callback dispatch target")?;
            let code_version = runtime
                .code_versions
                .get(&(function, target.version))
                .ok_or_else(|| {
                    anyhow!(
                        "missing callback code version {function}:{}",
                        target.version
                    )
                })?;
            let frame = code_version
                .enter()
                .context("enter callback code version")?;
            runtime.callback_dispatches += 1;
            (target.address, frame)
        };
        // SAFETY: this trampoline is used only for zero-argument managed entries,
        // and publication occurs after code finalization.
        let callable: unsafe extern "C" fn(*mut BaselineRuntime) -> i64 =
            unsafe { core::mem::transmute(address) };
        // SAFETY: the target was published for this runtime and module lifetime.
        let value = unsafe { callable(runtime) };
        // SAFETY: the synchronous callable has returned while its runtime remains live.
        let runtime = unsafe { &mut *runtime };
        runtime.release_return_root();

        if runtime.data_roots.leave_frame().is_err() {
            runtime.fail(BaselineFailure::RuntimePanic);
        }
        drop(frame);
        Ok(value)
    }

    fn declare_runtime_functions<M: Module>(
        module: &mut M,
        ir: &IrModule,
        source_async: bool,
        c_runtime: bool,
    ) -> Result<RuntimeFunctions> {
        let pointer = types::I64;
        let mut owned_imports = HashSet::new();
        let mut declare =
            |module: &mut M, name: &str, parameters: usize, returns: bool| -> Result<FuncId> {
                let mut signature = module.make_signature();
                signature.call_conv = module.target_config().default_call_conv;
                signature
                    .params
                    .extend((0..parameters).map(|_| AbiParam::new(pointer)));
                if returns {
                    signature.returns.push(AbiParam::new(types::I64));
                }
                let id = module
                    .declare_function(name, Linkage::Import, &signature)
                    .with_context(|| format!("declare runtime function {name}"))?;
                owned_imports.insert(id);
                Ok(id)
            };
        let source_async = if source_async {
            Some(AsyncRuntimeFunctions {
                take_task_errors: declare(module, "rinzu_baseline_take_task_errors", 3, true)?,
                task_error_origin: declare(module, "rinzu_baseline_task_error_origin", 2, false)?,
                task_scope: declare(module, "rinzu_baseline_task_scope", 3, false)?,
                spawn: declare(module, "rinzu_baseline_async_spawn", 4, true)?,
                await_value: declare(module, "rinzu_baseline_async_await", 3, true)?,
                call: declare(module, "rinzu_baseline_async_call", 5, true)?,
                service: declare(module, "rinzu_baseline_async_service", 3, true)?,
                supplant_reserve: declare(module, "rinzu_baseline_supplant_reserve", 3, true)?,
                supplant_start: declare(module, "rinzu_baseline_supplant_start", 2, false)?,
                supplant_settle: declare(module, "rinzu_baseline_supplant_settle", 2, true)?,
            })
        } else {
            None
        };
        Ok(RuntimeFunctions {
            proven_owner_operations: if c_runtime {
                rinzu_compiler::proven_data_owner_operations(ir)
            } else {
                HashMap::new()
            },
            owner_helpers: if c_runtime {
                [
                    ("commit_roots_poll", 2),
                    ("commit_proven_roots_poll", 2),
                    ("commit_partial_roots_poll", 3),
                    ("publish_roots_poll", 3),
                    ("project", 5),
                    ("construct", 5),
                    ("data_operation", 6),
                    ("bytes_slice_payload", 5),
                    ("bytes_int_length", 2),
                    ("string_scalar_length", 2),
                    ("collection_length", 3),
                ]
                .into_iter()
                .map(|(name, count)| {
                    Ok((
                        name,
                        declare(
                            module,
                            &format!("rinzu_baseline_{name}_owner_proven"),
                            count,
                            true,
                        )?,
                    ))
                })
                .collect::<Result<_>>()?
            } else {
                BTreeMap::new()
            },
            fresh_return_consuming_awaits: if c_runtime {
                rinzu_compiler::fresh_return_consuming_awaits(ir)
            } else {
                HashMap::new()
            },
            proven_frame_roots: if c_runtime {
                rinzu_compiler::proven_frame_roots(ir)
            } else {
                HashMap::new()
            },
            proven_frame_memberships: if c_runtime {
                rinzu_compiler::proven_frame_memberships(ir)
            } else {
                HashMap::new()
            },
            data_schema: crate::data_schema::DataSchema::new(ir)?,
            reference_targets: crate::function_reference::ReferenceTargets::new(ir)?,
            native_plans: crate::native_call::NativeCallPlans::new(ir)?,
            native_helpers: [
                ("prepare", 3, false),
                ("address", 2, true),
                ("words", 2, true),
                ("returned", 3, false),
                ("close", 2, false),
                ("settle", 2, false),
                ("read", 2, true),
                ("release", 2, false),
            ]
            .into_iter()
            .map(|(name, parameters, returns)| {
                Ok((
                    name,
                    declare(
                        module,
                        &format!("rinzu_baseline_source_native_{name}"),
                        parameters,
                        returns,
                    )?,
                ))
            })
            .collect::<Result<_>>()?,
            native_stubs: crate::source_native_abi::declare(module, &ir.source_native_signatures)?
                .into_iter()
                .map(|(name, (id, _))| (name, id))
                .collect(),
            source_async,
            source_ids: source_identities(ir).0,
            set_source: declare(module, SOURCE_SYMBOL, 3, false)?,
            observe_source: declare(module, OBSERVE_SOURCE_SYMBOL, 1, false)?,
            failure: declare(module, FAILURE_SYMBOL, 1, true)?,
            entry: declare(module, ENTRY_SYMBOL, 2, true)?,
            publish_roots: declare(module, ROOTS_SYMBOL, 3, true)?,
            prepare_roots: declare(module, PREPARE_ROOTS_SYMBOL, 2, true)?,
            commit_roots: declare(module, COMMIT_ROOTS_SYMBOL, 2, true)?,
            commit_proven_roots: if c_runtime {
                Some(declare(module, COMMIT_PROVEN_ROOTS_SYMBOL, 2, true)?)
            } else {
                None
            },
            commit_partial_roots: if c_runtime {
                Some(declare(module, COMMIT_PARTIAL_ROOTS_SYMBOL, 3, true)?)
            } else {
                None
            },
            construct: declare(module, CONSTRUCT_SYMBOL, 5, true)?,
            data_operation: declare(module, DATA_OPERATION_SYMBOL, 6, true)?,
            bytes_slice_payload: if c_runtime {
                Some(declare(module, BYTES_SLICE_PAYLOAD_SYMBOL, 5, true)?)
            } else {
                None
            },
            bytes_int_length: declare(module, BYTES_INT_LENGTH_SYMBOL, 2, true)?,
            string_scalar_length: declare(module, STRING_SCALAR_LENGTH_SYMBOL, 2, true)?,
            collection_length: declare(module, COLLECTION_LENGTH_SYMBOL, 3, true)?,
            bytes: declare(module, BYTES_SYMBOL, 3, true)?,
            bytes_pad: declare(module, BYTES_PAD_SYMBOL, 4, true)?,
            bytes_length: declare(module, BYTES_LENGTH_SYMBOL, 3, true)?,
            store_field: declare(module, STORE_FIELD_SYMBOL, 5, true)?,
            project: declare(module, PROJECT_SYMBOL, 5, true)?,
            enum_variant: declare(module, ENUM_VARIANT_SYMBOL, 3, true)?,
            value_compare: declare(module, VALUE_COMPARE_SYMBOL, 5, true)?,
            arithmetic: declare(module, ARITHMETIC_SYMBOL, 4, true)?,
            scalar_arithmetic: declare(module, SCALAR_ARITHMETIC_SYMBOL, 5, true)?,
            scalar_compare: declare(module, SCALAR_COMPARE_SYMBOL, 5, true)?,
            scalar_convert: declare(module, SCALAR_CONVERT_SYMBOL, 4, true)?,
            has_error: declare(module, HAS_ERROR_SYMBOL, 1, true)?,
            catch_error: declare(module, CATCH_ERROR_SYMBOL, 1, true)?,
            string: declare(module, STRING_SYMBOL, 3, true)?,
            error_message: declare(module, ERROR_MESSAGE_SYMBOL, 2, true)?,
            error_data: declare(module, ERROR_DATA_SYMBOL, 7, true)?,
            stdout: declare(module, STDOUT_SYMBOL, 4, true)?,
            format_string: declare(module, FORMAT_STRING_SYMBOL, 4, true)?,
            utf8_length: declare(module, UTF8_LENGTH_SYMBOL, 2, true)?,
            task_control: declare(module, TASK_CONTROL_SYMBOL, 3, true)?,
            string_compare: declare(module, STRING_COMPARE_SYMBOL, 4, true)?,
            begin_cleanup: declare(module, BEGIN_CLEANUP_SYMBOL, 2, false)?,
            end_cleanup: declare(module, END_CLEANUP_SYMBOL, 2, false)?,
            resolve_dispatch: declare(module, RESOLVE_DISPATCH_SYMBOL, 2, true)?,
            leave_dispatch: declare(module, LEAVE_DISPATCH_SYMBOL, 1, false)?,
            retain_return: declare(module, "rinzu_baseline_retain_return", 2, false)?,
            native_invoke: declare(module, NATIVE_INVOKE_SYMBOL, 11, true)?,
            owned_imports,
        })
    }

    fn parameter_count(function: &IrFunction) -> Result<usize> {
        let mut indexes = function
            .blocks
            .iter()
            .flat_map(|block| &block.operations)
            .filter_map(|operation| match operation.kind {
                OperationKind::Parameter { index } => Some(index),
                _ => None,
            })
            .collect::<Vec<_>>();
        indexes.sort_unstable();
        for (expected, actual) in indexes.iter().copied().enumerate() {
            ensure!(
                expected as u32 == actual,
                "non-contiguous parameter indexes"
            );
        }
        Ok(indexes.len())
    }

    #[derive(Default)]
    struct CFunctions {
        source: String,
        names: Vec<String>,
        fallbacks: BTreeMap<String, String>,
    }

    #[allow(clippy::too_many_arguments)]
    fn define_function<M: Module>(
        module: &mut M,
        function: &IrFunction,
        function_index: usize,
        functions: &HashMap<String, FunctionInfo>,
        runtime_functions: &RuntimeFunctions,
        native_imports: &HashMap<String, u64>,
        colocate_runtime_calls: bool,
        evidence: &mut CompileEvidence,
        inspection: Option<&mut Vec<(String, String)>>,
        c_output: Option<&mut CFunctions>,
    ) -> Result<usize> {
        let info = functions
            .get(&function.name)
            .expect("all functions were declared");
        let mut context = module.make_context();
        context.func.name = UserFuncName::user(1, info.id.as_u32());
        context.func.signature = module
            .declarations()
            .get_function_decl(info.id)
            .signature
            .clone();
        let mut string_literals = HashMap::new();
        let mut literal_bytes = HashMap::new();
        for operation in function.blocks.iter().flat_map(|block| &block.operations) {
            let bytes = match &operation.kind {
                OperationKind::Constant(ConstantValue::String(text)) => text.as_bytes(),
                OperationKind::Constant(ConstantValue::Bytes(bytes)) => bytes.as_slice(),
                _ => continue,
            };
            let address = if bytes.is_empty() {
                None
            } else {
                let data = module.declare_anonymous_data(false, false)?;
                let mut description = DataDescription::new();
                description.define(bytes.to_vec().into_boxed_slice());
                module.define_data(data, &description)?;
                literal_bytes.insert(data.as_u32(), bytes.to_vec());
                Some(module.declare_data_in_func(data, &mut context.func))
            };
            string_literals.insert(operation.result.as_ref().unwrap().id, address);
        }
        let mut builder_context = FunctionBuilderContext::new();
        let c_managed_signatures;
        {
            let target_config = module.target_config();
            let source_async =
                runtime_functions
                    .source_async
                    .as_ref()
                    .map(|runtime| AsyncRuntimeRefs {
                        take_task_errors: module
                            .declare_func_in_func(runtime.take_task_errors, &mut context.func),
                        task_error_origin: module
                            .declare_func_in_func(runtime.task_error_origin, &mut context.func),
                        task_scope: module
                            .declare_func_in_func(runtime.task_scope, &mut context.func),
                        spawn: module.declare_func_in_func(runtime.spawn, &mut context.func),
                        await_value: module
                            .declare_func_in_func(runtime.await_value, &mut context.func),
                        call: module.declare_func_in_func(runtime.call, &mut context.func),
                        service: module.declare_func_in_func(runtime.service, &mut context.func),
                        supplant_reserve: module
                            .declare_func_in_func(runtime.supplant_reserve, &mut context.func),
                        supplant_start: module
                            .declare_func_in_func(runtime.supplant_start, &mut context.func),
                        supplant_settle: module
                            .declare_func_in_func(runtime.supplant_settle, &mut context.func),
                    });
            let runtime_refs = RuntimeFunctionRefs {
                proven_owner_operations: runtime_functions
                    .proven_owner_operations
                    .get(&function.name)
                    .cloned()
                    .unwrap_or_default(),
                owner_helpers: runtime_functions
                    .owner_helpers
                    .iter()
                    .map(|(name, id)| (*name, module.declare_func_in_func(*id, &mut context.func)))
                    .collect(),
                fresh_return_consuming_awaits: runtime_functions
                    .fresh_return_consuming_awaits
                    .get(&function.name)
                    .cloned()
                    .unwrap_or_default(),
                direct_source: colocate_runtime_calls || runtime_functions.source_async.is_some(),
                virtual_bytes_options: HashSet::new(),
                dead_temporary_roots: HashSet::new(),
                dead_local_roots: HashSet::new(),
                proven_frame_roots: runtime_functions
                    .proven_frame_roots
                    .get(&function.name)
                    .cloned()
                    .unwrap_or_default(),
                proven_frame_memberships: runtime_functions
                    .proven_frame_memberships
                    .get(&function.name)
                    .cloned()
                    .unwrap_or_default(),
                function_index,
                data_schema: runtime_functions.data_schema.clone(),
                reference_targets: runtime_functions.reference_targets.clone(),
                native_plans: runtime_functions.native_plans.clone(),
                supplant_values: Default::default(),
                native_helpers: runtime_functions
                    .native_helpers
                    .iter()
                    .map(|(name, id)| (*name, module.declare_func_in_func(*id, &mut context.func)))
                    .collect(),
                native_stubs: runtime_functions
                    .native_stubs
                    .iter()
                    .map(|(name, id)| {
                        (
                            name.clone(),
                            module.declare_func_in_func(*id, &mut context.func),
                        )
                    })
                    .collect(),
                source_async,
                unwind: None,
                source_ids: runtime_functions.source_ids.clone(),
                string_literals,
                set_source: module
                    .declare_func_in_func(runtime_functions.set_source, &mut context.func),
                observe_source: module
                    .declare_func_in_func(runtime_functions.observe_source, &mut context.func),
                failure: module.declare_func_in_func(runtime_functions.failure, &mut context.func),
                entry: module.declare_func_in_func(runtime_functions.entry, &mut context.func),
                publish_roots: module
                    .declare_func_in_func(runtime_functions.publish_roots, &mut context.func),
                prepare_roots: module
                    .declare_func_in_func(runtime_functions.prepare_roots, &mut context.func),
                commit_roots: module
                    .declare_func_in_func(runtime_functions.commit_roots, &mut context.func),
                commit_proven_roots: runtime_functions
                    .commit_proven_roots
                    .map(|id| module.declare_func_in_func(id, &mut context.func)),
                commit_partial_roots: runtime_functions
                    .commit_partial_roots
                    .map(|id| module.declare_func_in_func(id, &mut context.func)),
                construct: module
                    .declare_func_in_func(runtime_functions.construct, &mut context.func),
                data_operation: module
                    .declare_func_in_func(runtime_functions.data_operation, &mut context.func),
                bytes_slice_payload: runtime_functions
                    .bytes_slice_payload
                    .map(|id| module.declare_func_in_func(id, &mut context.func)),
                bytes_int_length: module
                    .declare_func_in_func(runtime_functions.bytes_int_length, &mut context.func),
                string_scalar_length: module.declare_func_in_func(
                    runtime_functions.string_scalar_length,
                    &mut context.func,
                ),
                collection_length: module
                    .declare_func_in_func(runtime_functions.collection_length, &mut context.func),
                bytes: module.declare_func_in_func(runtime_functions.bytes, &mut context.func),
                bytes_pad: module
                    .declare_func_in_func(runtime_functions.bytes_pad, &mut context.func),
                bytes_length: module
                    .declare_func_in_func(runtime_functions.bytes_length, &mut context.func),
                store_field: module
                    .declare_func_in_func(runtime_functions.store_field, &mut context.func),
                project: module.declare_func_in_func(runtime_functions.project, &mut context.func),
                enum_variant: module
                    .declare_func_in_func(runtime_functions.enum_variant, &mut context.func),
                value_compare: module
                    .declare_func_in_func(runtime_functions.value_compare, &mut context.func),
                scalar_arithmetic: module
                    .declare_func_in_func(runtime_functions.scalar_arithmetic, &mut context.func),
                scalar_compare: module
                    .declare_func_in_func(runtime_functions.scalar_compare, &mut context.func),
                arithmetic: module
                    .declare_func_in_func(runtime_functions.arithmetic, &mut context.func),
                scalar_convert: module
                    .declare_func_in_func(runtime_functions.scalar_convert, &mut context.func),
                has_error: module
                    .declare_func_in_func(runtime_functions.has_error, &mut context.func),
                catch_error: module
                    .declare_func_in_func(runtime_functions.catch_error, &mut context.func),
                string: module.declare_func_in_func(runtime_functions.string, &mut context.func),
                error_message: module
                    .declare_func_in_func(runtime_functions.error_message, &mut context.func),
                error_data: module
                    .declare_func_in_func(runtime_functions.error_data, &mut context.func),
                stdout: module.declare_func_in_func(runtime_functions.stdout, &mut context.func),
                format_string: module
                    .declare_func_in_func(runtime_functions.format_string, &mut context.func),
                task_control: module
                    .declare_func_in_func(runtime_functions.task_control, &mut context.func),
                utf8_length: module
                    .declare_func_in_func(runtime_functions.utf8_length, &mut context.func),
                string_compare: module
                    .declare_func_in_func(runtime_functions.string_compare, &mut context.func),
                begin_cleanup: module
                    .declare_func_in_func(runtime_functions.begin_cleanup, &mut context.func),
                end_cleanup: module
                    .declare_func_in_func(runtime_functions.end_cleanup, &mut context.func),
                resolve_dispatch: module
                    .declare_func_in_func(runtime_functions.resolve_dispatch, &mut context.func),
                leave_dispatch: module
                    .declare_func_in_func(runtime_functions.leave_dispatch, &mut context.func),
                retain_return: module
                    .declare_func_in_func(runtime_functions.retain_return, &mut context.func),
                native_invoke: module
                    .declare_func_in_func(runtime_functions.native_invoke, &mut context.func),
            };
            let managed_signatures = functions
                .iter()
                .map(|(name, info)| {
                    (
                        name.clone(),
                        context.func.import_signature(info.signature.clone()),
                    )
                })
                .collect::<HashMap<_, _>>();
            c_managed_signatures = managed_signatures.values().copied().collect::<HashSet<_>>();
            let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
            lower_function(
                &mut builder,
                function,
                function_index,
                functions,
                &managed_signatures,
                &runtime_refs,
                native_imports,
            )?;
            builder.finalize(target_config);
        }
        if colocate_runtime_calls {
            // AOT links these fixed helpers with their generated C runtime in
            // the same image. Keep Import declarations: C owns the definitions
            // and the async host must still be able to look up their exports.
            // JIT helper addresses can be arbitrarily far away and retain the
            // default lowering. Other external/native symbols are untouched.
            let names = context.func.params.user_named_funcs();
            for external in context.func.stencil.dfg.ext_funcs.values_mut() {
                let cranelift_codegen::ir::ExternalName::User(name) = &external.name else {
                    continue;
                };
                let name = &names[*name];
                if name.namespace == 0
                    && runtime_functions
                        .owned_imports
                        .contains(&FuncId::from_u32(name.index))
                {
                    external.colocated = true;
                }
            }
        }
        if let Some(rows) = inspection {
            let mut text = String::new();
            let mut opcodes = BTreeMap::<String, usize>::new();
            for block in context.func.layout.blocks() {
                for inst in context.func.layout.block_insts(block) {
                    *opcodes
                        .entry(context.func.dfg.insts[inst].opcode().to_string())
                        .or_default() += 1;
                }
            }
            for (opcode, count) in opcodes {
                text.push_str(&format!("; C07F opcode {opcode} {count}\n"));
            }
            for (reference, external) in context.func.dfg.ext_funcs.iter() {
                if let cranelift_codegen::ir::ExternalName::User(name) = &external.name {
                    let name = &context.func.params.user_named_funcs()[*name];
                    if name.namespace == 0 {
                        let declaration = module
                            .declarations()
                            .get_function_decl(FuncId::from_u32(name.index));
                        text.push_str(&format!(
                            "; C07F {reference} {}\n",
                            declaration.name.as_deref().unwrap_or("<anonymous>")
                        ));
                    }
                }
            }
            text.push_str(&context.func.display().to_string());
            rows.push((function.name.clone(), text));
            return Ok(0);
        }
        if let Some(output) = c_output {
            if function.is_async {
                output
                    .fallbacks
                    .insert(function.name.clone(), "async body".into());
            } else {
                let emitted = (|| -> Result<crate::c_lowering::Emission> {
                    let mut imports = HashMap::new();
                    for (reference, external) in context.func.dfg.ext_funcs.iter() {
                        if let cranelift_codegen::ir::ExternalName::User(name) = &external.name {
                            let name = &context.func.params.user_named_funcs()[*name];
                            if name.namespace == 0 {
                                let declaration = module
                                    .declarations()
                                    .get_function_decl(FuncId::from_u32(name.index));
                                imports.insert(
                                    reference,
                                    declaration
                                        .name
                                        .clone()
                                        .ok_or_else(|| anyhow!("anonymous C import"))?,
                                );
                            }
                        }
                    }
                    let mut literals = HashMap::new();
                    for (global, data) in context.func.global_values.iter() {
                        let cranelift_codegen::ir::GlobalValueData::Symbol {
                            name: cranelift_codegen::ir::ExternalName::User(name),
                            offset,
                            tls: false,
                            ..
                        } = data
                        else {
                            bail!("unsupported C global")
                        };
                        let name = &context.func.params.user_named_funcs()[*name];
                        ensure!(name.namespace == 1, "non-data C global");
                        literals.insert(
                            global,
                            (
                                literal_bytes
                                    .get(&name.index)
                                    .ok_or_else(|| anyhow!("nonliteral C global"))?
                                    .clone(),
                                offset.bits(),
                            ),
                        );
                    }
                    crate::c_lowering::emit(
                        &context.func,
                        &info.symbol,
                        &imports,
                        &literals,
                        &c_managed_signatures,
                        module.isa(),
                    )
                })();
                match emitted {
                    Ok(emission) => {
                        output.source.push_str(&format!(
                            "/* {} lowered SSA instructions */\n",
                            emission.instructions
                        ));
                        output.source.push_str(&emission.source);
                        output.names.push(function.name.clone());
                        return Ok(0);
                    }
                    Err(error) => {
                        output
                            .fallbacks
                            .insert(function.name.clone(), error.to_string());
                    }
                }
            }
        }
        if module.declarations().get_function_decl(info.id).linkage == Linkage::Import {
            module.declare_function(&info.symbol, Linkage::Export, &info.signature)?;
        }
        module
            .define_function(info.id, &mut context)
            .with_context(|| format!("compile baseline function {}", function.name))?;
        let compiled = context
            .compiled_code()
            .ok_or_else(|| anyhow!("Cranelift retained no code for {}", function.name))?;
        let code_size = compiled.code_buffer().len();
        evidence.code_size += code_size;
        evidence.stack_map_count += compiled.buffer.user_stack_maps().len();
        evidence.stack_map_entry_count += compiled
            .buffer
            .user_stack_maps()
            .iter()
            .map(|(_, _, map)| map.entries().count())
            .sum::<usize>();
        module.clear_context(&mut context);
        Ok(code_size)
    }

    #[cfg(feature = "test-c-lowering-probe")]
    pub(super) fn inspect_clif(ir: &IrModule) -> Result<Vec<(String, String)>> {
        let isa = super::super::native_isa(true)?;
        let builder =
            ObjectBuilder::new(isa, "rinzu-c-lowering-inventory", default_libcall_names())?;
        let mut module = ObjectModule::new(builder);
        let runtime = declare_runtime_functions(&mut module, ir, true, true)?;
        let functions = declare_managed_functions(&mut module, ir, Linkage::Export)?;
        let mut compile = CompileEvidence {
            stack_map_count: 0,
            stack_map_entry_count: 0,
            code_size: 0,
        };
        let mut rows = Vec::new();
        for (index, function) in ir.functions.iter().enumerate() {
            define_function(
                &mut module,
                function,
                index,
                &functions,
                &runtime,
                &HashMap::new(),
                true,
                &mut compile,
                Some(&mut rows),
                None,
            )?;
        }
        Ok(rows)
    }

    #[derive(Clone)]
    struct AsyncRuntimeRefs {
        take_task_errors: cranelift_codegen::ir::FuncRef,
        task_error_origin: cranelift_codegen::ir::FuncRef,
        task_scope: cranelift_codegen::ir::FuncRef,
        spawn: cranelift_codegen::ir::FuncRef,
        await_value: cranelift_codegen::ir::FuncRef,
        call: cranelift_codegen::ir::FuncRef,
        service: cranelift_codegen::ir::FuncRef,
        supplant_reserve: cranelift_codegen::ir::FuncRef,
        supplant_start: cranelift_codegen::ir::FuncRef,
        supplant_settle: cranelift_codegen::ir::FuncRef,
    }

    #[derive(Clone)]
    struct RuntimeFunctionRefs {
        proven_owner_operations: HashSet<(BlockId, usize)>,
        owner_helpers: BTreeMap<&'static str, cranelift_codegen::ir::FuncRef>,
        direct_source: bool,
        virtual_bytes_options: HashSet<ValueId>,
        dead_temporary_roots: HashSet<(BlockId, usize, ValueId)>,
        dead_local_roots: HashSet<(BlockId, usize, String)>,
        proven_frame_roots: HashSet<(BlockId, usize)>,
        proven_frame_memberships: HashMap<(BlockId, usize), u64>,
        fresh_return_consuming_awaits: HashSet<(BlockId, usize)>,
        function_index: usize,
        reference_targets: crate::function_reference::ReferenceTargets,
        native_plans: crate::native_call::NativeCallPlans,
        supplant_values: std::rc::Rc<
            std::cell::RefCell<
                HashMap<rinzu_compiler::ReservationId, cranelift_codegen::ir::Value>,
            >,
        >,
        native_helpers: BTreeMap<&'static str, cranelift_codegen::ir::FuncRef>,
        native_stubs: BTreeMap<String, cranelift_codegen::ir::FuncRef>,
        data_schema: crate::data_schema::DataSchema,
        source_async: Option<AsyncRuntimeRefs>,
        source_ids: HashMap<u32, u64>,
        set_source: cranelift_codegen::ir::FuncRef,
        observe_source: cranelift_codegen::ir::FuncRef,
        failure: cranelift_codegen::ir::FuncRef,
        entry: cranelift_codegen::ir::FuncRef,
        publish_roots: cranelift_codegen::ir::FuncRef,
        prepare_roots: cranelift_codegen::ir::FuncRef,
        commit_roots: cranelift_codegen::ir::FuncRef,
        commit_proven_roots: Option<cranelift_codegen::ir::FuncRef>,
        commit_partial_roots: Option<cranelift_codegen::ir::FuncRef>,
        construct: cranelift_codegen::ir::FuncRef,
        data_operation: cranelift_codegen::ir::FuncRef,
        bytes_slice_payload: Option<cranelift_codegen::ir::FuncRef>,
        bytes_int_length: cranelift_codegen::ir::FuncRef,
        string_scalar_length: cranelift_codegen::ir::FuncRef,
        collection_length: cranelift_codegen::ir::FuncRef,
        bytes: cranelift_codegen::ir::FuncRef,
        bytes_pad: cranelift_codegen::ir::FuncRef,
        bytes_length: cranelift_codegen::ir::FuncRef,
        store_field: cranelift_codegen::ir::FuncRef,
        project: cranelift_codegen::ir::FuncRef,
        enum_variant: cranelift_codegen::ir::FuncRef,
        value_compare: cranelift_codegen::ir::FuncRef,
        arithmetic: cranelift_codegen::ir::FuncRef,
        scalar_arithmetic: cranelift_codegen::ir::FuncRef,
        scalar_compare: cranelift_codegen::ir::FuncRef,
        scalar_convert: cranelift_codegen::ir::FuncRef,
        has_error: cranelift_codegen::ir::FuncRef,
        catch_error: cranelift_codegen::ir::FuncRef,
        string: cranelift_codegen::ir::FuncRef,
        error_message: cranelift_codegen::ir::FuncRef,
        error_data: cranelift_codegen::ir::FuncRef,
        stdout: cranelift_codegen::ir::FuncRef,
        format_string: cranelift_codegen::ir::FuncRef,
        utf8_length: cranelift_codegen::ir::FuncRef,
        task_control: cranelift_codegen::ir::FuncRef,
        string_compare: cranelift_codegen::ir::FuncRef,
        string_literals: HashMap<ValueId, Option<cranelift_codegen::ir::GlobalValue>>,
        begin_cleanup: cranelift_codegen::ir::FuncRef,
        end_cleanup: cranelift_codegen::ir::FuncRef,
        resolve_dispatch: cranelift_codegen::ir::FuncRef,
        leave_dispatch: cranelift_codegen::ir::FuncRef,
        retain_return: cranelift_codegen::ir::FuncRef,
        native_invoke: cranelift_codegen::ir::FuncRef,
        unwind: Option<UnwindRoutes>,
    }
    #[derive(Clone)]
    struct UnwindRoutes {
        variable: Variable,
        owner: Variable,
        dispatch: cranelift_codegen::ir::Block,
        paths: std::rc::Rc<std::cell::RefCell<Vec<Vec<BlockId>>>>,
        cleanup_scopes: HashSet<rinzu_compiler::SourceRange>,
        cleanup_ids: HashMap<rinzu_compiler::SourceRange, i64>,
        active_cleanup: std::rc::Rc<std::cell::RefCell<Vec<rinzu_compiler::SourceRange>>>,
        saved_routes: std::rc::Rc<std::cell::RefCell<HashMap<BlockId, (Variable, Variable)>>>,
        return_value: Variable,
        return_target: BlockId,
    }
    impl UnwindRoutes {
        fn id(&self, path: &[BlockId]) -> i64 {
            if path.is_empty() {
                return 0;
            }
            if let Some(index) = self
                .paths
                .borrow()
                .iter()
                .position(|existing| existing == path)
            {
                return index as i64 + 1;
            }
            self.id(&path[1..]);
            let mut paths = self.paths.borrow_mut();
            paths.push(path.to_vec());
            paths.len() as i64
        }
    }

    fn owner_helper(
        runtime: &RuntimeFunctionRefs,
        name: &str,
        checked: cranelift_codegen::ir::FuncRef,
        proven: bool,
    ) -> cranelift_codegen::ir::FuncRef {
        if proven {
            runtime.owner_helpers.get(name).copied().unwrap_or(checked)
        } else {
            checked
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_function(
        builder: &mut FunctionBuilder<'_>,
        function: &IrFunction,
        function_index: usize,
        functions: &HashMap<String, FunctionInfo>,
        managed_signatures: &HashMap<String, cranelift_codegen::ir::SigRef>,
        runtime: &RuntimeFunctionRefs,
        native_imports: &HashMap<String, u64>,
    ) -> Result<()> {
        let mut runtime = runtime.clone();
        if runtime.bytes_slice_payload.is_some() && runtime.direct_source {
            runtime.virtual_bytes_options = rinzu_compiler::virtual_bytes_options(function);
            runtime.dead_temporary_roots = rinzu_compiler::dead_temporary_roots(function);
            runtime.dead_local_roots = rinzu_compiler::dead_local_roots(function);
        }
        runtime.unwind = Some(UnwindRoutes {
            variable: builder.declare_var(types::I64),
            owner: builder.declare_var(types::I64),
            dispatch: builder.create_block(),
            paths: std::rc::Rc::new(std::cell::RefCell::new(Vec::new())),
            cleanup_ids: function
                .blocks
                .iter()
                .filter(|block| {
                    block
                        .operations
                        .iter()
                        .any(|op| matches!(op.kind, OperationKind::LeaveTry { .. }))
                })
                .map(|block| {
                    (
                        block.range,
                        ((function_index as u64) << 32 | u64::from(block.id.0)) as i64,
                    )
                })
                .collect(),
            cleanup_scopes: function
                .blocks
                .iter()
                .filter(|block| {
                    block
                        .operations
                        .iter()
                        .any(|op| matches!(op.kind, OperationKind::LeaveTry { .. }))
                })
                .map(|block| block.range)
                .collect(),
            active_cleanup: std::rc::Rc::new(std::cell::RefCell::new(Vec::new())),
            saved_routes: std::rc::Rc::new(std::cell::RefCell::new(HashMap::new())),
            return_value: builder.declare_var(types::I64),
            return_target: BlockId(
                function
                    .blocks
                    .iter()
                    .map(|block| block.id.0)
                    .max()
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or_else(|| anyhow!("no block identity remains for deferred return"))?,
            ),
        });
        let runtime = &runtime;
        let mut blocks = function
            .blocks
            .iter()
            .map(|block| (block.id, builder.create_block()))
            .collect::<HashMap<_, _>>();
        blocks.insert(
            runtime.unwind.as_ref().unwrap().return_target,
            builder.create_block(),
        );
        let entry = blocks[&function.entry];
        builder.append_block_params_for_function_params(entry);
        let context_value = builder.block_params(entry)[0];
        let local_names = function
            .blocks
            .iter()
            .flat_map(|block| &block.operations)
            .filter_map(|operation| match &operation.kind {
                OperationKind::DeclareLocal { name }
                | OperationKind::CheckLocal { name, .. }
                | OperationKind::LoadLocal { name }
                | OperationKind::StoreLocal { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect::<HashSet<_>>();
        let mut locals = HashMap::new();
        let mut bound_locals = HashMap::new();
        for name in local_names {
            bound_locals.insert(name.clone(), builder.declare_var(types::I64));
            locals.insert(name, builder.declare_var(types::I64));
        }

        let unwind = runtime.unwind.as_ref().unwrap();
        for block in &function.blocks {
            if block.operations.iter().any(|op| {
                matches!(
                    op.kind,
                    OperationKind::CatchError { .. }
                        | OperationKind::CatchErrors { .. }
                        | OperationKind::RecordTaskError { .. }
                )
            }) && block
                .entry_scopes
                .iter()
                .any(|scope| unwind.cleanup_scopes.contains(scope))
            {
                unwind.saved_routes.borrow_mut().insert(
                    block.id,
                    (
                        builder.declare_var(types::I64),
                        builder.declare_var(types::I64),
                    ),
                );
            }
        }
        builder.switch_to_block(entry);
        let zero = builder.ins().iconst(types::I64, 0);
        builder.def_var(runtime.unwind.as_ref().unwrap().variable, zero);
        for variable in locals.values().chain(bound_locals.values()) {
            builder.def_var(*variable, zero);
        }
        builder.def_var(runtime.unwind.as_ref().unwrap().owner, zero);
        builder.def_var(runtime.unwind.as_ref().unwrap().return_value, zero);
        for (route, owner) in unwind.saved_routes.borrow().values() {
            builder.def_var(*route, zero);
            builder.def_var(*owner, zero);
        }
        for variable in locals.values() {
            builder.def_var(*variable, zero);
        }
        let function_id = builder
            .ins()
            .iconst(types::I64, i64::try_from(function_index)?);
        let entered = builder
            .ins()
            .call(runtime.entry, &[context_value, function_id]);
        // The C runtime returns a context-owned source address with its immutable
        // trace mode in the low bit. No pointer into JIT data is retained.
        let descriptor = builder.inst_results(entered)[0];
        let source_region = builder.ins().band_imm_s(descriptor, -2);
        let source_traced = builder.ins().band_imm_s(descriptor, 1);

        let value_types: HashMap<_, _> = function
            .blocks
            .iter()
            .flat_map(|block| &block.operations)
            .filter_map(|operation| operation.result.as_ref())
            .map(|value| (value.id, value.ty.clone()))
            .collect();
        let cleanup_blocks = function
            .blocks
            .iter()
            .filter(|block| {
                block
                    .operations
                    .iter()
                    .any(|operation| matches!(operation.kind, OperationKind::LeaveTry { .. }))
            })
            .map(|block| block.id)
            .collect::<HashSet<_>>();
        let cleanup_scopes: HashMap<_, _> = function
            .blocks
            .iter()
            .filter(|block| cleanup_blocks.contains(&block.id))
            .map(|block| (block.range, block.id))
            .collect();
        let mut values = HashMap::new();
        for block in &function.blocks {
            let cl_block = blocks[&block.id];
            if block.id != function.entry {
                builder.switch_to_block(cl_block);
            }
            let begins_cleanup = cleanup_blocks.contains(&block.id);
            let is_cleanup = begins_cleanup
                || block
                    .entry_scopes
                    .iter()
                    .any(|scope| cleanup_scopes.contains_key(scope));
            let cleanup_exit = block.operations.iter().rev().find_map(|operation| {
                if let OperationKind::ExitScope { scope, .. } = &operation.kind {
                    cleanup_scopes.get(scope).copied()
                } else {
                    None
                }
            });
            if begins_cleanup {
                let region = builder
                    .ins()
                    .iconst(types::I64, unwind.cleanup_ids[&block.range]);
                builder
                    .ins()
                    .call(runtime.begin_cleanup, &[context_value, region]);
            }
            lower_block_operations(
                builder,
                block,
                function,
                functions,
                managed_signatures,
                runtime,
                native_imports,
                context_value,
                source_region,
                source_traced,
                &blocks,
                &locals,
                &bound_locals,
                &mut values,
                &value_types,
                is_cleanup,
            )?;
            lower_terminator(
                builder,
                block,
                function,
                runtime,
                context_value,
                &blocks,
                &values,
                cleanup_exit,
            )?;
        }
        let unwind = runtime.unwind.as_ref().unwrap();
        builder.switch_to_block(blocks[&unwind.return_target]);
        let returned = builder.use_var(unwind.return_value);
        builder.ins().return_(&[returned]);
        let paths = unwind.paths.borrow().clone();
        builder.switch_to_block(unwind.dispatch);
        let route = builder.use_var(unwind.variable);
        let mut dispatch = cranelift_frontend::Switch::new();
        let route_blocks: Vec<_> = paths.iter().map(|_| builder.create_block()).collect();
        for (index, block) in route_blocks.iter().enumerate() {
            dispatch.set_entry(index as u128 + 1, *block);
        }
        dispatch.emit(builder, route, blocks[&function.failure]);
        for (path, route_block) in paths.iter().zip(route_blocks) {
            builder.switch_to_block(route_block);
            let next = builder.ins().iconst(types::I64, unwind.id(&path[1..]));
            builder.def_var(unwind.variable, next);
            let owner = builder.ins().iconst(types::I64, i64::from(path[0].0) + 1);
            builder.def_var(unwind.owner, owner);
            builder.ins().jump(blocks[&path[0]], &[]);
        }
        builder.seal_all_blocks();
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_block_operations(
        builder: &mut FunctionBuilder<'_>,
        block: &BasicBlock,
        function: &IrFunction,
        functions: &HashMap<String, FunctionInfo>,
        managed_signatures: &HashMap<String, cranelift_codegen::ir::SigRef>,
        runtime: &RuntimeFunctionRefs,
        native_imports: &HashMap<String, u64>,
        context_value: Value,
        source_region: Value,
        source_traced: Value,
        blocks: &HashMap<BlockId, cranelift_codegen::ir::Block>,
        locals: &HashMap<String, Variable>,
        bound_locals: &HashMap<String, Variable>,
        values: &mut HashMap<ValueId, Value>,
        value_types: &HashMap<ValueId, rinzu_compiler::Type>,
        is_cleanup: bool,
    ) -> Result<()> {
        let unwind = runtime.unwind.as_ref().unwrap();
        *unwind.active_cleanup.borrow_mut() = block
            .entry_scopes
            .iter()
            .filter(|scope| unwind.cleanup_scopes.contains(scope))
            .copied()
            .collect();
        let mut cleanup_chain = Vec::new();
        for (operation_index, operation) in block.operations.iter().enumerate() {
            let owner_proven = runtime
                .proven_owner_operations
                .contains(&(block.id, operation_index));
            let source = operation.range;
            let identity = runtime
                .source_ids
                .get(&source.file_id.0)
                .copied()
                .unwrap_or(u64::from(source.file_id.0));
            ensure!(
                identity <= u64::from(u32::MAX) && source.start_byte <= source.end_byte,
                "invalid generated source position"
            );
            // These operations only select an SSA input or materialize bits:
            // no failure, allocation, callback or trace observation can occur.
            // An unwind annotation can add a runtime call below, so retain its
            // source update even for an otherwise pure operation.
            let records_source = operation.unwind.is_some()
                || !matches!(
                    &operation.kind,
                    OperationKind::Parameter { .. }
                        | OperationKind::Constant(
                            ConstantValue::Unit
                                | ConstantValue::Bool(_)
                                | ConstantValue::Int(_)
                                | ConstantValue::Float(_)
                        )
                );
            if records_source {
                let file = builder.ins().iconst(types::I64, identity as i64);
                let span = u64::from(source.start_byte) | (u64::from(source.end_byte) << 32);
                let span = builder.ins().iconst(types::I64, span as i64);
                if runtime.direct_source {
                    // Stores and their source positions are unchanged. Trace
                    // observation reads this same region only when enabled;
                    // it does not repeat the file/span setup or source stores.
                    let flags = cranelift_codegen::ir::MemFlagsData::new();
                    builder.ins().store(flags, file, source_region, 0);
                    builder.ins().store(flags, span, source_region, 8);
                    let present = builder.ins().iconst(types::I32, 1);
                    builder.ins().store(flags, present, source_region, 16);
                    let traced = builder.create_block();
                    let next = builder.create_block();
                    builder.set_cold_block(traced);
                    let observe = builder.ins().icmp_imm_s(IntCC::NotEqual, source_traced, 0);
                    builder.ins().brif(observe, traced, &[], next, &[]);
                    builder.switch_to_block(traced);
                    builder.ins().call(runtime.observe_source, &[context_value]);
                    builder.ins().jump(next, &[]);
                    builder.switch_to_block(next);
                } else {
                    builder
                        .ins()
                        .call(runtime.set_source, &[context_value, file, span]);
                }
            }
            match &operation.kind {
                OperationKind::EnterScope { scope } if unwind.cleanup_scopes.contains(scope) => {
                    unwind.active_cleanup.borrow_mut().push(*scope)
                }
                OperationKind::ExitScope { scope, .. } if unwind.cleanup_scopes.contains(scope) => {
                    unwind
                        .active_cleanup
                        .borrow_mut()
                        .retain(|active| active != scope)
                }
                _ => {}
            }
            if let Some(async_runtime) = &runtime.source_async
                && operation.unwind.is_some()
            {
                let future = if let OperationKind::Await { future, .. } = operation.kind {
                    Some(value_of(values, future)?)
                } else {
                    None
                };
                crate::task_error_origin::lower(
                    builder,
                    context_value,
                    future,
                    runtime.direct_source,
                    async_runtime.task_error_origin,
                );
            }
            let result = match &operation.kind {
                OperationKind::Parameter { index } => {
                    Some(builder.block_params(blocks[&function.entry])[*index as usize + 1])
                }
                OperationKind::Constant(value) => {
                    let result = operation.result.as_ref().unwrap();
                    let lowered = lower_constant(
                        builder,
                        value,
                        &result.ty,
                        result.id,
                        runtime,
                        context_value,
                    )?;
                    if matches!(value, ConstantValue::String(_) | ConstantValue::Bytes(_)) {
                        branch_after_runtime_call(
                            builder,
                            runtime,
                            context_value,
                            blocks,
                            function.failure,
                            &cleanup_chain,
                        );
                    }
                    Some(lowered)
                }
                OperationKind::DeclareLocal { name } => {
                    let zero = builder.ins().iconst(types::I64, 0);
                    builder.def_var(locals[name], zero);
                    builder.def_var(bound_locals[name], zero);
                    None
                }
                OperationKind::CheckLocal {
                    name,
                    error,
                    cleanup_chain: operation_cleanup,
                    ..
                } => {
                    let bound = builder.use_var(bound_locals[name]);
                    let opcode = builder.ins().iconst(types::I64, 11);
                    let zero = builder.ins().iconst(types::I64, 0);
                    builder.ins().call(
                        runtime.error_data,
                        &[context_value, opcode, zero, bound, zero, zero, zero],
                    );
                    branch_after_error_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        error,
                        function.failure,
                        operation_cleanup,
                    )?;
                    None
                }
                OperationKind::TakeTaskErrors { scope } => {
                    let async_runtime = runtime.source_async.as_ref().unwrap();
                    let identity = builder.ins().iconst(
                        types::I64,
                        ((u64::from(scope.start_byte) << 32) | u64::from(scope.end_byte)) as i64,
                    );
                    let ty = builder.ins().iconst(
                        types::I64,
                        i64::try_from(
                            runtime
                                .data_schema
                                .index(&Type::List(Box::new(Type::Error)))?,
                        )?,
                    );
                    let call = builder.ins().call(
                        async_runtime.take_task_errors,
                        &[context_value, identity, ty],
                    );
                    Some(builder.inst_results(call)[0])
                }
                OperationKind::LoadLocal { name } => Some(builder.use_var(locals[name])),
                OperationKind::StoreField {
                    operand,
                    identity,
                    field,
                    value,
                } => {
                    let operand = value_of(values, *operand)?;
                    let value = value_of(values, *value)?;
                    let ty = runtime.data_schema.index(&Type::Named(identity.clone()))?;
                    let ty = builder.ins().iconst(types::I64, i64::try_from(ty)?);
                    let field = builder.ins().iconst(types::I64, i64::from(*field));
                    builder.ins().call(
                        runtime.store_field,
                        &[context_value, operand, ty, field, value],
                    );
                    branch_after_runtime_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        function.failure,
                        &cleanup_chain,
                    );
                    None
                }
                OperationKind::EnumPayload { operand, .. }
                    if runtime.virtual_bytes_options.contains(operand) =>
                {
                    // Verification and the leaf def-use plan prove the matching
                    // Some edge. The same managed payload remains a typed root.
                    let value = value_of(values, *operand)?;
                    branch_after_runtime_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        function.failure,
                        &cleanup_chain,
                    );
                    Some(value)
                }
                OperationKind::ListElement { operand, index } => {
                    let ty = runtime.data_schema.index(&value_types[operand])?;
                    let operand = value_of(values, *operand)?;
                    let index = value_of(values, *index)?;
                    let ty = builder.ins().iconst(types::I64, i64::try_from(ty)?);
                    let variant = builder.ins().iconst(types::I64, -1);
                    let call = builder.ins().call(
                        owner_helper(runtime, "project", runtime.project, owner_proven),
                        &[context_value, operand, ty, variant, index],
                    );
                    let result = builder.inst_results(call)[0];
                    branch_after_runtime_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        function.failure,
                        &cleanup_chain,
                    );
                    Some(result)
                }
                OperationKind::Field {
                    operand,
                    identity,
                    field,
                    ..
                }
                | OperationKind::EnumPayload {
                    operand,
                    identity,
                    field,
                    ..
                } => {
                    let variant = match &operation.kind {
                        OperationKind::EnumPayload { variant, .. } => i64::from(*variant),
                        _ => -1,
                    };
                    let ty = runtime.data_schema.index(&Type::Named(identity.clone()))?;
                    let operand = value_of(values, *operand)?;
                    let ty = builder.ins().iconst(types::I64, i64::try_from(ty)?);
                    let variant = builder.ins().iconst(types::I64, variant);
                    let field = builder.ins().iconst(types::I64, i64::from(*field));
                    let call = builder.ins().call(
                        owner_helper(runtime, "project", runtime.project, owner_proven),
                        &[context_value, operand, ty, variant, field],
                    );
                    let result = builder.inst_results(call)[0];
                    if let OperationKind::Field {
                        error: Some(error), ..
                    } = &operation.kind
                    {
                        branch_after_error_call(
                            builder,
                            runtime,
                            context_value,
                            blocks,
                            error,
                            function.failure,
                            &cleanup_chain,
                        )?;
                    } else {
                        branch_after_runtime_call(
                            builder,
                            runtime,
                            context_value,
                            blocks,
                            function.failure,
                            &cleanup_chain,
                        );
                    }
                    Some(result)
                }
                OperationKind::StaticHandlerReference { target } => {
                    let (_, function) = find_function(functions, target)?;
                    Some(
                        builder
                            .ins()
                            .iconst(types::I64, i64::try_from(function.index)?),
                    )
                }
                OperationKind::StoreLocal { name, value } => {
                    builder.def_var(locals[name], value_of(values, *value)?);
                    let bound = builder.ins().iconst(types::I64, 1);
                    builder.def_var(bound_locals[name], bound);
                    None
                }
                OperationKind::Unary {
                    operator,
                    operand,
                    cleanup_chain: operation_cleanup,
                    ..
                } => {
                    let operand = value_of(values, *operand)?;
                    match operator {
                        UnaryOperator::Not => {
                            let one = builder.ins().iconst(types::I64, 1);
                            Some(builder.ins().bxor(operand, one))
                        }
                        UnaryOperator::Negate => {
                            let code = builder.ins().iconst(types::I64, OP_NEGATE);
                            let zero = builder.ins().iconst(types::I64, 0);
                            let scalar = operation
                                .result
                                .as_ref()
                                .unwrap()
                                .ty
                                .scalar()
                                .ok_or_else(|| anyhow!("numeric negate has no scalar type"))?;
                            let kind = builder.ins().iconst(
                                types::I64,
                                i64::from(crate::scalar_backend::kind(scalar) as u32),
                            );
                            let call = builder.ins().call(
                                runtime.scalar_arithmetic,
                                &[context_value, kind, code, operand, zero],
                            );
                            let value = builder.inst_results(call)[0];
                            branch_after_runtime_call(
                                builder,
                                runtime,
                                context_value,
                                blocks,
                                function.failure,
                                operation_cleanup,
                            );
                            Some(value)
                        }
                    }
                }
                OperationKind::Binary {
                    operator,
                    left,
                    right,
                    cleanup_chain: operation_cleanup,
                    ..
                } => Some(lower_binary(
                    builder,
                    *operator,
                    value_of(values, *left)?,
                    value_of(values, *right)?,
                    &value_types[left],
                    runtime,
                    context_value,
                    blocks,
                    function.failure,
                    operation_cleanup,
                )),
                OperationKind::Spawn {
                    target,
                    arguments,
                    cleanup_chain: operation_cleanup,
                    ..
                } if runtime.source_async.is_some() => {
                    let (_, function_info) = find_function(functions, target)?;
                    ensure!(
                        function_info.parameter_count == arguments.len(),
                        "Worker argument count mismatch for {target}"
                    );
                    let arguments = arguments
                        .iter()
                        .map(|argument| value_of(values, *argument))
                        .collect::<Result<Vec<_>>>()?;
                    let address = stack_array(builder, &arguments)?;
                    let count = builder
                        .ins()
                        .iconst(types::I64, i64::try_from(arguments.len())?);
                    let index = builder
                        .ins()
                        .iconst(types::I64, i64::try_from(function_info.index)?);
                    let call = builder.ins().call(
                        runtime.source_async.as_ref().unwrap().spawn,
                        &[context_value, index, address, count],
                    );
                    let value = builder.inst_results(call)[0];
                    branch_after_runtime_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        function.failure,
                        operation_cleanup,
                    );
                    Some(value)
                }
                OperationKind::ManagedCall {
                    target,
                    arguments,
                    error,
                    cleanup_chain: operation_cleanup,
                    ..
                } if runtime.source_async.is_some()
                    && find_function(functions, target)?.1.is_async =>
                {
                    let (_, function_info) = find_function(functions, target)?;
                    ensure!(
                        function_info.parameter_count == arguments.len(),
                        "async call argument count mismatch for {target}"
                    );
                    let arguments = arguments
                        .iter()
                        .map(|argument| value_of(values, *argument))
                        .collect::<Result<Vec<_>>>()?;
                    let address = stack_array(builder, &arguments)?;
                    let count = builder
                        .ins()
                        .iconst(types::I64, i64::try_from(arguments.len())?);
                    let index = builder
                        .ins()
                        .iconst(types::I64, i64::try_from(function_info.index)?);
                    let Type::Future(inner) = &operation.result.as_ref().unwrap().ty else {
                        unreachable!()
                    };
                    let kind = builder
                        .ins()
                        .iconst(types::I64, async_transfer_kind(inner)?);
                    let call = builder.ins().call(
                        runtime.source_async.as_ref().unwrap().call,
                        &[context_value, index, address, count, kind],
                    );
                    let value = builder.inst_results(call)[0];
                    branch_after_error_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        error,
                        function.failure,
                        operation_cleanup,
                    )?;
                    Some(value)
                }
                OperationKind::ManagedCall {
                    target,
                    arguments,
                    cleanup_chain: operation_cleanup,
                    ..
                }
                | OperationKind::Spawn {
                    target,
                    arguments,
                    cleanup_chain: operation_cleanup,
                    ..
                } => {
                    let (name, function_info) = find_function(functions, target)?;
                    ensure!(
                        function_info.parameter_count == arguments.len(),
                        "managed call argument count mismatch for {target}"
                    );
                    let mut call_arguments = vec![context_value];
                    call_arguments.extend(
                        arguments
                            .iter()
                            .map(|argument| value_of(values, *argument))
                            .collect::<Result<Vec<_>>>()?,
                    );
                    let function_index = builder
                        .ins()
                        .iconst(types::I64, i64::try_from(function_info.index)?);
                    let resolve = builder
                        .ins()
                        .call(runtime.resolve_dispatch, &[context_value, function_index]);
                    let address = builder.inst_results(resolve)[0];
                    let call = builder.ins().call_indirect(
                        managed_signatures[name],
                        address,
                        &call_arguments,
                    );
                    let value = builder.inst_results(call)[0];
                    builder.ins().call(runtime.leave_dispatch, &[context_value]);
                    if let OperationKind::ManagedCall { error, .. } = &operation.kind {
                        branch_after_error_call(
                            builder,
                            runtime,
                            context_value,
                            blocks,
                            error,
                            function.failure,
                            operation_cleanup,
                        )?;
                    }
                    branch_after_runtime_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        function.failure,
                        operation_cleanup,
                    );
                    Some(value)
                }
                OperationKind::TaskControl { kind, future } => {
                    let kind = builder.ins().iconst(types::I64, *kind as i64);
                    let future = if let Some(id) = future {
                        value_of(values, *id)?
                    } else {
                        builder.ins().iconst(types::I64, 0)
                    };
                    let call = builder
                        .ins()
                        .call(runtime.task_control, &[context_value, kind, future]);
                    Some(builder.inst_results(call)[0])
                }
                OperationKind::DataOperation {
                    kind,
                    arguments,
                    argument_types,
                    error,
                    ..
                } => {
                    let argument_values = arguments
                        .iter()
                        .map(|id| value_of(values, *id))
                        .collect::<Result<Vec<_>>>()?;
                    // The verified closed signatures make scalar lengths leaf
                    // calls. Keep roots/source/error routing at this same IR site;
                    // only the compiler-owned metadata arrays and dispatch go.
                    let call = match kind {
                        rinzu_compiler::DataOperation::BytesSlice
                            if operation
                                .result
                                .as_ref()
                                .is_some_and(|v| runtime.virtual_bytes_options.contains(&v.id)) =>
                        {
                            let result = runtime
                                .data_schema
                                .index(&operation.result.as_ref().unwrap().ty)?;
                            let result = builder.ins().iconst(types::I64, i64::try_from(result)?);
                            builder.ins().call(
                                owner_helper(
                                    runtime,
                                    "bytes_slice_payload",
                                    runtime.bytes_slice_payload.expect("C bytes-slice entry"),
                                    owner_proven,
                                ),
                                &[
                                    context_value,
                                    result,
                                    argument_values[0],
                                    argument_values[1],
                                    argument_values[2],
                                ],
                            )
                        }
                        rinzu_compiler::DataOperation::BytesLength => builder.ins().call(
                            owner_helper(
                                runtime,
                                "bytes_int_length",
                                runtime.bytes_int_length,
                                owner_proven,
                            ),
                            &[context_value, argument_values[0]],
                        ),
                        rinzu_compiler::DataOperation::StringLength => builder.ins().call(
                            owner_helper(
                                runtime,
                                "string_scalar_length",
                                runtime.string_scalar_length,
                                owner_proven,
                            ),
                            &[context_value, argument_values[0]],
                        ),
                        rinzu_compiler::DataOperation::ListLength
                        | rinzu_compiler::DataOperation::MapLength => {
                            let ty = runtime.data_schema.index(&argument_types[0])?;
                            let ty = builder.ins().iconst(types::I64, i64::try_from(ty)?);
                            builder.ins().call(
                                owner_helper(
                                    runtime,
                                    "collection_length",
                                    runtime.collection_length,
                                    owner_proven,
                                ),
                                &[context_value, argument_values[0], ty],
                            )
                        }
                        _ => {
                            let result = builder.ins().iconst(
                                types::I64,
                                i64::try_from(
                                    runtime
                                        .data_schema
                                        .index(&operation.result.as_ref().unwrap().ty)?,
                                )?,
                            );
                            let op = builder.ins().iconst(types::I64, *kind as i64);
                            let argument_array = stack_array(builder, &argument_values)?;
                            let type_values = argument_types
                                .iter()
                                .map(|ty| {
                                    Ok(builder.ins().iconst(
                                        types::I64,
                                        i64::try_from(runtime.data_schema.index(ty)?)?,
                                    ))
                                })
                                .collect::<Result<Vec<_>>>()?;
                            let type_array = stack_array(builder, &type_values)?;
                            let count = builder.ins().iconst(types::I64, arguments.len() as i64);
                            builder.ins().call(
                                owner_helper(
                                    runtime,
                                    "data_operation",
                                    runtime.data_operation,
                                    owner_proven,
                                ),
                                &[context_value, op, result, type_array, argument_array, count],
                            )
                        }
                    };
                    let value = builder.inst_results(call)[0];
                    branch_after_error_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        error,
                        function.failure,
                        &cleanup_chain,
                    )?;
                    Some(value)
                }
                OperationKind::Construct {
                    arguments,
                    variant,
                    cleanup_chain: operation_cleanup,
                    ..
                } => {
                    let type_id = builder.ins().iconst(
                        types::I64,
                        i64::try_from(
                            runtime
                                .data_schema
                                .index(&operation.result.as_ref().unwrap().ty)?,
                        )?,
                    );
                    let fields = builder
                        .ins()
                        .iconst(types::I64, i64::try_from(arguments.len())?);
                    let argument_values = arguments
                        .iter()
                        .map(|value| value_of(values, *value))
                        .collect::<Result<Vec<_>>>()?;
                    let argument_array = stack_array(builder, &argument_values)?;
                    let variant = builder
                        .ins()
                        .iconst(types::I64, variant.map(i64::from).unwrap_or(-1));
                    let call = builder.ins().call(
                        owner_helper(runtime, "construct", runtime.construct, owner_proven),
                        &[context_value, type_id, variant, argument_array, fields],
                    );
                    let value = builder.inst_results(call)[0];
                    branch_after_runtime_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        function.failure,
                        operation_cleanup,
                    );
                    Some(value)
                }
                OperationKind::CancellationPoll {
                    cleanup_chain: operation_cleanup,
                } => {
                    cleanup_chain = operation_cleanup.clone();
                    None
                }
                OperationKind::GcSafepoint { stack_map, .. } => {
                    let mut live = Vec::new();
                    for root in &stack_map.roots {
                        if let StackRoot::Value { value, .. } = root
                            && runtime.dead_temporary_roots.contains(&(
                                block.id,
                                operation_index,
                                *value,
                            ))
                        {
                            continue;
                        }
                        if let StackRoot::Local { name, .. } = root
                            && runtime.dead_local_roots.contains(&(
                                block.id,
                                operation_index,
                                name.clone(),
                            ))
                        {
                            continue;
                        }
                        let root = match root {
                            StackRoot::Value { value, .. } => value_of(values, *value)?,
                            StackRoot::Local { name, .. } => builder.use_var(locals[name]),
                        };
                        builder.declare_value_needs_stack_map(root);
                        live.push(root);
                    }
                    let count = builder.ins().iconst(types::I64, i64::try_from(live.len())?);
                    let status =
                        if live.is_empty() {
                            // The compiler knows there is nothing to store. Keep the
                            // safepoint checks/GC without a prepare/commit round trip.
                            let empty = builder.ins().iconst(types::I64, 0);
                            let call = builder.ins().call(
                                owner_helper(
                                    runtime,
                                    "publish_roots_poll",
                                    runtime.publish_roots,
                                    owner_proven,
                                ),
                                &[context_value, empty, count],
                            );
                            builder.inst_results(call)[0]
                        } else {
                            let prepared = builder
                                .ins()
                                .call(runtime.prepare_roots, &[context_value, count]);
                            let address = builder.inst_results(prepared)[0];
                            // The compiler owns this region. Only aligned root stores
                            // may appear between prepare and commit: no GC or reentry.
                            for (index, root) in live.iter().enumerate() {
                                builder.ins().store(
                                    cranelift_codegen::ir::MemFlagsData::new(),
                                    *root,
                                    address,
                                    i32::try_from(index.checked_mul(8).ok_or_else(|| {
                                        anyhow!("managed root buffer is too large")
                                    })?)?,
                                );
                            }
                            let known = runtime
                                .proven_frame_memberships
                                .get(&(block.id, operation_index))
                                .copied()
                                .unwrap_or(0);
                            let all = if live.len() == 64 {
                                u64::MAX
                            } else if live.len() < 64 {
                                (1_u64 << live.len()) - 1
                            } else {
                                0
                            };
                            let (name, commit) = if runtime
                                .proven_frame_roots
                                .contains(&(block.id, operation_index))
                                || (all != 0 && known == all)
                            {
                                (
                                    "commit_proven_roots_poll",
                                    runtime
                                        .commit_proven_roots
                                        .expect("C membership proof entry"),
                                )
                            } else if known != 0 && all != 0 {
                                (
                                    "commit_partial_roots_poll",
                                    runtime
                                        .commit_partial_roots
                                        .expect("C partial membership proof entry"),
                                )
                            } else {
                                ("commit_roots_poll", runtime.commit_roots)
                            };
                            let mut arguments = vec![context_value, count];
                            if name == "commit_partial_roots_poll" {
                                arguments.push(builder.ins().iconst(types::I64, known as i64));
                            }
                            let call = builder.ins().call(
                                owner_helper(runtime, name, commit, owner_proven),
                                &arguments,
                            );
                            builder.inst_results(call)[0]
                        };
                    if !is_cleanup {
                        branch_after_runtime_status(
                            builder,
                            runtime,
                            context_value,
                            status,
                            blocks,
                            function.failure,
                            &cleanup_chain,
                        );
                    }
                    None
                }
                OperationKind::NativeCall {
                    target,
                    arguments,
                    cleanup_chain: operation_cleanup,
                    ..
                } if native_imports.contains_key(target) => {
                    ensure!(
                        arguments.len() <= 8,
                        "native package calls accept at most eight scalar arguments"
                    );
                    let key = native_imports[target];
                    let key = builder
                        .ins()
                        .iconst(types::I64, i64::from_ne_bytes(key.to_ne_bytes()));
                    let count = builder
                        .ins()
                        .iconst(types::I64, i64::try_from(arguments.len())?);
                    let zero = builder.ins().iconst(types::I64, 0);
                    let mut native_arguments = vec![context_value, key, count];
                    native_arguments.extend(
                        arguments
                            .iter()
                            .map(|argument| value_of(values, *argument))
                            .collect::<Result<Vec<_>>>()?,
                    );
                    native_arguments.resize(11, zero);
                    let call = builder.ins().call(runtime.native_invoke, &native_arguments);
                    let value = builder.inst_results(call)[0];
                    branch_after_runtime_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        function.failure,
                        operation_cleanup,
                    );
                    Some(value)
                }
                OperationKind::FormatString { parts, error, .. } => {
                    let mut words = Vec::with_capacity(parts.len());
                    let mut indices = Vec::with_capacity(parts.len());
                    for (value, ty) in parts {
                        words.push(value_of(values, *value)?);
                        let index = runtime.data_schema.index(ty)?;
                        indices.push(builder.ins().iconst(types::I64, i64::try_from(index)?));
                    }
                    let words = stack_array(builder, &words)?;
                    let indices = stack_array(builder, &indices)?;
                    let count = builder
                        .ins()
                        .iconst(types::I64, i64::try_from(parts.len())?);
                    let call = builder.ins().call(
                        runtime.format_string,
                        &[context_value, words, indices, count],
                    );
                    let result = builder.inst_results(call)[0];
                    if let Some(error) = error {
                        branch_after_error_call(
                            builder,
                            runtime,
                            context_value,
                            blocks,
                            error,
                            function.failure,
                            &cleanup_chain,
                        )?;
                    } else {
                        branch_after_runtime_call(
                            builder,
                            runtime,
                            context_value,
                            blocks,
                            function.failure,
                            &cleanup_chain,
                        );
                    }
                    Some(result)
                }
                OperationKind::Stdout {
                    operand,
                    newline,
                    error,
                    cleanup_chain: operation_cleanup,
                    ..
                } => {
                    let ty = runtime.data_schema.index(&value_types[operand])?;
                    let ty = builder.ins().iconst(types::I64, i64::try_from(ty)?);
                    let operand = value_of(values, *operand)?;
                    let newline = builder.ins().iconst(types::I64, i64::from(*newline));
                    let call = builder
                        .ins()
                        .call(runtime.stdout, &[context_value, operand, ty, newline]);
                    let value = builder.inst_results(call)[0];
                    branch_after_error_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        error,
                        function.failure,
                        operation_cleanup,
                    )?;
                    Some(value)
                }
                OperationKind::EnterErrorGroup { .. }
                | OperationKind::FinishErrorGroup { .. }
                | OperationKind::RecordTaskError { .. }
                | OperationKind::ThrowError { .. }
                | OperationKind::CatchErrors { .. }
                | OperationKind::ErrorIs { .. }
                | OperationKind::ErrorCast { .. }
                | OperationKind::ErrorField { .. }
                | OperationKind::ErrorMatch { .. }
                | OperationKind::ErrorIndex { .. }
                | OperationKind::ErrorRest { .. } => {
                    if matches!(
                        operation.kind,
                        OperationKind::CatchErrors { .. } | OperationKind::RecordTaskError { .. }
                    ) && let Some((route, owner)) = unwind.saved_routes.borrow().get(&block.id)
                    {
                        let route = builder.use_var(*route);
                        let owner = builder.use_var(*owner);
                        builder.def_var(unwind.variable, route);
                        builder.def_var(unwind.owner, owner);
                    }
                    let (opcode, operand, argument, expected) = match &operation.kind {
                        OperationKind::EnterErrorGroup { scope } => (
                            8,
                            None,
                            ((u64::from(scope.start_byte) << 32) | u64::from(scope.end_byte))
                                as i64,
                            Vec::new(),
                        ),
                        OperationKind::RecordTaskError { scope } => (
                            9,
                            None,
                            ((u64::from(scope.start_byte) << 32) | u64::from(scope.end_byte))
                                as i64,
                            Vec::new(),
                        ),
                        OperationKind::FinishErrorGroup { scope, .. } => (
                            10,
                            None,
                            ((u64::from(scope.start_byte) << 32) | u64::from(scope.end_byte))
                                as i64,
                            Vec::new(),
                        ),
                        OperationKind::ErrorIs { operand, target } => (
                            7,
                            Some(*operand),
                            i64::try_from(runtime.data_schema.index(target)?)?,
                            Vec::new(),
                        ),
                        OperationKind::ErrorCast { operand } => (0, Some(*operand), 0, Vec::new()),
                        OperationKind::ErrorField { error, field } => {
                            (1, Some(*error), i64::from(*field), Vec::new())
                        }
                        OperationKind::ErrorMatch {
                            errors,
                            types,
                            rest,
                        } => (
                            2,
                            Some(*errors),
                            i64::from(*rest),
                            types
                                .iter()
                                .map(|ty| runtime.data_schema.index(ty))
                                .collect::<Result<Vec<_>>>()?,
                        ),
                        OperationKind::ErrorIndex { errors, index } => {
                            (3, Some(*errors), i64::from(*index), Vec::new())
                        }
                        OperationKind::ErrorRest { errors, start } => {
                            (4, Some(*errors), i64::from(*start), Vec::new())
                        }
                        OperationKind::CatchErrors { .. } => (5, None, 0, Vec::new()),
                        OperationKind::ThrowError { value, .. } => (6, Some(*value), 0, Vec::new()),
                        _ => unreachable!(),
                    };
                    let ty = runtime.data_schema.index(
                        operation
                            .result
                            .as_ref()
                            .map_or(&Type::Error, |result| &result.ty),
                    )?;
                    let type_id = builder.ins().iconst(types::I64, i64::try_from(ty)?);
                    let value =
                        if let OperationKind::FinishErrorGroup { handles, .. } = operation.kind {
                            builder.ins().iconst(types::I64, i64::from(handles))
                        } else if let Some(operand) = operand {
                            value_of(values, operand)?
                        } else {
                            builder.ins().iconst(types::I64, 0)
                        };
                    let argument = builder.ins().iconst(types::I64, argument);
                    let length = builder
                        .ins()
                        .iconst(types::I64, i64::try_from(expected.len())?);
                    let expected = expected
                        .iter()
                        .map(|ty| Ok(builder.ins().iconst(types::I64, i64::try_from(*ty)?)))
                        .collect::<Result<Vec<_>>>()?;
                    let array = stack_array(builder, &expected)?;
                    let opcode = builder.ins().iconst(types::I64, opcode);
                    let call = builder.ins().call(
                        runtime.error_data,
                        &[
                            context_value,
                            opcode,
                            type_id,
                            value,
                            argument,
                            array,
                            length,
                        ],
                    );
                    let value = builder.inst_results(call)[0];
                    if let OperationKind::ThrowError {
                        error,
                        cleanup_chain,
                        ..
                    } = &operation.kind
                    {
                        branch_after_error_call(
                            builder,
                            runtime,
                            context_value,
                            blocks,
                            error,
                            function.failure,
                            cleanup_chain,
                        )?;
                    } else {
                        branch_after_runtime_call(
                            builder,
                            runtime,
                            context_value,
                            blocks,
                            function.failure,
                            &cleanup_chain,
                        );
                    }
                    operation.result.as_ref().map(|_| value)
                }
                OperationKind::ErrorMessage { error } => {
                    let error = value_of(values, *error)?;
                    let call = builder
                        .ins()
                        .call(runtime.error_message, &[context_value, error]);
                    let value = builder.inst_results(call)[0];
                    branch_after_runtime_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        function.failure,
                        &cleanup_chain,
                    );
                    Some(value)
                }
                OperationKind::CoreLength {
                    operand,
                    kind: rinzu_compiler::CoreLengthKind::Utf8,
                } => {
                    let operand = value_of(values, *operand)?;
                    let call = builder
                        .ins()
                        .call(runtime.utf8_length, &[context_value, operand]);
                    let value = builder.inst_results(call)[0];
                    branch_after_runtime_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        function.failure,
                        &cleanup_chain,
                    );
                    Some(value)
                }
                OperationKind::CoreLength {
                    operand,
                    kind: rinzu_compiler::CoreLengthKind::Bytes,
                } => {
                    let expected = match value_types[operand] {
                        Type::FixedBytes(n) => n,
                        Type::Bytes => u64::MAX,
                        _ => unreachable!("verified byte length"),
                    };
                    let operand = value_of(values, *operand)?;
                    let expected = builder.ins().iconst(types::I64, expected as i64);
                    let call = builder
                        .ins()
                        .call(runtime.bytes_length, &[context_value, operand, expected]);
                    let result = builder.inst_results(call)[0];
                    branch_after_runtime_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        function.failure,
                        &cleanup_chain,
                    );
                    Some(result)
                }
                OperationKind::BytesPad { operand, from, to } => {
                    let operand = value_of(values, *operand)?;
                    let from = builder.ins().iconst(types::I64, *from as i64);
                    let to = builder.ins().iconst(types::I64, *to as i64);
                    let call = builder
                        .ins()
                        .call(runtime.bytes_pad, &[context_value, operand, from, to]);
                    let result = builder.inst_results(call)[0];
                    branch_after_runtime_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        function.failure,
                        &cleanup_chain,
                    );
                    Some(result)
                }
                OperationKind::EnterScope { .. } | OperationKind::ExitScope { .. } => None,
                OperationKind::PrepareNativeCall {
                    call,
                    arguments,
                    error,
                    ..
                } => {
                    let plan = runtime.native_plans.index(runtime.function_index, *call)?;
                    let plan = builder.ins().iconst(types::I64, i64::try_from(plan)?);
                    let arguments = arguments
                        .iter()
                        .map(|value| value_of(values, *value))
                        .collect::<Result<Vec<_>>>()?;
                    let arguments = stack_array(builder, &arguments)?;
                    builder.ins().call(
                        runtime.native_helpers["prepare"],
                        &[context_value, plan, arguments],
                    );
                    branch_after_error_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        error,
                        function.failure,
                        &cleanup_chain,
                    )?;
                    None
                }
                OperationKind::SourceNativeCall {
                    call: Some(call),
                    signature,
                    ..
                } => {
                    let index = runtime.native_plans.index(runtime.function_index, *call)?;
                    ensure!(
                        runtime.native_plans.records[index].signature == *signature,
                        "physical native call changed its prepared signature"
                    );
                    let plan = builder.ins().iconst(types::I64, i64::try_from(index)?);
                    let call = builder
                        .ins()
                        .call(runtime.native_helpers["address"], &[context_value, plan]);
                    let address = builder.inst_results(call)[0];
                    let call = builder
                        .ins()
                        .call(runtime.native_helpers["words"], &[context_value, plan]);
                    let words = builder.inst_results(call)[0];
                    let call = builder
                        .ins()
                        .call(runtime.native_stubs[&signature.name], &[address, words]);
                    let result = builder.inst_results(call)[0];
                    builder.ins().call(
                        runtime.native_helpers["returned"],
                        &[context_value, plan, result],
                    );
                    None
                }
                OperationKind::CloseNativeCall { call }
                | OperationKind::SettleNativeAdmissions { call, .. }
                | OperationKind::ReleaseNativeCall { call } => {
                    let plan = runtime.native_plans.index(runtime.function_index, *call)?;
                    let plan = builder.ins().iconst(types::I64, i64::try_from(plan)?);
                    let helper = match operation.kind {
                        OperationKind::CloseNativeCall { .. } => "close",
                        OperationKind::SettleNativeAdmissions { .. } => "settle",
                        _ => "release",
                    };
                    builder
                        .ins()
                        .call(runtime.native_helpers[helper], &[context_value, plan]);
                    None
                }
                OperationKind::ReadNativeResult { call, error, .. } => {
                    let plan = runtime.native_plans.index(runtime.function_index, *call)?;
                    let plan = builder.ins().iconst(types::I64, i64::try_from(plan)?);
                    let call = builder
                        .ins()
                        .call(runtime.native_helpers["read"], &[context_value, plan]);
                    let result = builder.inst_results(call)[0];
                    branch_after_error_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        error,
                        function.failure,
                        &cleanup_chain,
                    )?;
                    Some(result)
                }
                OperationKind::CatchError { .. } => {
                    if let Some((route, owner)) = unwind.saved_routes.borrow().get(&block.id) {
                        let route = builder.use_var(*route);
                        let owner = builder.use_var(*owner);
                        builder.def_var(unwind.variable, route);
                        builder.def_var(unwind.owner, owner);
                    }
                    let call = builder.ins().call(runtime.catch_error, &[context_value]);
                    let value = builder.inst_results(call)[0];
                    branch_after_runtime_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        function.failure,
                        &cleanup_chain,
                    );
                    Some(value)
                }
                OperationKind::ReserveSupplant {
                    reservation,
                    callee,
                    arguments,
                    ..
                } => {
                    let async_runtime = runtime
                        .source_async
                        .as_ref()
                        .ok_or_else(|| anyhow!("supplant requires Main continuations"))?;
                    let plans = runtime
                        .native_plans
                        .supplant(runtime.function_index, *reservation)?;
                    let mut plan = builder.ins().iconst(types::I64, -1);
                    if let Some(callee) = callee {
                        let selected = value_of(values, *callee)?;
                        for index in plans {
                            let target = runtime
                                .reference_targets
                                .identity(&runtime.native_plans.records[*index].signature.name)?;
                            let matches = builder.ins().icmp_imm_s(IntCC::Equal, selected, target);
                            let candidate =
                                builder.ins().iconst(types::I64, i64::try_from(*index)?);
                            plan = builder.ins().select(matches, candidate, plan);
                        }
                    } else {
                        ensure!(plans.len() == 1, "direct supplant requires a unique plan");
                        plan = builder.ins().iconst(types::I64, i64::try_from(plans[0])?);
                    }
                    let arguments = arguments
                        .iter()
                        .map(|value| value_of(values, *value))
                        .collect::<Result<Vec<_>>>()?;
                    let arguments = stack_array(builder, &arguments)?;
                    let call = builder.ins().call(
                        async_runtime.supplant_reserve,
                        &[context_value, plan, arguments],
                    );
                    let handle = builder.inst_results(call)[0];
                    ensure!(
                        runtime
                            .supplant_values
                            .borrow_mut()
                            .insert(*reservation, handle)
                            .is_none(),
                        "duplicate supplant SSA reservation"
                    );
                    None
                }
                OperationKind::StartSupplant { reservation, .. } => {
                    let handle = runtime.supplant_values.borrow()[reservation];
                    builder.ins().call(
                        runtime.source_async.as_ref().unwrap().supplant_start,
                        &[context_value, handle],
                    );
                    None
                }
                OperationKind::SettleSupplant {
                    reservation, error, ..
                } => {
                    let handle = runtime.supplant_values.borrow()[reservation];
                    let call = builder.ins().call(
                        runtime.source_async.as_ref().unwrap().supplant_settle,
                        &[context_value, handle],
                    );
                    let result = builder.inst_results(call)[0];
                    branch_after_error_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        error,
                        function.failure,
                        &cleanup_chain,
                    )?;
                    Some(result)
                }
                OperationKind::FunctionReference { target } => {
                    ensure!(
                        crate::function_reference::core_length(target).is_some()
                            || functions.contains_key(target)
                            || runtime.native_stubs.contains_key(target),
                        "function reference has no managed/core/validated native target"
                    );
                    Some(
                        builder
                            .ins()
                            .iconst(types::I64, runtime.reference_targets.identity(target)?),
                    )
                }
                OperationKind::IndirectCall { .. } => Some(lower_reference_call(
                    builder,
                    operation,
                    values,
                    functions,
                    managed_signatures,
                    runtime,
                    context_value,
                    blocks,
                    function,
                )?),
                OperationKind::SourceNativeCall {
                    call: None,
                    signature,
                    arguments,
                    error,
                    cleanup_chain,
                    ..
                } if runtime.source_async.is_some() => {
                    ensure!(arguments.len() == 1, "closed source service argument count");
                    let rinzu_compiler::ast::NativeBinding::Runtime(runtime_function) =
                        &signature.binding
                    else {
                        bail!("async source native call has no closed service");
                    };
                    let service_id = match runtime_function {
                        rinzu_compiler::RuntimeFunction::ReadResourceSize => 1,
                        rinzu_compiler::RuntimeFunction::Sleep => 2,
                        rinzu_compiler::RuntimeFunction::NativeCompleted => 3,
                        rinzu_compiler::RuntimeFunction::NativeTransferred => 4,
                    };
                    let service = builder.ins().iconst(types::I64, service_id);
                    let argument = value_of(values, arguments[0])?;
                    let call = builder.ins().call(
                        runtime.source_async.as_ref().unwrap().service,
                        &[context_value, service, argument],
                    );
                    let value = builder.inst_results(call)[0];
                    branch_after_error_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        error,
                        function.failure,
                        cleanup_chain,
                    )?;
                    Some(value)
                }
                OperationKind::SourceNativeCall { .. } => {
                    bail!("source native execution requires R4 physical ABI backend")
                }
                OperationKind::NumericConvert {
                    operand,
                    source,
                    error,
                    cleanup_chain,
                    ..
                } => {
                    let source = builder.ins().iconst(
                        types::I64,
                        i64::from(crate::scalar_backend::kind(source.scalar().unwrap()) as u32),
                    );
                    let target = builder.ins().iconst(
                        types::I64,
                        i64::from(crate::scalar_backend::kind(
                            operation.result.as_ref().unwrap().ty.scalar().unwrap(),
                        ) as u32),
                    );
                    let operand = value_of(values, *operand)?;
                    let call = builder.ins().call(
                        runtime.scalar_convert,
                        &[context_value, source, target, operand],
                    );
                    let value = builder.inst_results(call)[0];
                    branch_after_error_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        error,
                        function.failure,
                        cleanup_chain,
                    )?;
                    Some(value)
                }
                OperationKind::NativeCall { target, .. } => {
                    bail!("native call lowering is unavailable for {target}")
                }
                OperationKind::Await {
                    future,
                    consume,
                    error,
                    cleanup_chain: operation_cleanup,
                    ..
                } if runtime.source_async.is_some() => {
                    let future = value_of(values, *future)?;
                    let consume = builder.ins().iconst(
                        types::I64,
                        i64::from(
                            *consume
                                || runtime
                                    .fresh_return_consuming_awaits
                                    .contains(&(block.id, operation_index)),
                        ),
                    );
                    let call = builder.ins().call(
                        runtime.source_async.as_ref().unwrap().await_value,
                        &[context_value, future, consume],
                    );
                    let value = builder.inst_results(call)[0];
                    branch_after_error_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        error,
                        function.failure,
                        operation_cleanup,
                    )?;
                    branch_after_runtime_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        function.failure,
                        operation_cleanup,
                    );
                    Some(value)
                }
                OperationKind::Await { future, .. } => Some(value_of(values, *future)?),
                OperationKind::ErrorGroupOutcome {
                    scope,
                    error,
                    cleanup_chain: operation_cleanup,
                    ..
                }
                | OperationKind::TaskScopeOutcome {
                    scope,
                    error,
                    cleanup_chain: operation_cleanup,
                    ..
                } => {
                    end_cleanup(builder, runtime, context_value, *scope);
                    branch_after_error_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        error,
                        function.failure,
                        operation_cleanup,
                    )?;
                    branch_after_runtime_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        function.failure,
                        operation_cleanup,
                    );
                    None
                }
                OperationKind::EnterTaskScope { scope, .. }
                | OperationKind::JoinTaskScope { scope, .. } => {
                    let runtime = runtime
                        .source_async
                        .as_ref()
                        .ok_or_else(|| anyhow!("task scopes require the async runtime"))?;
                    let identity = builder.ins().iconst(
                        types::I64,
                        ((u64::from(scope.start_byte) << 32) | u64::from(scope.end_byte)) as i64,
                    );
                    let phase = builder.ins().iconst(
                        types::I64,
                        match operation.kind {
                            OperationKind::EnterTaskScope { all: true, .. } => 3,
                            OperationKind::EnterTaskScope { .. } => 0,
                            OperationKind::JoinTaskScope { cleanup: false, .. } => 1,
                            _ => 2,
                        },
                    );
                    builder
                        .ins()
                        .call(runtime.task_scope, &[context_value, identity, phase]);
                    None
                }
                OperationKind::AsyncStart
                | OperationKind::EnterTry { .. }
                | OperationKind::LeaveTry { .. } => None,
            };
            if let (Some(result), Some(value)) = (&operation.result, result) {
                values.insert(result.id, value);
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_terminator(
        builder: &mut FunctionBuilder<'_>,
        block: &BasicBlock,
        function: &IrFunction,
        runtime: &RuntimeFunctionRefs,
        context_value: Value,
        blocks: &HashMap<BlockId, cranelift_codegen::ir::Block>,
        values: &HashMap<ValueId, Value>,
        cleanup_exit: Option<BlockId>,
    ) -> Result<()> {
        match &block.terminator {
            Terminator::Goto { target, .. } if cleanup_exit.is_some() => {
                let unwind = runtime.unwind.as_ref().unwrap();
                let scope = function
                    .blocks
                    .iter()
                    .find(|block| block.id == cleanup_exit.unwrap())
                    .unwrap()
                    .range;
                end_cleanup(builder, runtime, context_value, scope);
                let owner = builder.use_var(unwind.owner);
                let pending = builder.ins().icmp_imm_s(
                    IntCC::Equal,
                    owner,
                    i64::from(cleanup_exit.unwrap().0) + 1,
                );
                let pending_here =
                    builder
                        .ins()
                        .icmp_imm_s(IntCC::Equal, owner, i64::from(block.id.0) + 1);
                let pending = builder.ins().bor(pending, pending_here);
                let normal = builder.create_block();
                builder
                    .ins()
                    .brif(pending, unwind.dispatch, &[], normal, &[]);
                builder.switch_to_block(normal);
                builder.seal_block(normal);
                let failed = failure_status(builder, runtime, context_value);
                builder
                    .ins()
                    .brif(failed, blocks[&function.failure], &[], blocks[target], &[]);
            }
            Terminator::Goto {
                target,
                cleanup_chain,
                ..
            } => {
                if cleanup_chain.is_empty() {
                    builder.ins().jump(blocks[target], &[]);
                } else {
                    jump_unwind(builder, runtime, blocks, cleanup_chain, *target);
                }
            }
            Terminator::Branch {
                condition,
                then_target,
                else_target,
            } => {
                builder.ins().brif(
                    value_of(values, *condition)?,
                    blocks[then_target],
                    &[],
                    blocks[else_target],
                    &[],
                );
            }
            Terminator::Return {
                value,
                cleanup_chain,
                ..
            } => {
                let value = value
                    .map(|value| value_of(values, value))
                    .transpose()?
                    .unwrap_or_else(|| builder.ins().iconst(types::I64, 0));

                if cleanup_chain.is_empty() {
                    builder.ins().return_(&[value]);
                } else {
                    let unwind = runtime.unwind.as_ref().unwrap();
                    builder.def_var(unwind.return_value, value);
                    if runtime
                        .data_schema
                        .is_reference(runtime.data_schema.index(&function.result_type)?)
                        || matches!(function.result_type, Type::Future(_))
                    {
                        builder
                            .ins()
                            .call(runtime.retain_return, &[context_value, value]);
                    }
                    jump_unwind(
                        builder,
                        runtime,
                        blocks,
                        cleanup_chain,
                        unwind.return_target,
                    );
                }
            }
            Terminator::Fail | Terminator::Unreachable => {
                let zero = builder.ins().iconst(types::I64, 0);
                builder.ins().return_(&[zero]);
            }
            Terminator::Switch {
                value,
                identity,
                cases,
            } => {
                let bits = value_of(values, *value)?;
                let variant = if runtime.virtual_bytes_options.contains(value) {
                    let present = builder.ins().icmp_imm_s(IntCC::NotEqual, bits, 0);
                    builder.ins().uextend(types::I64, present)
                } else {
                    let ty = runtime.data_schema.index(&Type::Named(identity.clone()))?;
                    let ty = builder.ins().iconst(types::I64, i64::try_from(ty)?);
                    let call = builder
                        .ins()
                        .call(runtime.enum_variant, &[context_value, bits, ty]);
                    builder.inst_results(call)[0]
                };
                let mut switch = Switch::new();
                for (tag, target) in cases {
                    switch.set_entry(u128::from(*tag), blocks[target]);
                }
                switch.emit(builder, variant, blocks[&function.failure]);
            }
            Terminator::Pending => bail!("unverified pending terminator"),
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_reference_call(
        builder: &mut FunctionBuilder<'_>,
        operation: &rinzu_compiler::Operation,
        values: &HashMap<ValueId, Value>,
        functions: &HashMap<String, FunctionInfo>,
        managed_signatures: &HashMap<String, cranelift_codegen::ir::SigRef>,
        runtime: &RuntimeFunctionRefs,
        context_value: Value,
        blocks: &HashMap<BlockId, cranelift_codegen::ir::Block>,
        function: &IrFunction,
    ) -> Result<Value> {
        let OperationKind::IndirectCall {
            callee,
            targets,
            arguments,
            error,
            cleanup_chain: operation_cleanup,
            ..
        } = &operation.kind
        else {
            unreachable!()
        };
        let callee = value_of(values, *callee)?;
        let arguments = arguments
            .iter()
            .map(|argument| value_of(values, *argument))
            .collect::<Result<Vec<_>>>()?;
        let result_block = builder.create_block();
        builder.append_block_param(result_block, types::I64);
        let invalid = builder.create_block();
        let target_blocks = targets
            .iter()
            .map(|_| builder.create_block())
            .collect::<Vec<_>>();
        let mut switch = Switch::new();
        for (target, block) in targets.iter().zip(&target_blocks) {
            switch.set_entry(runtime.reference_targets.identity(target)? as u128, *block);
        }
        switch.emit(builder, callee, invalid);
        builder.switch_to_block(invalid);
        builder
            .ins()
            .trap(cranelift_codegen::ir::TrapCode::unwrap_user(1));
        for (target, target_block) in targets.iter().zip(target_blocks) {
            builder.switch_to_block(target_block);
            let value = if let Some((kind, length)) = crate::function_reference::core_length(target)
            {
                ensure!(
                    arguments.len() == 1,
                    "length reference argument count mismatch"
                );
                let mut args = vec![context_value, arguments[0]];
                if let Some(length) = length {
                    args.push(builder.ins().iconst(types::I64, length as i64));
                }
                let entry = if kind == rinzu_compiler::CoreLengthKind::Utf8 {
                    runtime.utf8_length
                } else {
                    runtime.bytes_length
                };
                let call = builder.ins().call(entry, &args);
                builder.inst_results(call)[0]
            } else {
                let (name, info) = find_function(functions, target)?;
                ensure!(
                    info.parameter_count == arguments.len(),
                    "indirect call argument count mismatch"
                );
                let index = builder.ins().iconst(types::I64, i64::try_from(info.index)?);
                if info.is_async
                    && let Some(source_async) = &runtime.source_async
                {
                    let address = stack_array(builder, &arguments)?;
                    let count = builder
                        .ins()
                        .iconst(types::I64, i64::try_from(arguments.len())?);
                    let Type::Future(inner) = &operation.result.as_ref().unwrap().ty else {
                        unreachable!()
                    };
                    let kind = builder
                        .ins()
                        .iconst(types::I64, async_transfer_kind(inner)?);
                    let call = builder.ins().call(
                        source_async.call,
                        &[context_value, index, address, count, kind],
                    );
                    builder.inst_results(call)[0]
                } else {
                    ensure!(
                        !info.is_async,
                        "async references require source continuation backend"
                    );
                    let resolve = builder
                        .ins()
                        .call(runtime.resolve_dispatch, &[context_value, index]);
                    let address = builder.inst_results(resolve)[0];
                    let mut args = vec![context_value];
                    args.extend_from_slice(&arguments);
                    let call =
                        builder
                            .ins()
                            .call_indirect(managed_signatures[name], address, &args);
                    let value = builder.inst_results(call)[0];
                    builder.ins().call(runtime.leave_dispatch, &[context_value]);
                    value
                }
            };
            branch_after_error_call(
                builder,
                runtime,
                context_value,
                blocks,
                error,
                function.failure,
                operation_cleanup,
            )?;
            branch_after_runtime_call(
                builder,
                runtime,
                context_value,
                blocks,
                function.failure,
                operation_cleanup,
            );
            builder.ins().jump(result_block, &[value.into()]);
        }
        builder.switch_to_block(result_block);
        Ok(builder.block_params(result_block)[0])
    }

    fn lower_constant(
        builder: &mut FunctionBuilder<'_>,
        value: &ConstantValue,
        ty: &rinzu_compiler::Type,
        id: ValueId,
        runtime: &RuntimeFunctionRefs,
        context_value: Value,
    ) -> Result<Value> {
        match value {
            ConstantValue::Unit => Ok(builder.ins().iconst(types::I64, 0)),
            ConstantValue::Bool(value) => Ok(builder.ins().iconst(types::I64, i64::from(*value))),
            ConstantValue::Int(value) => Ok(builder.ins().iconst(types::I64, *value)),
            ConstantValue::Float(value) => {
                let bits = if ty.scalar() == Some(rinzu_compiler::AbiScalar::F32) {
                    u64::from((*value as f32).to_bits())
                } else {
                    value.to_bits()
                };
                Ok(builder.ins().iconst(types::I64, bits as i64))
            }
            ConstantValue::String(text) => {
                let address = match runtime.string_literals[&id] {
                    None => builder.ins().iconst(types::I64, 0),
                    Some(global) => builder.ins().symbol_value(types::I64, global),
                };
                let length = builder.ins().iconst(types::I64, i64::try_from(text.len())?);
                let call = builder
                    .ins()
                    .call(runtime.string, &[context_value, address, length]);
                Ok(builder.inst_results(call)[0])
            }
            ConstantValue::Bytes(bytes) => {
                let address = match runtime.string_literals[&id] {
                    None => builder.ins().iconst(types::I64, 0),
                    Some(global) => builder.ins().symbol_value(types::I64, global),
                };
                let length = builder
                    .ins()
                    .iconst(types::I64, i64::try_from(bytes.len())?);
                let call = builder
                    .ins()
                    .call(runtime.bytes, &[context_value, address, length]);
                Ok(builder.inst_results(call)[0])
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_binary(
        builder: &mut FunctionBuilder<'_>,
        operator: BinaryOperator,
        left: Value,
        right: Value,
        operand_type: &rinzu_compiler::Type,
        runtime: &RuntimeFunctionRefs,
        context_value: Value,
        blocks: &HashMap<BlockId, cranelift_codegen::ir::Block>,
        failure: BlockId,
        cleanup_chain: &[BlockId],
    ) -> Value {
        if *operand_type == rinzu_compiler::Type::String {
            let code = match operator {
                BinaryOperator::Equal => 0,
                BinaryOperator::NotEqual => 1,
                _ => unreachable!("verified String equality operation"),
            };
            let code = builder.ins().iconst(types::I64, code);
            let call = builder
                .ins()
                .call(runtime.string_compare, &[context_value, code, left, right]);
            let value = builder.inst_results(call)[0];
            branch_after_runtime_call(
                builder,
                runtime,
                context_value,
                blocks,
                failure,
                cleanup_chain,
            );
            return value;
        }
        if let Some(scalar) = operand_type
            .scalar()
            .filter(|_| !matches!(operator, BinaryOperator::Or | BinaryOperator::And))
        {
            let (code, compare) = match operator {
                BinaryOperator::Equal => (0, true),
                BinaryOperator::NotEqual => (1, true),
                BinaryOperator::Less => (2, true),
                BinaryOperator::LessEqual => (3, true),
                BinaryOperator::Greater => (4, true),
                BinaryOperator::GreaterEqual => (5, true),
                BinaryOperator::Add => (OP_ADD, false),
                BinaryOperator::Subtract => (OP_SUBTRACT, false),
                BinaryOperator::Multiply => (OP_MULTIPLY, false),
                BinaryOperator::Divide => (OP_DIVIDE, false),
                BinaryOperator::Remainder => (OP_REMAINDER, false),
                _ => unreachable!(),
            };
            if compare
                && (operand_type.is_integer()
                    || (scalar == rinzu_compiler::AbiScalar::Bool && code < 2))
            {
                // Typed IR values use canonical sign/zero-extended bits. Native
                // ingress, conversions and field reads retain their validation.
                let signed = matches!(
                    scalar,
                    rinzu_compiler::AbiScalar::I8
                        | rinzu_compiler::AbiScalar::I16
                        | rinzu_compiler::AbiScalar::I32
                        | rinzu_compiler::AbiScalar::I64
                );
                let condition = match operator {
                    BinaryOperator::Equal => IntCC::Equal,
                    BinaryOperator::NotEqual => IntCC::NotEqual,
                    BinaryOperator::Less if signed => IntCC::SignedLessThan,
                    BinaryOperator::LessEqual if signed => IntCC::SignedLessThanOrEqual,
                    BinaryOperator::Greater if signed => IntCC::SignedGreaterThan,
                    BinaryOperator::GreaterEqual if signed => IntCC::SignedGreaterThanOrEqual,
                    BinaryOperator::Less => IntCC::UnsignedLessThan,
                    BinaryOperator::LessEqual => IntCC::UnsignedLessThanOrEqual,
                    BinaryOperator::Greater => IntCC::UnsignedGreaterThan,
                    BinaryOperator::GreaterEqual => IntCC::UnsignedGreaterThanOrEqual,
                    _ => unreachable!("verified integer/Bool comparison"),
                };
                let compared = builder.ins().icmp(condition, left, right);
                let value = builder.ins().uextend(types::I64, compared);
                // A comparison adds no failure, but preserve dispatch of any
                // already pending failure and its native/storage cleanup.
                branch_after_runtime_call(
                    builder,
                    runtime,
                    context_value,
                    blocks,
                    failure,
                    cleanup_chain,
                );
                return value;
            }
            let kind = builder.ins().iconst(
                types::I64,
                i64::from(crate::scalar_backend::kind(scalar) as u32),
            );
            let code = builder.ins().iconst(types::I64, code);
            let helper = if compare {
                runtime.scalar_compare
            } else {
                runtime.scalar_arithmetic
            };
            let call = builder
                .ins()
                .call(helper, &[context_value, kind, code, left, right]);
            let value = builder.inst_results(call)[0];
            branch_after_runtime_call(
                builder,
                runtime,
                context_value,
                blocks,
                failure,
                cleanup_chain,
            );
            return value;
        }
        if matches!(operator, BinaryOperator::Equal | BinaryOperator::NotEqual) {
            let ty = runtime
                .data_schema
                .index(operand_type)
                .expect("closed verified operand type");
            let ty = builder
                .ins()
                .iconst(types::I64, i64::try_from(ty).expect("bounded catalog"));
            let code = builder
                .ins()
                .iconst(types::I64, i64::from(operator == BinaryOperator::NotEqual));
            let call = builder.ins().call(
                runtime.value_compare,
                &[context_value, ty, code, left, right],
            );
            let value = builder.inst_results(call)[0];
            branch_after_runtime_call(
                builder,
                runtime,
                context_value,
                blocks,
                failure,
                cleanup_chain,
            );
            return value;
        }
        let comparison = |builder: &mut FunctionBuilder<'_>, condition| {
            let compared = builder.ins().icmp(condition, left, right);
            builder.ins().uextend(types::I64, compared)
        };
        match operator {
            BinaryOperator::Or => builder.ins().bor(left, right),
            BinaryOperator::And => builder.ins().band(left, right),
            BinaryOperator::Equal => comparison(builder, IntCC::Equal),
            BinaryOperator::NotEqual => comparison(builder, IntCC::NotEqual),
            BinaryOperator::Less => comparison(builder, IntCC::SignedLessThan),
            BinaryOperator::LessEqual => comparison(builder, IntCC::SignedLessThanOrEqual),
            BinaryOperator::Greater => comparison(builder, IntCC::SignedGreaterThan),
            BinaryOperator::GreaterEqual => comparison(builder, IntCC::SignedGreaterThanOrEqual),
            BinaryOperator::Add
            | BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
            | BinaryOperator::Remainder => {
                let operation = match operator {
                    BinaryOperator::Add => OP_ADD,
                    BinaryOperator::Subtract => OP_SUBTRACT,
                    BinaryOperator::Multiply => OP_MULTIPLY,
                    BinaryOperator::Divide => OP_DIVIDE,
                    BinaryOperator::Remainder => OP_REMAINDER,
                    _ => unreachable!(),
                };
                let operation = builder.ins().iconst(types::I64, operation);
                let call = builder
                    .ins()
                    .call(runtime.arithmetic, &[context_value, operation, left, right]);
                let value = builder.inst_results(call)[0];
                branch_after_runtime_call(
                    builder,
                    runtime,
                    context_value,
                    blocks,
                    failure,
                    cleanup_chain,
                );
                value
            }
        }
    }

    fn end_cleanup(
        builder: &mut FunctionBuilder<'_>,
        runtime: &RuntimeFunctionRefs,
        context: Value,
        scope: rinzu_compiler::SourceRange,
    ) {
        let region = builder.ins().iconst(
            types::I64,
            runtime.unwind.as_ref().unwrap().cleanup_ids[&scope],
        );
        builder.ins().call(runtime.end_cleanup, &[context, region]);
    }

    // The C Context is native, nonmoving storage. Its private field offset and
    // width are asserted by the generated C runtime. Keep every load at the
    // original helper point; Rust reference contexts retain their typed entry.
    fn failure_status(
        builder: &mut FunctionBuilder<'_>,
        runtime: &RuntimeFunctionRefs,
        context: Value,
    ) -> Value {
        if runtime.direct_source {
            builder.ins().load(
                types::I64,
                cranelift_codegen::ir::MemFlagsData::new(),
                context,
                crate::C_CONTEXT_PENDING_PRIMARY_OFFSET,
            )
        } else {
            let call = builder.ins().call(runtime.failure, &[context]);
            builder.inst_results(call)[0]
        }
    }

    fn branch_if_recoverable(
        builder: &mut FunctionBuilder<'_>,
        runtime: &RuntimeFunctionRefs,
        context: Value,
        recoverable: cranelift_codegen::ir::Block,
        terminal: cranelift_codegen::ir::Block,
    ) {
        if runtime.direct_source {
            // Only primary == 0 can bypass classification. A nonzero primary
            // may have secondary diagnostics, so preserve the existing helper.
            let primary = failure_status(builder, runtime, context);
            let inspect = builder.create_block();
            builder.set_cold_block(inspect);
            builder.ins().brif(primary, inspect, &[], terminal, &[]);
            builder.switch_to_block(inspect);
            builder.seal_block(inspect);
        }
        let call = builder.ins().call(runtime.has_error, &[context]);
        let pending = builder.inst_results(call)[0];
        builder.ins().brif(pending, recoverable, &[], terminal, &[]);
    }

    #[allow(clippy::too_many_arguments)]
    fn branch_after_error_call(
        builder: &mut FunctionBuilder<'_>,
        runtime: &RuntimeFunctionRefs,
        context: Value,
        blocks: &HashMap<BlockId, cranelift_codegen::ir::Block>,
        error: &rinzu_compiler::ErrorEdge,
        failure: BlockId,
        cleanup: &[BlockId],
    ) -> Result<()> {
        let continuation = builder.create_block();
        let failed = builder.create_block();
        branch_if_recoverable(builder, runtime, context, failed, continuation);
        builder.switch_to_block(failed);
        builder.seal_block(failed);
        for call in &error.release_native_calls {
            let index = runtime.native_plans.index(runtime.function_index, *call)?;
            let plan = builder.ins().iconst(types::I64, i64::try_from(index)?);
            builder
                .ins()
                .call(runtime.native_helpers["release"], &[context, plan]);
        }
        let unwind = runtime.unwind.as_ref().unwrap();
        if let Some((saved_route, saved_owner)) = unwind.saved_routes.borrow().get(&error.target) {
            let route = builder.use_var(unwind.variable);
            let owner = builder.use_var(unwind.owner);
            builder.def_var(*saved_route, route);
            builder.def_var(*saved_owner, owner);
        }
        let released: Vec<_> = error
            .release_scopes
            .iter()
            .filter(|scope| unwind.cleanup_scopes.contains(scope))
            .collect();
        for scope in &released {
            end_cleanup(builder, runtime, context, **scope);
        }
        if released.is_empty() {
            jump_unwind(builder, runtime, blocks, &error.cleanup_chain, error.target);
        } else {
            let catch = builder.create_block();
            let terminal = builder.create_block();
            branch_if_recoverable(builder, runtime, context, catch, terminal);
            builder.switch_to_block(catch);
            builder.seal_block(catch);

            jump_unwind(builder, runtime, blocks, &error.cleanup_chain, error.target);
            builder.switch_to_block(terminal);
            builder.seal_block(terminal);
            for scope in unwind
                .active_cleanup
                .borrow()
                .iter()
                .rev()
                .filter(|scope| !error.release_scopes.contains(scope))
            {
                end_cleanup(builder, runtime, context, *scope);
            }

            jump_unwind(builder, runtime, blocks, cleanup, failure);
        }
        builder.switch_to_block(continuation);
        builder.seal_block(continuation);
        branch_after_runtime_call(builder, runtime, context, blocks, failure, cleanup);
        Ok(())
    }

    fn branch_after_runtime_call(
        builder: &mut FunctionBuilder<'_>,
        runtime: &RuntimeFunctionRefs,
        context_value: Value,
        blocks: &HashMap<BlockId, cranelift_codegen::ir::Block>,
        failure: BlockId,
        cleanup_chain: &[BlockId],
    ) {
        let failed = failure_status(builder, runtime, context_value);
        branch_after_runtime_status(
            builder,
            runtime,
            context_value,
            failed,
            blocks,
            failure,
            cleanup_chain,
        );
    }

    fn branch_after_runtime_status(
        builder: &mut FunctionBuilder<'_>,
        runtime: &RuntimeFunctionRefs,
        context_value: Value,
        failed: Value,
        blocks: &HashMap<BlockId, cranelift_codegen::ir::Block>,
        failure: BlockId,
        cleanup_chain: &[BlockId],
    ) {
        let continuation = builder.create_block();
        let failure_block = builder.create_block();
        builder
            .ins()
            .brif(failed, failure_block, &[], continuation, &[]);
        builder.switch_to_block(failure_block);
        builder.seal_block(failure_block);

        for scope in runtime
            .unwind
            .as_ref()
            .unwrap()
            .active_cleanup
            .borrow()
            .iter()
            .rev()
        {
            end_cleanup(builder, runtime, context_value, *scope);
        }
        jump_unwind(builder, runtime, blocks, cleanup_chain, failure);
        builder.switch_to_block(continuation);
        builder.seal_block(continuation);
    }

    fn jump_unwind(
        builder: &mut FunctionBuilder<'_>,
        runtime: &RuntimeFunctionRefs,
        blocks: &HashMap<BlockId, cranelift_codegen::ir::Block>,
        cleanup: &[BlockId],
        target: BlockId,
    ) {
        let unwind = runtime.unwind.as_ref().unwrap();
        let mut path = cleanup.to_vec();
        path.push(target);
        let next = builder.ins().iconst(types::I64, unwind.id(&path[1..]));
        builder.def_var(unwind.variable, next);
        let owner = builder.ins().iconst(types::I64, i64::from(path[0].0) + 1);
        builder.def_var(unwind.owner, owner);
        builder.ins().jump(blocks[&path[0]], &[]);
    }

    fn value_of(values: &HashMap<ValueId, Value>, value: ValueId) -> Result<Value> {
        values
            .get(&value)
            .copied()
            .ok_or_else(|| anyhow!("missing lowered value {value:?}"))
    }

    fn async_transfer_kind(ty: &Type) -> Result<i64> {
        if *ty == Type::Unit {
            return Ok(11);
        }
        if matches!(
            ty,
            Type::String
                | Type::Bytes
                | Type::FixedBytes(_)
                | Type::Named(_)
                | Type::Native {
                    call_only: false,
                    ..
                }
                | Type::List(_)
                | Type::Map(_, _)
                | Type::Error
        ) {
            return Ok(12);
        }
        if matches!(ty, Type::Future(_)) {
            return Ok(13);
        }
        ty.scalar()
            .map(|scalar| i64::from(crate::scalar_backend::kind(scalar) as u32))
            .ok_or_else(|| {
                anyhow!("AOT async result {ty:?} requires the R3 data transport backend")
            })
    }

    fn stack_array(builder: &mut FunctionBuilder<'_>, values: &[Value]) -> Result<Value> {
        if values.is_empty() {
            return Ok(builder.ins().iconst(types::I64, 0));
        }
        let size = values
            .len()
            .checked_mul(8)
            .and_then(|size| u32::try_from(size).ok())
            .ok_or_else(|| anyhow!("managed argument stack array is too large"))?;
        let slot = builder.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
            cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
            size,
            3,
        ));
        for (index, value) in values.iter().enumerate() {
            builder
                .ins()
                .stack_store(types::I64, *value, slot, i32::try_from(index * 8)?);
        }
        Ok(builder.ins().stack_addr(types::I64, slot, 0))
    }

    fn find_function<'a>(
        functions: &'a HashMap<String, FunctionInfo>,
        target: &str,
    ) -> Result<(&'a String, &'a FunctionInfo)> {
        functions
            .get_key_value(target)
            .or_else(|| {
                let short = target.rsplit("::").next()?;
                functions.get_key_value(short)
            })
            .ok_or_else(|| anyhow!("unknown managed function {target}"))
    }
}

pub fn run_baseline_jit(module: &IrModule, entry: &str) -> Result<BaselineRun> {
    run_baseline_jit_with_options(module, entry, BaselineOptions::default())
}

pub fn run_baseline_jit_with_options(
    module: &IrModule,
    entry: &str,
    options: BaselineOptions,
) -> Result<BaselineRun> {
    crate::ensure_scalar_backend(module, true)?;
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = (module, entry, options);
        Err(anyhow!(
            "Rinzu v0.1 baseline JIT requires aarch64-apple-darwin"
        ))
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        target::run(module, entry, options, None, None)
    }
}

pub fn run_baseline_jit_with_native_packages(
    module: &IrModule,
    entry: &str,
    options: BaselineOptions,
    native_packages: LoadedNativePackages,
) -> Result<BaselineRun> {
    crate::ensure_scalar_backend(module, true)?;
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = (module, entry, options, native_packages);
        Err(anyhow!(
            "Rinzu v0.2 native package JIT requires aarch64-apple-darwin"
        ))
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        target::run(module, entry, options, Some(native_packages), None)
    }
}

pub fn run_baseline_jit_with_package_bindings(
    module: &IrModule,
    entry: &str,
    options: BaselineOptions,
    native_packages: Option<LoadedNativePackages>,
    source_packages: Option<crate::source_package::LoadedSourcePackages>,
) -> Result<BaselineRun> {
    crate::ensure_scalar_backend(module, true)?;
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = (module, entry, options, native_packages, source_packages);
        anyhow::bail!("source native package JIT requires macOS arm64")
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        target::run(module, entry, options, native_packages, source_packages)
    }
}

pub fn emit_baseline_aot_object(module: &IrModule, entry: &str) -> Result<BaselineAotObject> {
    emit_baseline_aot_object_with_native_bindings(module, entry, &HashMap::new())
}

/// Diagnostic lowering inventory; emits no executable code or AOT object.
#[cfg(feature = "test-c-lowering-probe")]
pub fn inspect_source_async_clif(module: &IrModule) -> Result<Vec<(String, String)>> {
    crate::ensure_source_async_backend(module)?;
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        target::inspect_clif(module)
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = module;
        bail!("C lowering inventory requires macOS arm64")
    }
}

pub fn emit_baseline_aot_object_with_native_bindings(
    module: &IrModule,
    entry: &str,
    native_imports: &HashMap<String, u64>,
) -> Result<BaselineAotObject> {
    crate::ensure_scalar_backend(module, true)?;
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = (module, entry, native_imports);
        Err(anyhow!(
            "Rinzu v0.1 baseline AOT requires aarch64-apple-darwin"
        ))
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        target::emit_aot(module, entry, native_imports, false, false)
    }
}

/// Emit real source async calls with private Future records and typed Worker
/// transport. The linked runtime owns pending completion and Origin adoption.
pub fn emit_source_async_aot_object(
    module: &IrModule,
    entry: &str,
    native_imports: &HashMap<String, u64>,
) -> Result<BaselineAotObject> {
    crate::ensure_source_async_backend(module)?;
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = (module, entry, native_imports);
        Err(anyhow!(
            "Rinzu source async AOT requires aarch64-apple-darwin"
        ))
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        target::emit_aot(module, entry, native_imports, true, true)
    }
}

/// C-JIT keeps its existing managed code backend; only its runtime plans use
/// the AOT metadata format, so no C function definitions belong in that shim.
pub fn emit_source_async_aot_plans(
    module: &IrModule,
    entry: &str,
    native_imports: &HashMap<String, u64>,
) -> Result<BaselineAotObject> {
    crate::ensure_source_async_backend(module)?;
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        target::emit_aot(module, entry, native_imports, true, false)
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = (module, entry, native_imports);
        bail!("source async plans require macOS arm64")
    }
}
