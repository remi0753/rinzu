//! Shared Cranelift lowering used by the baseline JIT and AOT object emitter.

#[cfg(feature = "test-allocation-probe")]
pub mod allocation_probe;
mod baseline;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod compiled_runtime;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub use compiled_runtime::CompiledRuntimeLibrary;
mod data_schema;
mod error_sources;
#[cfg(unix)]
mod json_schema;
pub use error_sources::format_error_traces;
mod function_reference;
#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
mod m5_reload_spike;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod macos_jit_memory;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod native_binding;
mod native_call;
#[cfg(target_os = "macos")]
mod native_library;
mod native_package;
mod native_record;
pub use native_call::NativeCallPlan;
pub use native_record::source_native_record_layouts;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod c_lowering;
mod scalar_backend;
mod source_async;
mod source_failure;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod source_native_abi;
pub mod source_package;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod task_error_origin;
#[cfg(test)]
#[path = "../test-support/library.rs"]
mod test_library;

#[cfg(feature = "test-c-lowering-probe")]
pub use baseline::inspect_source_async_clif;
pub use baseline::{
    AotDataType, AotFunctionMetadata, BaselineAotObject, BaselineFailure, BaselineOptions,
    BaselineRun, emit_baseline_aot_object, emit_baseline_aot_object_with_native_bindings,
    emit_source_async_aot_object, emit_source_async_aot_plans, run_baseline_jit,
    run_baseline_jit_with_arguments, run_baseline_jit_with_native_packages,
    run_baseline_jit_with_options, run_baseline_jit_with_package_bindings,
};
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub use baseline::{CompiledAsyncJit, compile_source_async_jit};
pub use native_package::{LoadedNativePackages, NativePackageArtifact};
#[cfg(all(
    feature = "test-dispatcher-probe",
    target_os = "macos",
    target_arch = "aarch64"
))]
pub use source_async::configure_foreign_return_probe;
pub use source_async::{
    CallbackReturnEvidence, ForeignInvocationEvidence, FutureAdmission, NativeAsyncOperation,
    SourceAsyncEvent, SourceAsyncOptions, SourceAsyncRun, SourceHeapEvidence,
    SourceNativeCallEvidence, SourceWorkerHeapEvidence, run_source_async, run_source_async_report,
};
mod native_resources;
pub use native_resources::{CallbackPoolEvidence, NativeProcessEvidence};

/// Private generated object/runtime contract; public native ABI versions are separate.
pub const BASELINE_RUNTIME_ABI_REVISION: &str = "rinzu-baseline-0.5-r185";

/// Private macOS arm64 C Context contract; generated C asserts this field offset.
pub const C_CONTEXT_PENDING_PRIMARY_OFFSET: i32 = 208;
/// The same private C Context contract used by direct Source stores.
pub const C_CONTEXT_FAILURE_ORIGIN_OFFSET: i32 = 896;
/// Kept equal to the generated runtime's Future handle base by a C assertion.
pub const C_ASYNC_HANDLE_BASE: i64 = 1 << 61;

use anyhow::{Context, Result, anyhow, ensure};
use cranelift_codegen::ir::{AbiParam, InstBuilder, UserFuncName, types};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module, default_libcall_names};
use cranelift_object::{ObjectBuilder, ObjectModule};

pub const SPIKE_SYMBOL: &str = "rinzu_backend_spike";
/// The same identities used by generated source diagnostics and callback sites.
pub fn resource_source_files(
    module: &rinzu_compiler::IrModule,
) -> std::collections::BTreeMap<u64, String> {
    baseline::source_identities(module).1
}
pub const SAFEPOINT_SYMBOL: &str = "rinzu_backend_spike_safepoint";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompileEvidence {
    pub stack_map_count: usize,
    pub stack_map_entry_count: usize,
    pub code_size: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JitEvidence {
    pub result: i64,
    pub compile: CompileEvidence,
    pub memory_policy: &'static str,
    pub executable_mappings: usize,
    pub executable_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AotEvidence {
    pub object: Vec<u8>,
    pub compile: CompileEvidence,
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
extern "C" fn spike_safepoint() {}

fn native_isa(position_independent: bool) -> Result<cranelift_codegen::isa::OwnedTargetIsa> {
    let mut flags = settings::builder();
    flags
        .set(
            "is_pic",
            if position_independent {
                "true"
            } else {
                "false"
            },
        )
        .context("configure position-independent code")?;
    flags
        .set("use_colocated_libcalls", "false")
        .context("allow external runtime symbols")?;
    cranelift_native::builder()
        .map_err(|message| anyhow!(message))?
        .finish(settings::Flags::new(flags))
        .context("build native Cranelift ISA")
}

fn define_spike<M: Module>(module: &mut M) -> Result<(FuncId, CompileEvidence)> {
    let mut safepoint_signature = module.make_signature();
    safepoint_signature.call_conv = module.target_config().default_call_conv;
    let safepoint = module
        .declare_function(SAFEPOINT_SYMBOL, Linkage::Import, &safepoint_signature)
        .context("declare safepoint import")?;

    let mut signature = module.make_signature();
    signature.call_conv = module.target_config().default_call_conv;
    signature.params.push(AbiParam::new(types::I64));
    signature.params.push(AbiParam::new(types::I64));
    signature.returns.push(AbiParam::new(types::I64));
    let function = module
        .declare_function(SPIKE_SYMBOL, Linkage::Export, &signature)
        .context("declare spike function")?;

    let mut context = module.make_context();
    context.func.name = UserFuncName::user(0, function.as_u32());
    context.func.signature = signature;
    let mut builder_context = FunctionBuilderContext::new();

    {
        let target_config = module.target_config();
        let safepoint_ref = module.declare_func_in_func(safepoint, &mut context.func);
        let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        builder.seal_block(entry);

        let root = builder.block_params(entry)[0];
        let value = builder.block_params(entry)[1];
        builder.declare_value_needs_stack_map(root);
        builder.ins().call(safepoint_ref, &[]);

        // Keeping the root semantically live after the call forces the call
        // safepoint to describe it. The test root is aligned, so its low bit is 0.
        let root_low_bit = builder.ins().band_imm_u(root, 1);
        let incremented = builder.ins().iadd_imm_u(value, 1);
        let result = builder.ins().iadd(incremented, root_low_bit);
        builder.ins().return_(&[result]);
        builder.finalize(target_config);
    }

    module
        .define_function(function, &mut context)
        .context("compile spike function")?;
    let compiled = context
        .compiled_code()
        .ok_or_else(|| anyhow!("Cranelift did not retain compiled code evidence"))?;
    let stack_maps = compiled.buffer.user_stack_maps();
    let evidence = CompileEvidence {
        stack_map_count: stack_maps.len(),
        stack_map_entry_count: stack_maps
            .iter()
            .map(|(_, _, map)| map.entries().count())
            .sum(),
        code_size: compiled.code_buffer().len(),
    };
    ensure!(
        evidence.stack_map_count > 0,
        "no safepoint stack map emitted"
    );
    ensure!(
        evidence.stack_map_entry_count > 0,
        "safepoint stack map contains no managed root"
    );
    module.clear_context(&mut context);
    Ok((function, evidence))
}

pub fn run_jit_spike() -> Result<JitEvidence> {
    with_jit_spike_inspection(|_| Ok(())).map(|(evidence, ())| evidence)
}

/// Runs an inspection callback while the production JIT module and all of its
/// published code mappings are still live.
pub fn with_jit_spike_inspection<T>(
    inspect: impl FnOnce(&JitEvidence) -> Result<T>,
) -> Result<(JitEvidence, T)> {
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        return Err(anyhow!("Rinzu v1 JIT spike requires aarch64-apple-darwin"));
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        let isa = native_isa(false)?;
        let mut jit_builder = JITBuilder::with_isa(isa, default_libcall_names());
        jit_builder.symbol(SAFEPOINT_SYMBOL, spike_safepoint as *const u8);
        let (jit_memory, memory_audit) = macos_jit_memory::MacOsArm64JitMemory::new_with_audit()?;
        jit_builder.memory_provider(Box::new(jit_memory));
        let mut module = JITModule::new(jit_builder);
        let (function, compile) = define_spike(&mut module)?;
        module
            .finalize_definitions()
            .context("finalize JIT definitions")?;
        let address = module.get_finalized_function(function);

        // SAFETY: define_spike emits this exact platform C signature and the
        // JIT module remains live until after the call.
        let callable: unsafe extern "C" fn(i64, i64) -> i64 =
            unsafe { core::mem::transmute(address) };
        // SAFETY: the generated function does not dereference its aligned root.
        let result = unsafe { callable(0x1000, 41) };
        ensure!(result == 42, "JIT returned {result}, expected 42");

        let memory_audit = memory_audit.snapshot();
        let evidence = JitEvidence {
            result,
            compile,
            memory_policy: "MAP_JIT RW -> instruction-cache flush -> RX",
            executable_mappings: memory_audit.executable_mappings,
            executable_bytes: memory_audit.executable_bytes,
        };
        let inspected = inspect(&evidence)?;
        // SAFETY: callable is no longer used after freeing this module's memory.
        unsafe { module.free_memory() };
        Ok((evidence, inspected))
    }
}

pub fn emit_aot_object() -> Result<AotEvidence> {
    let isa = native_isa(true)?;
    let builder = ObjectBuilder::new(isa, "rinzu-i0-spike", default_libcall_names())
        .context("create object backend")?;
    let mut module = ObjectModule::new(builder);
    let (_, compile) = define_spike(&mut module)?;
    let object = module.finish().emit().context("emit native object bytes")?;
    ensure!(!object.is_empty(), "AOT object emitter returned no bytes");
    Ok(AotEvidence { object, compile })
}

// R2 introduces typed source values; R3 supplies their exact executable semantics.
fn ensure_scalar_backend(
    module: &rinzu_compiler::IrModule,
    scalar_values: bool,
) -> anyhow::Result<()> {
    ensure_backend(module, scalar_values, false)
}

fn ensure_source_async_backend(module: &rinzu_compiler::IrModule) -> anyhow::Result<()> {
    ensure_backend(
        module,
        true,
        cfg!(all(target_os = "macos", target_arch = "aarch64")),
    )
}

fn ensure_backend(
    module: &rinzu_compiler::IrModule,
    scalar_values: bool,
    supplant: bool,
) -> anyhow::Result<()> {
    fn pending(ty: &rinzu_compiler::Type, scalar_values: bool) -> bool {
        use rinzu_compiler::Type;
        match ty {
            Type::Error => !scalar_values,
            Type::FixedBytes(_) => !scalar_values,
            Type::FunctionReference(_) => !scalar_values,
            Type::List(element) | Type::Future(element) => pending(element, scalar_values),
            Type::Map(key, value) => pending(key, scalar_values) || pending(value, scalar_values),
            _ => false,
        }
    }
    for function in &module.functions {
        anyhow::ensure!(
            !pending(&function.result_type, scalar_values),
            "ABI scalar execution requires the R3 scalar backend"
        );
        for operation in function.blocks.iter().flat_map(|block| &block.operations) {
            anyhow::ensure!(
                !matches!(&operation.kind, rinzu_compiler::OperationKind::LoadLocal { name } if name.contains('.'))
                    && (scalar_values
                        || !matches!(
                            operation.kind,
                            rinzu_compiler::OperationKind::Field { .. }
                                | rinzu_compiler::OperationKind::ListElement { .. }
                                | rinzu_compiler::OperationKind::EnumPayload { .. }
                        ))
                    && (scalar_values
                        || !matches!(
                            operation.kind,
                            rinzu_compiler::OperationKind::ErrorMessage { .. }
                        )),
                "data field projection execution requires the R3 data backend"
            );
            if let rinzu_compiler::OperationKind::Field { identity, .. }
            | rinzu_compiler::OperationKind::Construct {
                target: identity, ..
            } = &operation.kind
            {
                anyhow::ensure!(
                    !module
                        .source_types
                        .get(identity)
                        .is_some_and(|schema| schema.kind == "opaque"
                            || (!scalar_values && schema.kind == "native-record")),
                    "native data execution requires the R3 native layout backend"
                );
            }
            anyhow::ensure!(
                scalar_values
                    || !matches!(
                        operation.kind,
                        rinzu_compiler::OperationKind::CatchError { .. }
                    ),
                "Error/catch execution requires R3 failure backend"
            );
            anyhow::ensure!(
                supplant
                    || !matches!(
                        operation.kind,
                        rinzu_compiler::OperationKind::ReserveSupplant { .. }
                            | rinzu_compiler::OperationKind::StartSupplant { .. }
                            | rinzu_compiler::OperationKind::SettleSupplant { .. }
                    ),
                "supplant execution requires R4 reservation backend"
            );
            anyhow::ensure!(
                !matches!(
                    operation.kind,
                    rinzu_compiler::OperationKind::SourceNativeCall { ref signature, .. }
                        if signature.is_supplant || (signature.is_async
                            && !(scalar_values && matches!(&signature.binding,
                                rinzu_compiler::ast::NativeBinding::Runtime(_))))
                ),
                "source native execution requires the R4 physical ABI backend"
            );
            anyhow::ensure!(
                (scalar_values
                    || !matches!(
                        operation.kind,
                        rinzu_compiler::OperationKind::NumericConvert { .. }
                    ))
                    && !operation
                        .result
                        .as_ref()
                        .is_some_and(|result| pending(&result.ty, scalar_values)),
                "ABI scalar/conversion execution requires the R3 scalar backend"
            );
        }
    }
    Ok(())
}
