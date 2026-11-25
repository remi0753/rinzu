use super::*;
use cranelift_codegen::{
    control::ControlPlane,
    incremental_cache::CacheKvStore,
    ir::{MemFlagsData, SourceLoc},
};
use cranelift_module::ModuleReloc;
use std::borrow::Cow;

#[derive(Default)]
struct Store(HashMap<Vec<u8>, Vec<u8>>);

impl CacheKvStore for Store {
    fn get(&self, key: &[u8]) -> Option<Cow<'_, [u8]>> {
        self.0.get(key).map(|bytes| Cow::Borrowed(bytes.as_slice()))
    }

    fn insert(&mut self, key: &[u8], value: Vec<u8>) {
        self.0.insert(key.to_vec(), value);
    }
}

fn define(
    module: &mut JITModule,
    id: FuncId,
    ctx: &mut cranelift_codegen::Context,
    cache: &mut Store,
) -> bool {
    let (_, reused) = ctx
        .compile_with_cache(module.isa(), cache, &mut ControlPlane::default())
        .unwrap();
    let code = ctx.compiled_code().unwrap();
    let relocs = code
        .buffer
        .relocs()
        .iter()
        .map(|reloc| ModuleReloc::from_mach_reloc(reloc, &ctx.func, id))
        .collect::<Vec<_>>();
    module
        .define_function_bytes(
            id,
            u64::from(code.buffer.alignment),
            code.code_buffer(),
            &relocs,
        )
        .unwrap();
    reused
}

#[test]
fn compile_with_cache_rebinds_functions_constants_and_source_locations() {
    let compiler = AsyncJitCompiler::new().unwrap();
    let mut cache = Store::default();
    let mut previous = None;
    for changed in [false, true] {
        let mut prepared = compiler.module().unwrap();
        let module = &mut prepared.module;
        let mut signature = module.make_signature();
        signature.returns.push(AbiParam::new(types::I64));
        if changed {
            module
                .declare_function("unreferenced", Linkage::Import, &signature)
                .unwrap();
            let padding = module.declare_anonymous_data(false, false).unwrap();
            let mut description = DataDescription::new();
            description.define(Box::new([0; 8]));
            module.define_data(padding, &description).unwrap();
        }
        let answer = module
            .declare_function("answer", Linkage::Export, &signature)
            .unwrap();
        let caller = module
            .declare_function("caller", Linkage::Export, &signature)
            .unwrap();
        let data = module.declare_anonymous_data(false, false).unwrap();
        let mut description = DataDescription::new();
        description.align = Some(8);
        let addend = if changed { 2i64 } else { 1i64 };
        description.define(Box::new(addend.to_ne_bytes()));
        module.define_data(data, &description).unwrap();

        let mut context = module.make_context();
        context.func.signature = signature.clone();
        {
            let mut builder_context = FunctionBuilderContext::new();
            let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
            let block = builder.create_block();
            builder.switch_to_block(block);
            let answer = builder
                .ins()
                .iconst(types::I64, if changed { 20 } else { 10 });
            builder.ins().return_(&[answer]);
            builder.seal_all_blocks();
            builder.finalize(module.target_config());
        }
        assert!(!define(module, answer, &mut context, &mut cache));

        module.clear_context(&mut context);
        context.func.signature = signature;
        let location = SourceLoc::new(if changed { 150 } else { 50 });
        {
            let mut builder_context = FunctionBuilderContext::new();
            let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
            let block = builder.create_block();
            builder.switch_to_block(block);
            builder.set_srcloc(location);
            let global = module.declare_data_in_func(data, builder.func);
            let callee = module.declare_func_in_func(answer, builder.func);
            let address = builder
                .ins()
                .symbol_value(module.isa().pointer_type(), global);
            let kept = builder
                .ins()
                .load(types::I64, MemFlagsData::trusted(), address, 0);
            builder.declare_value_needs_stack_map(kept);
            let call = builder.ins().call(callee, &[]);
            let result = builder.inst_results(call)[0];
            let total = builder.ins().iadd(kept, result);
            builder.ins().return_(&[total]);
            builder.seal_all_blocks();
            builder.finalize(module.target_config());
        }
        assert_eq!(define(module, caller, &mut context, &mut cache), changed);
        let code = context.compiled_code().unwrap();
        assert_eq!(code.buffer.user_stack_maps().len(), 1);
        assert_eq!(code.buffer.user_stack_maps()[0].2.entries().count(), 1);
        let positions = code
            .buffer
            .get_srclocs_sorted()
            .iter()
            .map(|entry| entry.loc)
            .filter(|position| !position.is_default())
            .collect::<Vec<_>>();
        assert!(!positions.is_empty() && positions.iter().all(|&position| position == location));
        let stencil = (code.buffer.alignment, code.code_buffer().to_vec());
        if let Some(previous) = &previous {
            assert_eq!(&stencil, previous);
        }
        previous = Some(stencil);

        let published = prepared.publish(BTreeMap::new()).unwrap();
        assert_eq!(published.executable_mappings, 2);
        assert_eq!(published.rx_transitions, 2);
        #[cfg(target_os = "macos")]
        assert_eq!(published.icache_flushes, 2);
        #[cfg(target_os = "linux")]
        assert_eq!(published.icache_flushes, 0);
        assert_eq!(published.rwx_mappings, 0);
        // SAFETY: the published caller has the declared C ABI with no arguments and returns i64.
        let call: extern "C" fn() -> i64 =
            unsafe { std::mem::transmute(published.symbols["caller"]) };
        assert_eq!(call(), if changed { 22 } else { 11 });
        // SAFETY: the synchronous call has returned and no other entry points were shared.
        unsafe { published.retire().unwrap() };
    }
}
