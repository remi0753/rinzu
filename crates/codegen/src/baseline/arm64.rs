//! Cranelift lowering and JIT/object emission for macOS arm64.
use super::*;

mod lowering;
mod runtime;

use anyhow::{Context, ensure};
use rinzu_compiler::{
    BasicBlock, BinaryOperator, BlockId, ConstantValue, IrFunction, OperationKind, StackRoot,
    Terminator, UnaryOperator, ValueId,
};
use std::collections::HashSet;

use cranelift_codegen::ir::{AbiParam, InstBuilder, UserFuncName, Value, condcodes::IntCC, types};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Switch, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{DataDescription, FuncId, Linkage, Module, default_libcall_names};
use cranelift_object::{ObjectBuilder, ObjectModule};
use lowering::define_function;
pub(super) use runtime::run;

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

const OP_NEGATE: i64 = 0;
const OP_ADD: i64 = 1;
const OP_SUBTRACT: i64 = 2;
const OP_MULTIPLY: i64 = 3;
const OP_DIVIDE: i64 = 4;
const OP_REMAINDER: i64 = 5;

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
    let runtime_functions = declare_runtime_functions(&mut module, module_ir, source_async, true)?;
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
        uses_json: runtime_functions.data_schema.json.is_some(),
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

#[cfg(feature = "test-c-lowering-probe")]
pub(super) fn inspect_clif(ir: &IrModule) -> Result<Vec<(String, String)>> {
    let isa = super::super::native_isa(true)?;
    let builder = ObjectBuilder::new(isa, "rinzu-c-lowering-inventory", default_libcall_names())?;
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
