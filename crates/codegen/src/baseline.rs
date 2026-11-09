//! Baseline JIT and AOT entry points and shared metadata.
use std::collections::{BTreeMap, HashMap};

use anyhow::{Result, anyhow, bail};
use rinzu_compiler::IrModule;
use rinzu_runtime::failure::{ErrorKind, Failure, SourcePosition};

use crate::{CompileEvidence, LoadedNativePackages};

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
    pub uses_json: bool,
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
    arm64::compile_async_jit(module, external_symbols, native_imports)
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

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod arm64;

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
        arm64::run(module, entry, options, None, None, Vec::new())
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
        arm64::run(
            module,
            entry,
            options,
            Some(native_packages),
            None,
            Vec::new(),
        )
    }
}

pub fn run_baseline_jit_with_package_bindings(
    module: &IrModule,
    entry: &str,
    options: BaselineOptions,
    native_packages: Option<LoadedNativePackages>,
    source_packages: Option<crate::source_package::LoadedSourcePackages>,
) -> Result<BaselineRun> {
    run_baseline_jit_with_arguments(
        module,
        entry,
        options,
        native_packages,
        source_packages,
        Vec::new(),
    )
}

pub fn run_baseline_jit_with_arguments(
    module: &IrModule,
    entry: &str,
    options: BaselineOptions,
    native_packages: Option<LoadedNativePackages>,
    source_packages: Option<crate::source_package::LoadedSourcePackages>,
    arguments: Vec<Vec<u8>>,
) -> Result<BaselineRun> {
    crate::ensure_scalar_backend(module, true)?;
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = (
            module,
            entry,
            options,
            native_packages,
            source_packages,
            arguments,
        );
        anyhow::bail!("source native package JIT requires macOS arm64")
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        arm64::run(
            module,
            entry,
            options,
            native_packages,
            source_packages,
            arguments,
        )
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
        arm64::inspect_clif(module)
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
        arm64::emit_aot(module, entry, native_imports, false, false)
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
        arm64::emit_aot(module, entry, native_imports, true, true)
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
        arm64::emit_aot(module, entry, native_imports, true, false)
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = (module, entry, native_imports);
        bail!("source async plans require macOS arm64")
    }
}
