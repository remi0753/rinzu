//! Preserve each Error-origin write while avoiding a call for private C Contexts.
use cranelift_codegen::ir::{FuncRef, InstBuilder, MemFlagsData, Value, condcodes::IntCC, types};
use cranelift_frontend::FunctionBuilder;

pub(super) fn lower(
    builder: &mut FunctionBuilder<'_>,
    context: Value,
    future: Option<Value>,
    direct_c_context: bool,
    general_helper: FuncRef,
) {
    let zero = builder.ins().iconst(types::I64, 0);
    if !direct_c_context {
        builder
            .ins()
            .call(general_helper, &[context, future.unwrap_or(zero)]);
        return;
    }
    let origin = if let Some(future) = future {
        let base = builder.ins().iconst(types::I64, crate::C_ASYNC_HANDLE_BASE);
        let valid = builder
            .ins()
            .icmp(IntCC::SignedGreaterThanOrEqual, future, base);
        let identity = builder.ins().isub(future, base);
        builder.ins().select(valid, identity, zero)
    } else {
        zero
    };
    builder.ins().store(
        MemFlagsData::new(),
        origin,
        context,
        crate::C_CONTEXT_FAILURE_ORIGIN_OFFSET,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use cranelift_codegen::ir::AbiParam;
    use cranelift_frontend::FunctionBuilderContext;
    use cranelift_module::{Linkage, Module, default_libcall_names};
    use cranelift_object::{ObjectBuilder, ObjectModule};

    #[test]
    fn emitted_origin_stores_match_the_helper_on_every_context_byte() {
        let mut module = ObjectModule::new(
            ObjectBuilder::new(
                crate::native_isa(true).unwrap(),
                "origin-store",
                default_libcall_names(),
            )
            .unwrap(),
        );
        let mut helper_sig = module.make_signature();
        helper_sig.params = vec![AbiParam::new(types::I64); 2];
        let helper = module
            .declare_function(
                "rinzu_baseline_task_error_origin",
                Linkage::Import,
                &helper_sig,
            )
            .unwrap();
        for (name, direct, has_future) in [
            ("origin_zero", true, false),
            ("origin_future", true, true),
            ("origin_fallback", false, true),
        ] {
            let mut context = module.make_context();
            context.func.signature = helper_sig.clone();
            context
                .func
                .signature
                .returns
                .push(AbiParam::new(types::I64));
            let id = module
                .declare_function(name, Linkage::Export, &context.func.signature)
                .unwrap();
            context.func.name = cranelift_codegen::ir::UserFuncName::user(0, id.as_u32());
            let general = module.declare_func_in_func(helper, &mut context.func);
            let mut frontend = FunctionBuilderContext::new();
            let mut builder = FunctionBuilder::new(&mut context.func, &mut frontend);
            let entry = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            let raw = builder.block_params(entry)[0];
            let future = has_future.then_some(builder.block_params(entry)[1]);
            lower(&mut builder, raw, future, direct, general);
            let zero = builder.ins().iconst(types::I64, 0);
            builder.ins().return_(&[zero]);
            builder.seal_all_blocks();
            builder.finalize(module.isa().frontend_config());
            module.define_function(id, &mut context).unwrap();
        }
        let root = std::env::temp_dir().join(format!("rinzu-origin-store-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let object = root.join("origin.o");
        std::fs::write(&object, module.finish().emit().unwrap()).unwrap();
        let source = root.join("check.c");
        std::fs::write(&source, r#"
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
extern int64_t origin_zero(void*,int64_t),origin_future(void*,int64_t),origin_fallback(void*,int64_t);
void rinzu_baseline_task_error_origin(void*raw,int64_t future){
 uint64_t value=future>=(int64_t)(UINT64_C(1)<<61)?(uint64_t)future-(UINT64_C(1)<<61):0;
 /* Mirror the independently checked RinzuContext layout in driver/aot.rs. */
 memcpy((unsigned char*)raw+896,&value,8);
}
int main(void){
 uint64_t edges[]={0,1,(UINT64_C(1)<<61)-1,UINT64_C(1)<<61,(UINT64_C(1)<<61)+1,INT64_MAX,UINT64_C(1)<<63,UINT64_MAX};
 int64_t(*functions[])(void*,int64_t)={origin_zero,origin_future,origin_fallback};
 uint64_t random=UINT64_C(0x123456789abcdef);
 for(unsigned mode=0;mode<3;++mode)for(unsigned pattern=0;pattern<4;++pattern)for(unsigned i=0;i<2056;++i){
  uint64_t actual[129],expected[129];unsigned char*p=(unsigned char*)actual;
  for(unsigned k=0;k<1032;++k)p[k]=pattern==0?0:pattern==1?255:pattern==2?0xa5:(unsigned char)(k*31+i);
  memcpy(expected,actual,1032);random^=random<<13;random^=random>>7;random^=random<<17;
  uint64_t bits=i<8?edges[i]:random;int64_t future;memcpy(&future,&bits,8);
  rinzu_baseline_task_error_origin(expected,mode==0?0:future);
  if(functions[mode](actual,future)!=0||memcmp(actual,expected,1032))abort();
 }
 return 0;
}
"#).unwrap();
        let executable = root.join("check");
        let compile = std::process::Command::new("clang")
            .args(["-target", "arm64-apple-macos11", "-std=c11", "-O2"])
            .arg(&source)
            .arg(&object)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let run = std::process::Command::new(&executable).output().unwrap();
        assert!(run.status.success(), "{run:?}");
        std::fs::remove_dir_all(root).unwrap();
    }
}
