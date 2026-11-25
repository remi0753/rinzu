//! Publish compiled async code in the host JIT.
mod code;

#[cfg(test)]
mod cache_probe;

use super::*;
use cranelift_codegen::isa::OwnedTargetIsa;
use std::sync::{Arc, OnceLock};

pub use code::{AsyncJitCode, AsyncJitImage};
pub use cranelift_codegen::incremental_cache::CacheKvStore as AsyncJitFunctionCache;

pub struct AsyncJitCompiler {
    isa: OwnedTargetIsa,
}

/// Managed code before relocations and executable memory publication.
pub struct PreparedAsyncJit {
    module: JITModule,
    audit: Arc<crate::jit_memory::JitMemoryAudit>,
    compile: CompileEvidence,
    externals: Arc<OnceLock<BTreeMap<String, usize>>>,
}

impl AsyncJitCompiler {
    pub fn new() -> Result<Self> {
        Ok(Self {
            isa: isa::jit_isa()?,
        })
    }

    pub fn cache_identity(&self) -> impl serde::Serialize + use<> {
        let flags = |values: Vec<cranelift_codegen::settings::Value>| {
            values
                .into_iter()
                .map(|value| (value.name, value.value_string()))
                .collect::<BTreeMap<_, _>>()
        };
        (
            cranelift_codegen::VERSION,
            self.isa.triple().to_string(),
            flags(self.isa.flags().iter().collect()),
            flags(self.isa.isa_flags()),
        )
    }

    fn module(&self) -> Result<PreparedAsyncJit> {
        let mut builder = JITBuilder::with_isa(self.isa.clone(), default_libcall_names());
        let externals = Arc::new(OnceLock::<BTreeMap<String, usize>>::new());
        let lookup = Arc::clone(&externals);
        builder.symbol_lookup_fn(Box::new(move |name| {
            lookup.get()?.get(name).map(|&address| address as *const u8)
        }));
        let (memory, audit) = JitMemory::new_with_audit()?;
        builder.memory_provider(Box::new(memory));
        Ok(PreparedAsyncJit {
            module: JITModule::new(builder),
            audit,
            compile: CompileEvidence {
                stack_map_count: 0,
                stack_map_entry_count: 0,
                code_size: 0,
            },
            externals,
        })
    }

    pub fn prepare(
        &self,
        ir: &IrModule,
        native_imports: &HashMap<String, u64>,
        record: bool,
    ) -> Result<(PreparedAsyncJit, Option<AsyncJitImage>)> {
        self.prepare_with_cache(ir, native_imports, record, None)
    }

    pub fn prepare_with_cache(
        &self,
        ir: &IrModule,
        native_imports: &HashMap<String, u64>,
        record: bool,
        cache: Option<&mut dyn AsyncJitFunctionCache>,
    ) -> Result<(PreparedAsyncJit, Option<AsyncJitImage>)> {
        crate::ensure_source_async_backend(ir)?;
        let target = Target::host().ok_or_else(|| anyhow!("unsupported async JIT host"))?;
        let mut prepared = self.module()?;
        let mut module = code::Capture::new(&mut prepared.module, record, cache);
        let mut analysis = rinzu_compiler::timing::span("jit.managed.analysis");
        let mut runtime =
            declare_runtime_functions(&mut module, ir, true, true, uses_direct_c_context(target))?;
        runtime.link_values = true;
        let stubs = runtime.native_stubs.len();
        let reused_stubs = module.reused();
        analysis.set_counter("native_stubs", stubs as u64);
        analysis.set_counter("generated", (stubs - reused_stubs) as u64);
        analysis.set_counter("reused", reused_stubs as u64);
        drop(analysis);
        let functions = declare_managed_functions(&mut module, ir, Linkage::Export)?;
        let mut lowering = rinzu_compiler::timing::span("jit.managed.lower");
        lowering.set_counter("functions", ir.functions.len() as u64);
        for (index, function) in ir.functions.iter().enumerate() {
            define_function(
                &mut module,
                function,
                index,
                &functions,
                &runtime,
                native_imports,
                false,
                &mut prepared.compile,
                None,
                None,
            )?;
        }
        let reused = module.reused() - reused_stubs;
        lowering.set_counter("generated", (ir.functions.len() - reused) as u64);
        lowering.set_counter("reused", reused as u64);
        drop(lowering);
        let image = module.finish(&prepared.compile);
        Ok((prepared, image))
    }

    pub fn restore(
        &self,
        aot: &crate::BaselineAotObject,
        runtime_symbols: &[String],
        image: AsyncJitCode,
        bytes: &[u8],
    ) -> Result<PreparedAsyncJit> {
        ensure!(
            Target::host() == Some(aot.target),
            "cached JIT target differs from the host"
        );
        let mut prepared = self.module()?;
        prepared.compile = image.replay(&mut prepared.module, aot, runtime_symbols, bytes)?;
        Ok(prepared)
    }
}

impl PreparedAsyncJit {
    pub fn publish(
        mut self,
        external_symbols: BTreeMap<String, usize>,
    ) -> Result<CompiledAsyncJit> {
        let mut publication = rinzu_compiler::timing::span("jit.managed.publish");
        ensure!(
            external_symbols.values().all(|&address| address != 0),
            "null async runtime symbol"
        );
        self.externals
            .set(external_symbols)
            .map_err(|_| anyhow!("async runtime symbols already set"))?;
        self.module
            .finalize_definitions()
            .context("finalize compiled async JIT")?;
        let memory = self.audit.snapshot();
        ensure!(
            memory.executable_mappings > 0 && memory.is_published(),
            "compiled async JIT violated W^X publication"
        );
        publication.set_counter("executable_mappings", memory.executable_mappings as u64);
        publication.set_counter("rx_transitions", memory.rx_transitions as u64);
        publication.set_counter("icache_flushes", memory.icache_flushes as u64);
        publication.set_counter("rwx_mappings", memory.rwx_mappings as u64);
        let symbols = self
            .module
            .declarations()
            .get_functions()
            .filter(|(_, declaration)| declaration.linkage == Linkage::Export)
            .map(|(id, declaration)| {
                (
                    declaration.name.clone().expect("export has a name"),
                    self.module.get_finalized_function(id) as usize,
                )
            })
            .collect();
        Ok(CompiledAsyncJit {
            module: Some(self.module),
            audit: self.audit,
            symbols,
            compile: self.compile,
            executable_mappings: memory.executable_mappings,
            executable_bytes: memory.executable_bytes,
            icache_flushes: memory.icache_flushes,
            rx_transitions: memory.rx_transitions,
            rwx_mappings: memory.rwx_mappings,
        })
    }
}
