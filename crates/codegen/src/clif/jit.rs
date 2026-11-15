//! Publish compiled async code in the host JIT.
use super::*;

pub(crate) fn compile_async_jit(
    ir: &IrModule,
    external_symbols: &BTreeMap<String, usize>,
    native_imports: &HashMap<String, u64>,
) -> Result<CompiledAsyncJit> {
    let target = Target::host().ok_or_else(|| anyhow!("unsupported async JIT host"))?;
    let isa = isa::jit_isa()?;
    let mut builder = JITBuilder::with_isa(isa, default_libcall_names());
    for (symbol, address) in external_symbols {
        ensure!(*address != 0, "null async runtime symbol");
        builder.symbol(symbol, address.to_owned() as *const u8);
    }
    let (memory, audit) = JitMemory::new_with_audit()?;
    builder.memory_provider(Box::new(memory));
    let mut module = JITModule::new(builder);
    let runtime =
        declare_runtime_functions(&mut module, ir, true, true, uses_direct_c_context(target))?;
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
        memory.executable_mappings > 0 && memory.is_published(),
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
        audit,
        symbols,
        compile,
        executable_mappings: memory.executable_mappings,
        executable_bytes: memory.executable_bytes,
        icache_flushes: memory.icache_flushes,
        rx_transitions: memory.rx_transitions,
        rwx_mappings: memory.rwx_mappings,
    })
}
