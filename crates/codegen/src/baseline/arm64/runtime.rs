//! Managed execution state and runtime entry points for the baseline JIT.
use super::*;
use core::ffi::c_void;
use rinzu_abi::{ABI_VERSION, NativeInvocationContextV1};
use rinzu_runtime::code::{CodeFrame, CodeVersion, CodeVersionId, CodeVersionState};
use rinzu_runtime::data::{DataError, ManagedData, ManagedHandle, ManagedRoots, ManagedSlot};
use rinzu_runtime::dispatch::VersionedDispatchCell;
use rinzu_runtime::failure::{Diagnostic, FailureCause, PendingFailure};
use rinzu_runtime::heap::{CollectionKind, DomainHeap, HeapError};
use std::sync::Arc;

mod errors;
mod native;
use native::NativeLedger;

pub(in crate::baseline) fn run(
    module_ir: &IrModule,
    entry: &str,
    options: BaselineOptions,
    native_packages: Option<LoadedNativePackages>,
    source_packages: Option<crate::source_package::LoadedSourcePackages>,
    arguments: Vec<Vec<u8>>,
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
    let isa = crate::native_isa(false)?;
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
        arguments,
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
        poll_count: runtime.poll.count,
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

struct PollControl {
    count: usize,
    cancel_after: Option<usize>,
    shutdown_after: Option<usize>,
}

impl PollControl {
    fn poll(&mut self, in_cleanup: bool, in_callback: bool) -> Option<FailureCause> {
        self.count += 1;
        if in_cleanup || in_callback {
            return None;
        }
        if self.shutdown_after.is_some_and(|limit| self.count >= limit) {
            Some(FailureCause::Shutdown)
        } else if self.cancel_after.is_some_and(|limit| self.count >= limit) {
            Some(FailureCause::Cancelled)
        } else {
            None
        }
    }
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
    arguments: rinzu_runtime::data::ProgramArguments,
    data_roots: ManagedRoots,
    root_publications: usize,
    peak_root_frames: usize,
    return_roots: BTreeMap<usize, rinzu_runtime::data::RootLease>,
    function_names: Vec<String>,
    function_trace: Vec<String>,
    poll: PollControl,
    allocation_count: usize,
    collection_count: usize,
    callback_depth: usize,
    max_allocations: Option<usize>,
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
    source_native: NativeLedger,
    userdata: rinzu_runtime::data::NativeUserDataRegistry,
}

impl BaselineRuntime {
    fn new(
        function_names: Vec<String>,
        options: BaselineOptions,
        native_packages: Option<LoadedNativePackages>,
        data_schema: crate::data_schema::DataSchema,
        arguments: Vec<Vec<u8>>,
    ) -> Self {
        let mut heap = DomainHeap::new(0x4a49_5401);
        if options.collect_every_allocation {
            heap.enable_gc_stress();
        }
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
            arguments: rinzu_runtime::data::ProgramArguments::new(arguments),
            data_roots: ManagedRoots::default(),
            root_publications: 0,
            peak_root_frames: 0,
            return_roots: BTreeMap::new(),
            function_names,
            function_trace: Vec::new(),
            poll: PollControl {
                count: 0,
                cancel_after: options.cancel_after_polls,
                shutdown_after: options.shutdown_after_polls,
            },
            allocation_count: 0,
            collection_count: 0,
            callback_depth: 0,
            max_allocations: options.max_allocations,
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
            source_native: NativeLedger::default(),
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
            Err(DataError::ManagedThrown(handle)) => {
                let thrown = errors::throw_error(self, handle);
                self.data_result(thrown);
                None
            }
            Err(DataError::Interrupted(cause)) => {
                self.fail(match cause {
                    FailureCause::Cancelled => BaselineFailure::Cancelled,
                    FailureCause::Shutdown => BaselineFailure::Shutdown,
                    _ => BaselineFailure::RuntimePanic,
                });
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
        let in_cleanup = runtime.pending.cleanup_depth() != 0;
        let control = runtime.poll.poll(in_cleanup, runtime.callback_depth != 0);
        if in_cleanup {
            return 0;
        }
        if let Some(cause) = control {
            runtime.data_result::<()>(Err(DataError::Interrupted(cause)));
        }
        i64::from(runtime.pending.current().is_some())
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
            .retain_roots(
                &mut runtime.heap,
                runtime.data_roots.live().chain(runtime.arguments.root()),
            )
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
            if count != op.arity() {
                return Err(DataError::InvalidEncoding);
            }
            let result = usize::try_from(result).map_err(|_| DataError::InvalidEncoding)?;
            if op == rinzu_compiler::DataOperation::ProcessArguments {
                return runtime
                    .arguments
                    .snapshot(&mut runtime.data, &mut runtime.heap, result);
            }
            if count != 0 && (bits.is_null() || types.is_null()) {
                return Err(DataError::InvalidEncoding);
            }
            let (raw_types, bits) = if count == 0 {
                (&[][..], &[][..])
            } else {
                // SAFETY: generated code supplies two aligned arrays with the verified arity.
                unsafe {
                    (
                        std::slice::from_raw_parts(types, count),
                        std::slice::from_raw_parts(bits, count),
                    )
                }
            };
            let mut types = [0; 3];
            let mut slots = [rinzu_runtime::data::ManagedSlot::Unit; 3];
            for index in 0..count {
                types[index] =
                    usize::try_from(raw_types[index]).map_err(|_| DataError::InvalidEncoding)?;
                slots[index] = runtime.data_schema.slot(types[index], bits[index])?;
            }
            #[cfg(unix)]
            if op.is_json() {
                let schema = runtime
                    .data_schema
                    .json
                    .as_ref()
                    .ok_or(DataError::InvalidEncoding)?;
                let cleanup = runtime.pending.cleanup_depth() != 0;
                let callback = runtime.callback_depth != 0;
                return runtime.data.json_operation(
                    &mut runtime.heap,
                    schema,
                    rinzu_runtime::data::JsonCall {
                        operation: op,
                        result,
                        types: &types[..count],
                        slots: &slots[..count],
                    },
                    || runtime.poll.poll(cleanup, callback),
                );
            }
            let output = runtime.data.operation(
                &mut runtime.heap,
                op,
                result,
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
        runtime.data_allocated(
            logical.unwrap_or_else(|| runtime.heap.object_count().saturating_sub(before)),
        );
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

unsafe extern "C" fn invoke_native_handler(context: *mut c_void, handler: u64) -> i64 {
    let Ok(handler) = usize::try_from(handler) else {
        with_runtime(context.cast(), (), |runtime| {
            runtime.fail(BaselineFailure::Unsupported);
        });
        return 0;
    };
    // SAFETY: the package call is synchronous and the validated JIT runtime,
    // dispatch table, and code mapping remain live until it returns.
    match unsafe { invoke_callback_trampoline(context.cast(), handler) } {
        Ok(value) => value,
        Err(_) => {
            with_runtime(context.cast(), (), |runtime| {
                runtime.fail(BaselineFailure::Unsupported);
            });
            0
        }
    }
}

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

unsafe fn invoke_callback_trampoline(
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
