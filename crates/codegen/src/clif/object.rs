//! Emit native objects from the shared managed lowering.
use super::*;
use crate::baseline::{AotFunctionMetadata, BaselineAotObject};
use cranelift_object::{ObjectBuilder, ObjectModule};
use rinzu_runtime::failure::SourcePosition;

pub(crate) fn emit_async_plans(
    target: Target,
    ir: &IrModule,
    entry: &str,
) -> Result<BaselineAotObject> {
    program_metadata(
        target,
        ir,
        entry,
        &crate::data_schema::DataSchema::new(ir)?,
        &crate::function_reference::ReferenceTargets::new(ir)?,
        crate::native_call::NativeCallPlans::new(ir)?.records,
    )
}

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
    let mut result = program_metadata(
        target,
        module_ir,
        entry,
        &runtime_functions.data_schema,
        &runtime_functions.reference_targets,
        runtime_functions.native_plans.records.clone(),
    )?;
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
    result.object = module
        .finish()
        .emit()
        .context("emit baseline AOT object bytes")?;
    ensure!(!result.object.is_empty(), "baseline AOT object is empty");
    result.c_function_source = c_output.source;
    result.c_functions = c_output.names;
    result.c_fallbacks = c_output.fallbacks;
    result.compile = compile;
    Ok(result)
}

fn program_metadata(
    target: Target,
    ir: &IrModule,
    entry: &str,
    data_schema: &crate::data_schema::DataSchema,
    references: &crate::function_reference::ReferenceTargets,
    native_call_plans: Vec<crate::NativeCallPlan>,
) -> Result<BaselineAotObject> {
    let (source_ids, source_files) = source_identities(ir);
    let functions = ir
        .functions
        .iter()
        .enumerate()
        .map(|(index, function)| {
            let range = function.range;
            Ok(AotFunctionMetadata {
                source: SourcePosition {
                    file: source_ids
                        .get(&range.file_id.0)
                        .copied()
                        .unwrap_or(u64::from(range.file_id.0)),
                    start_byte: range.start_byte,
                    end_byte: range.end_byte,
                },
                index,
                name: function.name.clone(),
                symbol: managed_symbol(index, &function.name),
                parameter_count: parameter_count(function)?,
                parameter_types: function.parameter_types.clone(),
                result_type: function.result_type.clone(),
                is_async: function.is_async,
                callback_fallback: function.callback_fallback.clone(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let entry = functions
        .iter()
        .find(|function| function.name == entry)
        .ok_or_else(|| anyhow!("missing AOT entry function {entry}"))?;
    ensure!(
        entry.parameter_count == 0,
        "AOT entry must not take arguments"
    );
    Ok(BaselineAotObject {
        target,
        object: Vec::new(),
        c_function_source: String::new(),
        c_functions: Vec::new(),
        c_fallbacks: BTreeMap::new(),
        entry_index: entry.index,
        entry_symbol: entry.symbol.clone(),
        functions,
        source_files,
        source_locations_c: crate::error_sources::c_source(ir),
        data_schema_c: data_schema.c_source(),
        uses_json: data_schema.json.is_some(),
        data_types: data_schema.metadata(),
        reference_targets: references.metadata(),
        native_call_plans,
        native_signatures: ir.source_native_signatures.clone(),
        native_record_schemas: ir.native_records.clone(),
        compile: CompileEvidence {
            stack_map_count: 0,
            stack_map_entry_count: 0,
            code_size: 0,
        },
    })
}
