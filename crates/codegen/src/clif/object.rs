//! Emit native objects from the shared managed lowering.
use super::*;
use crate::baseline::{AotFunctionMetadata, BaselineAotObject};
use cranelift_object::{ObjectBuilder, ObjectModule};
use rinzu_runtime::failure::SourcePosition;

pub(crate) fn emit_aot(
    target: Target,
    module_ir: &IrModule,
    entry: &str,
    native_imports: &HashMap<String, u64>,
    source_async: bool,
    c_backend: bool,
) -> Result<BaselineAotObject> {
    let isa = isa::aot_isa(target)?;
    let builder = ObjectBuilder::new(isa, "rinzu-baseline-aot", default_libcall_names())
        .context("create baseline AOT object backend")?;
    let mut module = ObjectModule::new(builder);
    let runtime_functions = declare_runtime_functions(
        &mut module,
        module_ir,
        source_async,
        true,
        uses_direct_c_context(target),
    )?;
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
        target,
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
        uses_json: runtime_functions.data_schema.json.is_some(),
        data_types: runtime_functions.data_schema.metadata(),
        reference_targets: runtime_functions.reference_targets.metadata(),
        native_call_plans: runtime_functions.native_plans.records,
        native_signatures: module_ir.source_native_signatures.clone(),
        native_record_schemas: module_ir.native_records.clone(),
        compile,
    })
}
