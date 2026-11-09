//! Lower typed IR functions, control flow and runtime calls into Cranelift IR.
use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn define_function<M: Module>(
    module: &mut M,
    function: &IrFunction,
    function_index: usize,
    functions: &HashMap<String, FunctionInfo>,
    runtime_functions: &RuntimeFunctions,
    native_imports: &HashMap<String, u64>,
    colocate_runtime_calls: bool,
    evidence: &mut CompileEvidence,
    inspection: Option<&mut Vec<(String, String)>>,
    c_output: Option<&mut CFunctions>,
) -> Result<usize> {
    let info = functions
        .get(&function.name)
        .expect("all functions were declared");
    let mut context = module.make_context();
    context.func.name = UserFuncName::user(1, info.id.as_u32());
    context.func.signature = module
        .declarations()
        .get_function_decl(info.id)
        .signature
        .clone();
    let mut string_literals = HashMap::new();
    let mut literal_bytes = HashMap::new();
    for operation in function.blocks.iter().flat_map(|block| &block.operations) {
        let bytes = match &operation.kind {
            OperationKind::Constant(ConstantValue::String(text)) => text.as_bytes(),
            OperationKind::Constant(ConstantValue::Bytes(bytes)) => bytes.as_slice(),
            _ => continue,
        };
        let address = if bytes.is_empty() {
            None
        } else {
            let data = module.declare_anonymous_data(false, false)?;
            let mut description = DataDescription::new();
            description.define(bytes.to_vec().into_boxed_slice());
            module.define_data(data, &description)?;
            literal_bytes.insert(data.as_u32(), bytes.to_vec());
            Some(module.declare_data_in_func(data, &mut context.func))
        };
        string_literals.insert(operation.result.as_ref().unwrap().id, address);
    }
    let mut builder_context = FunctionBuilderContext::new();
    let c_managed_signatures;
    {
        let target_config = module.target_config();
        let source_async =
            runtime_functions
                .source_async
                .as_ref()
                .map(|runtime| AsyncRuntimeRefs {
                    take_task_errors: module
                        .declare_func_in_func(runtime.take_task_errors, &mut context.func),
                    task_error_origin: module
                        .declare_func_in_func(runtime.task_error_origin, &mut context.func),
                    task_scope: module.declare_func_in_func(runtime.task_scope, &mut context.func),
                    spawn: module.declare_func_in_func(runtime.spawn, &mut context.func),
                    await_value: module
                        .declare_func_in_func(runtime.await_value, &mut context.func),
                    call: module.declare_func_in_func(runtime.call, &mut context.func),
                    service: module.declare_func_in_func(runtime.service, &mut context.func),
                    supplant_reserve: module
                        .declare_func_in_func(runtime.supplant_reserve, &mut context.func),
                    supplant_start: module
                        .declare_func_in_func(runtime.supplant_start, &mut context.func),
                    supplant_settle: module
                        .declare_func_in_func(runtime.supplant_settle, &mut context.func),
                });
        let runtime_refs = RuntimeFunctionRefs {
            proven_owner_operations: runtime_functions
                .proven_owner_operations
                .get(&function.name)
                .cloned()
                .unwrap_or_default(),
            owner_helpers: runtime_functions
                .owner_helpers
                .iter()
                .map(|(name, id)| (*name, module.declare_func_in_func(*id, &mut context.func)))
                .collect(),
            fresh_return_consuming_awaits: runtime_functions
                .fresh_return_consuming_awaits
                .get(&function.name)
                .cloned()
                .unwrap_or_default(),
            direct_source: colocate_runtime_calls || runtime_functions.source_async.is_some(),
            virtual_bytes_options: HashSet::new(),
            dead_temporary_roots: HashSet::new(),
            dead_local_roots: HashSet::new(),
            proven_frame_roots: runtime_functions
                .proven_frame_roots
                .get(&function.name)
                .cloned()
                .unwrap_or_default(),
            proven_frame_memberships: runtime_functions
                .proven_frame_memberships
                .get(&function.name)
                .cloned()
                .unwrap_or_default(),
            function_index,
            data_schema: runtime_functions.data_schema.clone(),
            reference_targets: runtime_functions.reference_targets.clone(),
            native_plans: runtime_functions.native_plans.clone(),
            supplant_values: Default::default(),
            native_helpers: runtime_functions
                .native_helpers
                .iter()
                .map(|(name, id)| (*name, module.declare_func_in_func(*id, &mut context.func)))
                .collect(),
            native_stubs: runtime_functions
                .native_stubs
                .iter()
                .map(|(name, id)| {
                    (
                        name.clone(),
                        module.declare_func_in_func(*id, &mut context.func),
                    )
                })
                .collect(),
            source_async,
            unwind: None,
            source_ids: runtime_functions.source_ids.clone(),
            string_literals,
            set_source: module
                .declare_func_in_func(runtime_functions.set_source, &mut context.func),
            observe_source: module
                .declare_func_in_func(runtime_functions.observe_source, &mut context.func),
            failure: module.declare_func_in_func(runtime_functions.failure, &mut context.func),
            entry: module.declare_func_in_func(runtime_functions.entry, &mut context.func),
            publish_roots: module
                .declare_func_in_func(runtime_functions.publish_roots, &mut context.func),
            prepare_roots: module
                .declare_func_in_func(runtime_functions.prepare_roots, &mut context.func),
            commit_roots: module
                .declare_func_in_func(runtime_functions.commit_roots, &mut context.func),
            commit_proven_roots: runtime_functions
                .commit_proven_roots
                .map(|id| module.declare_func_in_func(id, &mut context.func)),
            commit_partial_roots: runtime_functions
                .commit_partial_roots
                .map(|id| module.declare_func_in_func(id, &mut context.func)),
            construct: module.declare_func_in_func(runtime_functions.construct, &mut context.func),
            data_operation: module
                .declare_func_in_func(runtime_functions.data_operation, &mut context.func),
            bytes_slice_payload: runtime_functions
                .bytes_slice_payload
                .map(|id| module.declare_func_in_func(id, &mut context.func)),
            bytes_int_length: module
                .declare_func_in_func(runtime_functions.bytes_int_length, &mut context.func),
            string_scalar_length: module
                .declare_func_in_func(runtime_functions.string_scalar_length, &mut context.func),
            collection_length: module
                .declare_func_in_func(runtime_functions.collection_length, &mut context.func),
            bytes: module.declare_func_in_func(runtime_functions.bytes, &mut context.func),
            bytes_pad: module.declare_func_in_func(runtime_functions.bytes_pad, &mut context.func),
            bytes_length: module
                .declare_func_in_func(runtime_functions.bytes_length, &mut context.func),
            store_field: module
                .declare_func_in_func(runtime_functions.store_field, &mut context.func),
            project: module.declare_func_in_func(runtime_functions.project, &mut context.func),
            enum_variant: module
                .declare_func_in_func(runtime_functions.enum_variant, &mut context.func),
            value_compare: module
                .declare_func_in_func(runtime_functions.value_compare, &mut context.func),
            scalar_arithmetic: module
                .declare_func_in_func(runtime_functions.scalar_arithmetic, &mut context.func),
            scalar_compare: module
                .declare_func_in_func(runtime_functions.scalar_compare, &mut context.func),
            arithmetic: module
                .declare_func_in_func(runtime_functions.arithmetic, &mut context.func),
            scalar_convert: module
                .declare_func_in_func(runtime_functions.scalar_convert, &mut context.func),
            has_error: module.declare_func_in_func(runtime_functions.has_error, &mut context.func),
            catch_error: module
                .declare_func_in_func(runtime_functions.catch_error, &mut context.func),
            string: module.declare_func_in_func(runtime_functions.string, &mut context.func),
            error_message: module
                .declare_func_in_func(runtime_functions.error_message, &mut context.func),
            error_data: module
                .declare_func_in_func(runtime_functions.error_data, &mut context.func),
            stdout: module.declare_func_in_func(runtime_functions.stdout, &mut context.func),
            format_string: module
                .declare_func_in_func(runtime_functions.format_string, &mut context.func),
            task_control: module
                .declare_func_in_func(runtime_functions.task_control, &mut context.func),
            utf8_length: module
                .declare_func_in_func(runtime_functions.utf8_length, &mut context.func),
            string_compare: module
                .declare_func_in_func(runtime_functions.string_compare, &mut context.func),
            begin_cleanup: module
                .declare_func_in_func(runtime_functions.begin_cleanup, &mut context.func),
            end_cleanup: module
                .declare_func_in_func(runtime_functions.end_cleanup, &mut context.func),
            resolve_dispatch: module
                .declare_func_in_func(runtime_functions.resolve_dispatch, &mut context.func),
            leave_dispatch: module
                .declare_func_in_func(runtime_functions.leave_dispatch, &mut context.func),
            retain_return: module
                .declare_func_in_func(runtime_functions.retain_return, &mut context.func),
            native_invoke: module
                .declare_func_in_func(runtime_functions.native_invoke, &mut context.func),
        };
        let managed_signatures = functions
            .iter()
            .map(|(name, info)| {
                (
                    name.clone(),
                    context.func.import_signature(info.signature.clone()),
                )
            })
            .collect::<HashMap<_, _>>();
        c_managed_signatures = managed_signatures.values().copied().collect::<HashSet<_>>();
        let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
        lower_function(
            &mut builder,
            function,
            function_index,
            functions,
            &managed_signatures,
            &runtime_refs,
            native_imports,
        )?;
        builder.finalize(target_config);
    }
    if colocate_runtime_calls {
        // AOT links these fixed helpers with their generated C runtime in
        // the same image. Keep Import declarations: C owns the definitions
        // and the async host must still be able to look up their exports.
        // JIT helper addresses can be arbitrarily far away and retain the
        // default lowering. Other external/native symbols are untouched.
        let names = context.func.params.user_named_funcs();
        for external in context.func.stencil.dfg.ext_funcs.values_mut() {
            let cranelift_codegen::ir::ExternalName::User(name) = &external.name else {
                continue;
            };
            let name = &names[*name];
            if name.namespace == 0
                && runtime_functions
                    .owned_imports
                    .contains(&FuncId::from_u32(name.index))
            {
                external.colocated = true;
            }
        }
    }
    if let Some(rows) = inspection {
        let mut text = String::new();
        let mut opcodes = BTreeMap::<String, usize>::new();
        for block in context.func.layout.blocks() {
            for inst in context.func.layout.block_insts(block) {
                *opcodes
                    .entry(context.func.dfg.insts[inst].opcode().to_string())
                    .or_default() += 1;
            }
        }
        for (opcode, count) in opcodes {
            text.push_str(&format!("; C07F opcode {opcode} {count}\n"));
        }
        for (reference, external) in context.func.dfg.ext_funcs.iter() {
            if let cranelift_codegen::ir::ExternalName::User(name) = &external.name {
                let name = &context.func.params.user_named_funcs()[*name];
                if name.namespace == 0 {
                    let declaration = module
                        .declarations()
                        .get_function_decl(FuncId::from_u32(name.index));
                    text.push_str(&format!(
                        "; C07F {reference} {}\n",
                        declaration.name.as_deref().unwrap_or("<anonymous>")
                    ));
                }
            }
        }
        text.push_str(&context.func.display().to_string());
        rows.push((function.name.clone(), text));
        return Ok(0);
    }
    if let Some(output) = c_output {
        if function.is_async {
            output
                .fallbacks
                .insert(function.name.clone(), "async body".into());
        } else {
            let emitted = (|| -> Result<crate::c_lowering::Emission> {
                let mut imports = HashMap::new();
                for (reference, external) in context.func.dfg.ext_funcs.iter() {
                    if let cranelift_codegen::ir::ExternalName::User(name) = &external.name {
                        let name = &context.func.params.user_named_funcs()[*name];
                        if name.namespace == 0 {
                            let declaration = module
                                .declarations()
                                .get_function_decl(FuncId::from_u32(name.index));
                            imports.insert(
                                reference,
                                declaration
                                    .name
                                    .clone()
                                    .ok_or_else(|| anyhow!("anonymous C import"))?,
                            );
                        }
                    }
                }
                let mut literals = HashMap::new();
                for (global, data) in context.func.global_values.iter() {
                    let cranelift_codegen::ir::GlobalValueData::Symbol {
                        name: cranelift_codegen::ir::ExternalName::User(name),
                        offset,
                        tls: false,
                        ..
                    } = data
                    else {
                        bail!("unsupported C global")
                    };
                    let name = &context.func.params.user_named_funcs()[*name];
                    ensure!(name.namespace == 1, "non-data C global");
                    literals.insert(
                        global,
                        (
                            literal_bytes
                                .get(&name.index)
                                .ok_or_else(|| anyhow!("nonliteral C global"))?
                                .clone(),
                            offset.bits(),
                        ),
                    );
                }
                crate::c_lowering::emit(
                    &context.func,
                    &info.symbol,
                    &imports,
                    &literals,
                    &c_managed_signatures,
                    module.isa(),
                )
            })();
            match emitted {
                Ok(emission) => {
                    output.source.push_str(&format!(
                        "/* {} lowered SSA instructions */\n",
                        emission.instructions
                    ));
                    output.source.push_str(&emission.source);
                    output.names.push(function.name.clone());
                    return Ok(0);
                }
                Err(error) => {
                    output
                        .fallbacks
                        .insert(function.name.clone(), error.to_string());
                }
            }
        }
    }
    if module.declarations().get_function_decl(info.id).linkage == Linkage::Import {
        module.declare_function(&info.symbol, Linkage::Export, &info.signature)?;
    }
    module
        .define_function(info.id, &mut context)
        .with_context(|| format!("compile baseline function {}", function.name))?;
    let compiled = context
        .compiled_code()
        .ok_or_else(|| anyhow!("Cranelift retained no code for {}", function.name))?;
    let code_size = compiled.code_buffer().len();
    evidence.code_size += code_size;
    evidence.stack_map_count += compiled.buffer.user_stack_maps().len();
    evidence.stack_map_entry_count += compiled
        .buffer
        .user_stack_maps()
        .iter()
        .map(|(_, _, map)| map.entries().count())
        .sum::<usize>();
    module.clear_context(&mut context);
    Ok(code_size)
}

#[derive(Clone)]
struct AsyncRuntimeRefs {
    take_task_errors: cranelift_codegen::ir::FuncRef,
    task_error_origin: cranelift_codegen::ir::FuncRef,
    task_scope: cranelift_codegen::ir::FuncRef,
    spawn: cranelift_codegen::ir::FuncRef,
    await_value: cranelift_codegen::ir::FuncRef,
    call: cranelift_codegen::ir::FuncRef,
    service: cranelift_codegen::ir::FuncRef,
    supplant_reserve: cranelift_codegen::ir::FuncRef,
    supplant_start: cranelift_codegen::ir::FuncRef,
    supplant_settle: cranelift_codegen::ir::FuncRef,
}

#[derive(Clone)]
struct RuntimeFunctionRefs {
    proven_owner_operations: HashSet<(BlockId, usize)>,
    owner_helpers: BTreeMap<&'static str, cranelift_codegen::ir::FuncRef>,
    direct_source: bool,
    virtual_bytes_options: HashSet<ValueId>,
    dead_temporary_roots: HashSet<(BlockId, usize, ValueId)>,
    dead_local_roots: HashSet<(BlockId, usize, String)>,
    proven_frame_roots: HashSet<(BlockId, usize)>,
    proven_frame_memberships: HashMap<(BlockId, usize), u64>,
    fresh_return_consuming_awaits: HashSet<(BlockId, usize)>,
    function_index: usize,
    reference_targets: crate::function_reference::ReferenceTargets,
    native_plans: crate::native_call::NativeCallPlans,
    supplant_values: std::rc::Rc<
        std::cell::RefCell<HashMap<rinzu_compiler::ReservationId, cranelift_codegen::ir::Value>>,
    >,
    native_helpers: BTreeMap<&'static str, cranelift_codegen::ir::FuncRef>,
    native_stubs: BTreeMap<String, cranelift_codegen::ir::FuncRef>,
    data_schema: crate::data_schema::DataSchema,
    source_async: Option<AsyncRuntimeRefs>,
    source_ids: HashMap<u32, u64>,
    set_source: cranelift_codegen::ir::FuncRef,
    observe_source: cranelift_codegen::ir::FuncRef,
    failure: cranelift_codegen::ir::FuncRef,
    entry: cranelift_codegen::ir::FuncRef,
    publish_roots: cranelift_codegen::ir::FuncRef,
    prepare_roots: cranelift_codegen::ir::FuncRef,
    commit_roots: cranelift_codegen::ir::FuncRef,
    commit_proven_roots: Option<cranelift_codegen::ir::FuncRef>,
    commit_partial_roots: Option<cranelift_codegen::ir::FuncRef>,
    construct: cranelift_codegen::ir::FuncRef,
    data_operation: cranelift_codegen::ir::FuncRef,
    bytes_slice_payload: Option<cranelift_codegen::ir::FuncRef>,
    bytes_int_length: cranelift_codegen::ir::FuncRef,
    string_scalar_length: cranelift_codegen::ir::FuncRef,
    collection_length: cranelift_codegen::ir::FuncRef,
    bytes: cranelift_codegen::ir::FuncRef,
    bytes_pad: cranelift_codegen::ir::FuncRef,
    bytes_length: cranelift_codegen::ir::FuncRef,
    store_field: cranelift_codegen::ir::FuncRef,
    project: cranelift_codegen::ir::FuncRef,
    enum_variant: cranelift_codegen::ir::FuncRef,
    value_compare: cranelift_codegen::ir::FuncRef,
    arithmetic: cranelift_codegen::ir::FuncRef,
    scalar_arithmetic: cranelift_codegen::ir::FuncRef,
    scalar_compare: cranelift_codegen::ir::FuncRef,
    scalar_convert: cranelift_codegen::ir::FuncRef,
    has_error: cranelift_codegen::ir::FuncRef,
    catch_error: cranelift_codegen::ir::FuncRef,
    string: cranelift_codegen::ir::FuncRef,
    error_message: cranelift_codegen::ir::FuncRef,
    error_data: cranelift_codegen::ir::FuncRef,
    stdout: cranelift_codegen::ir::FuncRef,
    format_string: cranelift_codegen::ir::FuncRef,
    utf8_length: cranelift_codegen::ir::FuncRef,
    task_control: cranelift_codegen::ir::FuncRef,
    string_compare: cranelift_codegen::ir::FuncRef,
    string_literals: HashMap<ValueId, Option<cranelift_codegen::ir::GlobalValue>>,
    begin_cleanup: cranelift_codegen::ir::FuncRef,
    end_cleanup: cranelift_codegen::ir::FuncRef,
    resolve_dispatch: cranelift_codegen::ir::FuncRef,
    leave_dispatch: cranelift_codegen::ir::FuncRef,
    retain_return: cranelift_codegen::ir::FuncRef,
    native_invoke: cranelift_codegen::ir::FuncRef,
    unwind: Option<UnwindRoutes>,
}
#[derive(Clone)]
struct UnwindRoutes {
    variable: Variable,
    owner: Variable,
    dispatch: cranelift_codegen::ir::Block,
    paths: std::rc::Rc<std::cell::RefCell<Vec<Vec<BlockId>>>>,
    cleanup_scopes: HashSet<rinzu_compiler::SourceRange>,
    cleanup_ids: HashMap<rinzu_compiler::SourceRange, i64>,
    active_cleanup: std::rc::Rc<std::cell::RefCell<Vec<rinzu_compiler::SourceRange>>>,
    saved_routes: std::rc::Rc<std::cell::RefCell<HashMap<BlockId, (Variable, Variable)>>>,
    return_value: Variable,
    return_target: BlockId,
}
impl UnwindRoutes {
    fn id(&self, path: &[BlockId]) -> i64 {
        if path.is_empty() {
            return 0;
        }
        if let Some(index) = self
            .paths
            .borrow()
            .iter()
            .position(|existing| existing == path)
        {
            return index as i64 + 1;
        }
        self.id(&path[1..]);
        let mut paths = self.paths.borrow_mut();
        paths.push(path.to_vec());
        paths.len() as i64
    }
}

fn owner_helper(
    runtime: &RuntimeFunctionRefs,
    name: &str,
    checked: cranelift_codegen::ir::FuncRef,
    proven: bool,
) -> cranelift_codegen::ir::FuncRef {
    if proven {
        runtime.owner_helpers.get(name).copied().unwrap_or(checked)
    } else {
        checked
    }
}

#[allow(clippy::too_many_arguments)]
fn lower_function(
    builder: &mut FunctionBuilder<'_>,
    function: &IrFunction,
    function_index: usize,
    functions: &HashMap<String, FunctionInfo>,
    managed_signatures: &HashMap<String, cranelift_codegen::ir::SigRef>,
    runtime: &RuntimeFunctionRefs,
    native_imports: &HashMap<String, u64>,
) -> Result<()> {
    let mut runtime = runtime.clone();
    if runtime.bytes_slice_payload.is_some() && runtime.direct_source {
        runtime.virtual_bytes_options = rinzu_compiler::virtual_bytes_options(function);
        runtime.dead_temporary_roots = rinzu_compiler::dead_temporary_roots(function);
        runtime.dead_local_roots = rinzu_compiler::dead_local_roots(function);
    }
    runtime.unwind = Some(UnwindRoutes {
        variable: builder.declare_var(types::I64),
        owner: builder.declare_var(types::I64),
        dispatch: builder.create_block(),
        paths: std::rc::Rc::new(std::cell::RefCell::new(Vec::new())),
        cleanup_ids: function
            .blocks
            .iter()
            .filter(|block| {
                block
                    .operations
                    .iter()
                    .any(|op| matches!(op.kind, OperationKind::LeaveTry { .. }))
            })
            .map(|block| {
                (
                    block.range,
                    ((function_index as u64) << 32 | u64::from(block.id.0)) as i64,
                )
            })
            .collect(),
        cleanup_scopes: function
            .blocks
            .iter()
            .filter(|block| {
                block
                    .operations
                    .iter()
                    .any(|op| matches!(op.kind, OperationKind::LeaveTry { .. }))
            })
            .map(|block| block.range)
            .collect(),
        active_cleanup: std::rc::Rc::new(std::cell::RefCell::new(Vec::new())),
        saved_routes: std::rc::Rc::new(std::cell::RefCell::new(HashMap::new())),
        return_value: builder.declare_var(types::I64),
        return_target: BlockId(
            function
                .blocks
                .iter()
                .map(|block| block.id.0)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or_else(|| anyhow!("no block identity remains for deferred return"))?,
        ),
    });
    let runtime = &runtime;
    let mut blocks = function
        .blocks
        .iter()
        .map(|block| (block.id, builder.create_block()))
        .collect::<HashMap<_, _>>();
    blocks.insert(
        runtime.unwind.as_ref().unwrap().return_target,
        builder.create_block(),
    );
    let entry = blocks[&function.entry];
    builder.append_block_params_for_function_params(entry);
    let context_value = builder.block_params(entry)[0];
    let local_names = function
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .filter_map(|operation| match &operation.kind {
            OperationKind::DeclareLocal { name }
            | OperationKind::CheckLocal { name, .. }
            | OperationKind::LoadLocal { name }
            | OperationKind::StoreLocal { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let mut locals = HashMap::new();
    let mut bound_locals = HashMap::new();
    for name in local_names {
        bound_locals.insert(name.clone(), builder.declare_var(types::I64));
        locals.insert(name, builder.declare_var(types::I64));
    }

    let unwind = runtime.unwind.as_ref().unwrap();
    for block in &function.blocks {
        if block.operations.iter().any(|op| {
            matches!(
                op.kind,
                OperationKind::CatchError { .. }
                    | OperationKind::CatchErrors { .. }
                    | OperationKind::RecordTaskError { .. }
            )
        }) && block
            .entry_scopes
            .iter()
            .any(|scope| unwind.cleanup_scopes.contains(scope))
        {
            unwind.saved_routes.borrow_mut().insert(
                block.id,
                (
                    builder.declare_var(types::I64),
                    builder.declare_var(types::I64),
                ),
            );
        }
    }
    builder.switch_to_block(entry);
    let zero = builder.ins().iconst(types::I64, 0);
    builder.def_var(runtime.unwind.as_ref().unwrap().variable, zero);
    for variable in locals.values().chain(bound_locals.values()) {
        builder.def_var(*variable, zero);
    }
    builder.def_var(runtime.unwind.as_ref().unwrap().owner, zero);
    builder.def_var(runtime.unwind.as_ref().unwrap().return_value, zero);
    for (route, owner) in unwind.saved_routes.borrow().values() {
        builder.def_var(*route, zero);
        builder.def_var(*owner, zero);
    }
    for variable in locals.values() {
        builder.def_var(*variable, zero);
    }
    let function_id = builder
        .ins()
        .iconst(types::I64, i64::try_from(function_index)?);
    let entered = builder
        .ins()
        .call(runtime.entry, &[context_value, function_id]);
    // The C runtime returns a context-owned source address with its immutable
    // trace mode in the low bit. No pointer into JIT data is retained.
    let descriptor = builder.inst_results(entered)[0];
    let source_region = builder.ins().band_imm_s(descriptor, -2);
    let source_traced = builder.ins().band_imm_s(descriptor, 1);

    let value_types: HashMap<_, _> = function
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .filter_map(|operation| operation.result.as_ref())
        .map(|value| (value.id, value.ty.clone()))
        .collect();
    let cleanup_blocks = function
        .blocks
        .iter()
        .filter(|block| {
            block
                .operations
                .iter()
                .any(|operation| matches!(operation.kind, OperationKind::LeaveTry { .. }))
        })
        .map(|block| block.id)
        .collect::<HashSet<_>>();
    let cleanup_scopes: HashMap<_, _> = function
        .blocks
        .iter()
        .filter(|block| cleanup_blocks.contains(&block.id))
        .map(|block| (block.range, block.id))
        .collect();
    let mut values = HashMap::new();
    for block in &function.blocks {
        let cl_block = blocks[&block.id];
        if block.id != function.entry {
            builder.switch_to_block(cl_block);
        }
        let begins_cleanup = cleanup_blocks.contains(&block.id);
        let is_cleanup = begins_cleanup
            || block
                .entry_scopes
                .iter()
                .any(|scope| cleanup_scopes.contains_key(scope));
        let cleanup_exit = block.operations.iter().rev().find_map(|operation| {
            if let OperationKind::ExitScope { scope, .. } = &operation.kind {
                cleanup_scopes.get(scope).copied()
            } else {
                None
            }
        });
        if begins_cleanup {
            let region = builder
                .ins()
                .iconst(types::I64, unwind.cleanup_ids[&block.range]);
            builder
                .ins()
                .call(runtime.begin_cleanup, &[context_value, region]);
        }
        lower_block_operations(
            builder,
            block,
            function,
            functions,
            managed_signatures,
            runtime,
            native_imports,
            context_value,
            source_region,
            source_traced,
            &blocks,
            &locals,
            &bound_locals,
            &mut values,
            &value_types,
            is_cleanup,
        )?;
        lower_terminator(
            builder,
            block,
            function,
            runtime,
            context_value,
            &blocks,
            &values,
            cleanup_exit,
        )?;
    }
    let unwind = runtime.unwind.as_ref().unwrap();
    builder.switch_to_block(blocks[&unwind.return_target]);
    let returned = builder.use_var(unwind.return_value);
    builder.ins().return_(&[returned]);
    let paths = unwind.paths.borrow().clone();
    builder.switch_to_block(unwind.dispatch);
    let route = builder.use_var(unwind.variable);
    let mut dispatch = cranelift_frontend::Switch::new();
    let route_blocks: Vec<_> = paths.iter().map(|_| builder.create_block()).collect();
    for (index, block) in route_blocks.iter().enumerate() {
        dispatch.set_entry(index as u128 + 1, *block);
    }
    dispatch.emit(builder, route, blocks[&function.failure]);
    for (path, route_block) in paths.iter().zip(route_blocks) {
        builder.switch_to_block(route_block);
        let next = builder.ins().iconst(types::I64, unwind.id(&path[1..]));
        builder.def_var(unwind.variable, next);
        let owner = builder.ins().iconst(types::I64, i64::from(path[0].0) + 1);
        builder.def_var(unwind.owner, owner);
        builder.ins().jump(blocks[&path[0]], &[]);
    }
    builder.seal_all_blocks();
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn lower_block_operations(
    builder: &mut FunctionBuilder<'_>,
    block: &BasicBlock,
    function: &IrFunction,
    functions: &HashMap<String, FunctionInfo>,
    managed_signatures: &HashMap<String, cranelift_codegen::ir::SigRef>,
    runtime: &RuntimeFunctionRefs,
    native_imports: &HashMap<String, u64>,
    context_value: Value,
    source_region: Value,
    source_traced: Value,
    blocks: &HashMap<BlockId, cranelift_codegen::ir::Block>,
    locals: &HashMap<String, Variable>,
    bound_locals: &HashMap<String, Variable>,
    values: &mut HashMap<ValueId, Value>,
    value_types: &HashMap<ValueId, rinzu_compiler::Type>,
    is_cleanup: bool,
) -> Result<()> {
    let unwind = runtime.unwind.as_ref().unwrap();
    *unwind.active_cleanup.borrow_mut() = block
        .entry_scopes
        .iter()
        .filter(|scope| unwind.cleanup_scopes.contains(scope))
        .copied()
        .collect();
    let mut cleanup_chain = Vec::new();
    for (operation_index, operation) in block.operations.iter().enumerate() {
        let owner_proven = runtime
            .proven_owner_operations
            .contains(&(block.id, operation_index));
        let source = operation.range;
        let identity = runtime
            .source_ids
            .get(&source.file_id.0)
            .copied()
            .unwrap_or(u64::from(source.file_id.0));
        ensure!(
            identity <= u64::from(u32::MAX) && source.start_byte <= source.end_byte,
            "invalid generated source position"
        );
        // These operations only select an SSA input or materialize bits:
        // no failure, allocation, callback or trace observation can occur.
        // An unwind annotation can add a runtime call below, so retain its
        // source update even for an otherwise pure operation.
        let records_source = operation.unwind.is_some()
            || !matches!(
                &operation.kind,
                OperationKind::Parameter { .. }
                    | OperationKind::Constant(
                        ConstantValue::Unit
                            | ConstantValue::Bool(_)
                            | ConstantValue::Int(_)
                            | ConstantValue::Float(_)
                    )
            );
        if records_source {
            let file = builder.ins().iconst(types::I64, identity as i64);
            let span = u64::from(source.start_byte) | (u64::from(source.end_byte) << 32);
            let span = builder.ins().iconst(types::I64, span as i64);
            if runtime.direct_source {
                // Stores and their source positions are unchanged. Trace
                // observation reads this same region only when enabled;
                // it does not repeat the file/span setup or source stores.
                let flags = cranelift_codegen::ir::MemFlagsData::new();
                builder.ins().store(flags, file, source_region, 0);
                builder.ins().store(flags, span, source_region, 8);
                let present = builder.ins().iconst(types::I32, 1);
                builder.ins().store(flags, present, source_region, 16);
                let traced = builder.create_block();
                let next = builder.create_block();
                builder.set_cold_block(traced);
                let observe = builder.ins().icmp_imm_s(IntCC::NotEqual, source_traced, 0);
                builder.ins().brif(observe, traced, &[], next, &[]);
                builder.switch_to_block(traced);
                builder.ins().call(runtime.observe_source, &[context_value]);
                builder.ins().jump(next, &[]);
                builder.switch_to_block(next);
            } else {
                builder
                    .ins()
                    .call(runtime.set_source, &[context_value, file, span]);
            }
        }
        match &operation.kind {
            OperationKind::EnterScope { scope } if unwind.cleanup_scopes.contains(scope) => {
                unwind.active_cleanup.borrow_mut().push(*scope)
            }
            OperationKind::ExitScope { scope, .. } if unwind.cleanup_scopes.contains(scope) => {
                unwind
                    .active_cleanup
                    .borrow_mut()
                    .retain(|active| active != scope)
            }
            _ => {}
        }
        if let Some(async_runtime) = &runtime.source_async
            && operation.unwind.is_some()
        {
            let future = if let OperationKind::Await { future, .. } = operation.kind {
                Some(value_of(values, future)?)
            } else {
                None
            };
            crate::task_error_origin::lower(
                builder,
                context_value,
                future,
                runtime.direct_source,
                async_runtime.task_error_origin,
            );
        }
        let result = match &operation.kind {
            OperationKind::Parameter { index } => {
                Some(builder.block_params(blocks[&function.entry])[*index as usize + 1])
            }
            OperationKind::Constant(value) => {
                let result = operation.result.as_ref().unwrap();
                let lowered = lower_constant(
                    builder,
                    value,
                    &result.ty,
                    result.id,
                    runtime,
                    context_value,
                )?;
                if matches!(value, ConstantValue::String(_) | ConstantValue::Bytes(_)) {
                    branch_after_runtime_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        function.failure,
                        &cleanup_chain,
                    );
                }
                Some(lowered)
            }
            OperationKind::DeclareLocal { name } => {
                let zero = builder.ins().iconst(types::I64, 0);
                builder.def_var(locals[name], zero);
                builder.def_var(bound_locals[name], zero);
                None
            }
            OperationKind::CheckLocal {
                name,
                error,
                cleanup_chain: operation_cleanup,
                ..
            } => {
                let bound = builder.use_var(bound_locals[name]);
                let opcode = builder.ins().iconst(types::I64, 11);
                let zero = builder.ins().iconst(types::I64, 0);
                builder.ins().call(
                    runtime.error_data,
                    &[context_value, opcode, zero, bound, zero, zero, zero],
                );
                branch_after_error_call(
                    builder,
                    runtime,
                    context_value,
                    blocks,
                    error,
                    function.failure,
                    operation_cleanup,
                )?;
                None
            }
            OperationKind::TakeTaskErrors { scope } => {
                let async_runtime = runtime.source_async.as_ref().unwrap();
                let identity = builder.ins().iconst(
                    types::I64,
                    ((u64::from(scope.start_byte) << 32) | u64::from(scope.end_byte)) as i64,
                );
                let ty = builder.ins().iconst(
                    types::I64,
                    i64::try_from(
                        runtime
                            .data_schema
                            .index(&Type::List(Box::new(Type::Error)))?,
                    )?,
                );
                let call = builder.ins().call(
                    async_runtime.take_task_errors,
                    &[context_value, identity, ty],
                );
                Some(builder.inst_results(call)[0])
            }
            OperationKind::LoadLocal { name } => Some(builder.use_var(locals[name])),
            OperationKind::StoreField {
                operand,
                identity,
                field,
                value,
            } => {
                let operand = value_of(values, *operand)?;
                let value = value_of(values, *value)?;
                let ty = runtime.data_schema.index(&Type::Named(identity.clone()))?;
                let ty = builder.ins().iconst(types::I64, i64::try_from(ty)?);
                let field = builder.ins().iconst(types::I64, i64::from(*field));
                builder.ins().call(
                    runtime.store_field,
                    &[context_value, operand, ty, field, value],
                );
                branch_after_runtime_call(
                    builder,
                    runtime,
                    context_value,
                    blocks,
                    function.failure,
                    &cleanup_chain,
                );
                None
            }
            OperationKind::EnumPayload { operand, .. }
                if runtime.virtual_bytes_options.contains(operand) =>
            {
                // Verification and the leaf def-use plan prove the matching
                // Some edge. The same managed payload remains a typed root.
                let value = value_of(values, *operand)?;
                branch_after_runtime_call(
                    builder,
                    runtime,
                    context_value,
                    blocks,
                    function.failure,
                    &cleanup_chain,
                );
                Some(value)
            }
            OperationKind::ListElement { operand, index } => {
                let ty = runtime.data_schema.index(&value_types[operand])?;
                let operand = value_of(values, *operand)?;
                let index = value_of(values, *index)?;
                let ty = builder.ins().iconst(types::I64, i64::try_from(ty)?);
                let variant = builder.ins().iconst(types::I64, -1);
                let call = builder.ins().call(
                    owner_helper(runtime, "project", runtime.project, owner_proven),
                    &[context_value, operand, ty, variant, index],
                );
                let result = builder.inst_results(call)[0];
                branch_after_runtime_call(
                    builder,
                    runtime,
                    context_value,
                    blocks,
                    function.failure,
                    &cleanup_chain,
                );
                Some(result)
            }
            OperationKind::Field {
                operand,
                identity,
                field,
                ..
            }
            | OperationKind::EnumPayload {
                operand,
                identity,
                field,
                ..
            } => {
                let variant = match &operation.kind {
                    OperationKind::EnumPayload { variant, .. } => i64::from(*variant),
                    _ => -1,
                };
                let ty = runtime.data_schema.index(&Type::Named(identity.clone()))?;
                let operand = value_of(values, *operand)?;
                let ty = builder.ins().iconst(types::I64, i64::try_from(ty)?);
                let variant = builder.ins().iconst(types::I64, variant);
                let field = builder.ins().iconst(types::I64, i64::from(*field));
                let call = builder.ins().call(
                    owner_helper(runtime, "project", runtime.project, owner_proven),
                    &[context_value, operand, ty, variant, field],
                );
                let result = builder.inst_results(call)[0];
                if let OperationKind::Field {
                    error: Some(error), ..
                } = &operation.kind
                {
                    branch_after_error_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        error,
                        function.failure,
                        &cleanup_chain,
                    )?;
                } else {
                    branch_after_runtime_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        function.failure,
                        &cleanup_chain,
                    );
                }
                Some(result)
            }
            OperationKind::StaticHandlerReference { target } => {
                let (_, function) = find_function(functions, target)?;
                Some(
                    builder
                        .ins()
                        .iconst(types::I64, i64::try_from(function.index)?),
                )
            }
            OperationKind::StoreLocal { name, value } => {
                builder.def_var(locals[name], value_of(values, *value)?);
                let bound = builder.ins().iconst(types::I64, 1);
                builder.def_var(bound_locals[name], bound);
                None
            }
            OperationKind::Unary {
                operator,
                operand,
                cleanup_chain: operation_cleanup,
                ..
            } => {
                let operand = value_of(values, *operand)?;
                match operator {
                    UnaryOperator::Not => {
                        let one = builder.ins().iconst(types::I64, 1);
                        Some(builder.ins().bxor(operand, one))
                    }
                    UnaryOperator::Negate => {
                        let code = builder.ins().iconst(types::I64, OP_NEGATE);
                        let zero = builder.ins().iconst(types::I64, 0);
                        let scalar = operation
                            .result
                            .as_ref()
                            .unwrap()
                            .ty
                            .scalar()
                            .ok_or_else(|| anyhow!("numeric negate has no scalar type"))?;
                        let kind = builder.ins().iconst(
                            types::I64,
                            i64::from(crate::scalar_backend::kind(scalar) as u32),
                        );
                        let call = builder.ins().call(
                            runtime.scalar_arithmetic,
                            &[context_value, kind, code, operand, zero],
                        );
                        let value = builder.inst_results(call)[0];
                        branch_after_runtime_call(
                            builder,
                            runtime,
                            context_value,
                            blocks,
                            function.failure,
                            operation_cleanup,
                        );
                        Some(value)
                    }
                }
            }
            OperationKind::Binary {
                operator,
                left,
                right,
                cleanup_chain: operation_cleanup,
                ..
            } => Some(lower_binary(
                builder,
                *operator,
                value_of(values, *left)?,
                value_of(values, *right)?,
                &value_types[left],
                runtime,
                context_value,
                blocks,
                function.failure,
                operation_cleanup,
            )),
            OperationKind::Spawn {
                target,
                arguments,
                cleanup_chain: operation_cleanup,
                ..
            } if runtime.source_async.is_some() => {
                let (_, function_info) = find_function(functions, target)?;
                ensure!(
                    function_info.parameter_count == arguments.len(),
                    "Worker argument count mismatch for {target}"
                );
                let arguments = arguments
                    .iter()
                    .map(|argument| value_of(values, *argument))
                    .collect::<Result<Vec<_>>>()?;
                let address = stack_array(builder, &arguments)?;
                let count = builder
                    .ins()
                    .iconst(types::I64, i64::try_from(arguments.len())?);
                let index = builder
                    .ins()
                    .iconst(types::I64, i64::try_from(function_info.index)?);
                let call = builder.ins().call(
                    runtime.source_async.as_ref().unwrap().spawn,
                    &[context_value, index, address, count],
                );
                let value = builder.inst_results(call)[0];
                branch_after_runtime_call(
                    builder,
                    runtime,
                    context_value,
                    blocks,
                    function.failure,
                    operation_cleanup,
                );
                Some(value)
            }
            OperationKind::ManagedCall {
                target,
                arguments,
                error,
                cleanup_chain: operation_cleanup,
                ..
            } if runtime.source_async.is_some() && find_function(functions, target)?.1.is_async => {
                let (_, function_info) = find_function(functions, target)?;
                ensure!(
                    function_info.parameter_count == arguments.len(),
                    "async call argument count mismatch for {target}"
                );
                let arguments = arguments
                    .iter()
                    .map(|argument| value_of(values, *argument))
                    .collect::<Result<Vec<_>>>()?;
                let address = stack_array(builder, &arguments)?;
                let count = builder
                    .ins()
                    .iconst(types::I64, i64::try_from(arguments.len())?);
                let index = builder
                    .ins()
                    .iconst(types::I64, i64::try_from(function_info.index)?);
                let Type::Future(inner) = &operation.result.as_ref().unwrap().ty else {
                    unreachable!()
                };
                let kind = builder
                    .ins()
                    .iconst(types::I64, async_transfer_kind(inner)?);
                let call = builder.ins().call(
                    runtime.source_async.as_ref().unwrap().call,
                    &[context_value, index, address, count, kind],
                );
                let value = builder.inst_results(call)[0];
                branch_after_error_call(
                    builder,
                    runtime,
                    context_value,
                    blocks,
                    error,
                    function.failure,
                    operation_cleanup,
                )?;
                Some(value)
            }
            OperationKind::ManagedCall {
                target,
                arguments,
                cleanup_chain: operation_cleanup,
                ..
            }
            | OperationKind::Spawn {
                target,
                arguments,
                cleanup_chain: operation_cleanup,
                ..
            } => {
                let (name, function_info) = find_function(functions, target)?;
                ensure!(
                    function_info.parameter_count == arguments.len(),
                    "managed call argument count mismatch for {target}"
                );
                let mut call_arguments = vec![context_value];
                call_arguments.extend(
                    arguments
                        .iter()
                        .map(|argument| value_of(values, *argument))
                        .collect::<Result<Vec<_>>>()?,
                );
                let function_index = builder
                    .ins()
                    .iconst(types::I64, i64::try_from(function_info.index)?);
                let resolve = builder
                    .ins()
                    .call(runtime.resolve_dispatch, &[context_value, function_index]);
                let address = builder.inst_results(resolve)[0];
                let call =
                    builder
                        .ins()
                        .call_indirect(managed_signatures[name], address, &call_arguments);
                let value = builder.inst_results(call)[0];
                builder.ins().call(runtime.leave_dispatch, &[context_value]);
                if let OperationKind::ManagedCall { error, .. } = &operation.kind {
                    branch_after_error_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        error,
                        function.failure,
                        operation_cleanup,
                    )?;
                }
                branch_after_runtime_call(
                    builder,
                    runtime,
                    context_value,
                    blocks,
                    function.failure,
                    operation_cleanup,
                );
                Some(value)
            }
            OperationKind::TaskControl { kind, future } => {
                let kind = builder.ins().iconst(types::I64, *kind as i64);
                let future = if let Some(id) = future {
                    value_of(values, *id)?
                } else {
                    builder.ins().iconst(types::I64, 0)
                };
                let call = builder
                    .ins()
                    .call(runtime.task_control, &[context_value, kind, future]);
                Some(builder.inst_results(call)[0])
            }
            OperationKind::DataOperation {
                kind,
                arguments,
                argument_types,
                error,
                ..
            } => {
                let argument_values = arguments
                    .iter()
                    .map(|id| value_of(values, *id))
                    .collect::<Result<Vec<_>>>()?;
                // The verified closed signatures make scalar lengths leaf
                // calls. Keep roots/source/error routing at this same IR site;
                // only the compiler-owned metadata arrays and dispatch go.
                let call = match kind {
                    rinzu_compiler::DataOperation::BytesSlice
                        if operation
                            .result
                            .as_ref()
                            .is_some_and(|v| runtime.virtual_bytes_options.contains(&v.id)) =>
                    {
                        let result = runtime
                            .data_schema
                            .index(&operation.result.as_ref().unwrap().ty)?;
                        let result = builder.ins().iconst(types::I64, i64::try_from(result)?);
                        builder.ins().call(
                            owner_helper(
                                runtime,
                                "bytes_slice_payload",
                                runtime.bytes_slice_payload.expect("C bytes-slice entry"),
                                owner_proven,
                            ),
                            &[
                                context_value,
                                result,
                                argument_values[0],
                                argument_values[1],
                                argument_values[2],
                            ],
                        )
                    }
                    rinzu_compiler::DataOperation::BytesLength => builder.ins().call(
                        owner_helper(
                            runtime,
                            "bytes_int_length",
                            runtime.bytes_int_length,
                            owner_proven,
                        ),
                        &[context_value, argument_values[0]],
                    ),
                    rinzu_compiler::DataOperation::StringLength => builder.ins().call(
                        owner_helper(
                            runtime,
                            "string_scalar_length",
                            runtime.string_scalar_length,
                            owner_proven,
                        ),
                        &[context_value, argument_values[0]],
                    ),
                    rinzu_compiler::DataOperation::ListLength
                    | rinzu_compiler::DataOperation::MapLength => {
                        let ty = runtime.data_schema.index(&argument_types[0])?;
                        let ty = builder.ins().iconst(types::I64, i64::try_from(ty)?);
                        builder.ins().call(
                            owner_helper(
                                runtime,
                                "collection_length",
                                runtime.collection_length,
                                owner_proven,
                            ),
                            &[context_value, argument_values[0], ty],
                        )
                    }
                    _ => {
                        let result = builder.ins().iconst(
                            types::I64,
                            i64::try_from(
                                runtime
                                    .data_schema
                                    .index(&operation.result.as_ref().unwrap().ty)?,
                            )?,
                        );
                        let op = builder.ins().iconst(types::I64, *kind as i64);
                        let argument_array = stack_array(builder, &argument_values)?;
                        let type_values = argument_types
                            .iter()
                            .map(|ty| {
                                Ok(builder.ins().iconst(
                                    types::I64,
                                    i64::try_from(runtime.data_schema.index(ty)?)?,
                                ))
                            })
                            .collect::<Result<Vec<_>>>()?;
                        let type_array = stack_array(builder, &type_values)?;
                        let count = builder.ins().iconst(types::I64, arguments.len() as i64);
                        builder.ins().call(
                            owner_helper(
                                runtime,
                                "data_operation",
                                runtime.data_operation,
                                owner_proven,
                            ),
                            &[context_value, op, result, type_array, argument_array, count],
                        )
                    }
                };
                let value = builder.inst_results(call)[0];
                branch_after_error_call(
                    builder,
                    runtime,
                    context_value,
                    blocks,
                    error,
                    function.failure,
                    &cleanup_chain,
                )?;
                Some(value)
            }
            OperationKind::Construct {
                arguments,
                variant,
                cleanup_chain: operation_cleanup,
                ..
            } => {
                let type_id = builder.ins().iconst(
                    types::I64,
                    i64::try_from(
                        runtime
                            .data_schema
                            .index(&operation.result.as_ref().unwrap().ty)?,
                    )?,
                );
                let fields = builder
                    .ins()
                    .iconst(types::I64, i64::try_from(arguments.len())?);
                let argument_values = arguments
                    .iter()
                    .map(|value| value_of(values, *value))
                    .collect::<Result<Vec<_>>>()?;
                let argument_array = stack_array(builder, &argument_values)?;
                let variant = builder
                    .ins()
                    .iconst(types::I64, variant.map(i64::from).unwrap_or(-1));
                let call = builder.ins().call(
                    owner_helper(runtime, "construct", runtime.construct, owner_proven),
                    &[context_value, type_id, variant, argument_array, fields],
                );
                let value = builder.inst_results(call)[0];
                branch_after_runtime_call(
                    builder,
                    runtime,
                    context_value,
                    blocks,
                    function.failure,
                    operation_cleanup,
                );
                Some(value)
            }
            OperationKind::CancellationPoll {
                cleanup_chain: operation_cleanup,
            } => {
                cleanup_chain = operation_cleanup.clone();
                None
            }
            OperationKind::GcSafepoint { stack_map, .. } => {
                let mut live = Vec::new();
                for root in &stack_map.roots {
                    if let StackRoot::Value { value, .. } = root
                        && runtime.dead_temporary_roots.contains(&(
                            block.id,
                            operation_index,
                            *value,
                        ))
                    {
                        continue;
                    }
                    if let StackRoot::Local { name, .. } = root
                        && runtime.dead_local_roots.contains(&(
                            block.id,
                            operation_index,
                            name.clone(),
                        ))
                    {
                        continue;
                    }
                    let root = match root {
                        StackRoot::Value { value, .. } => value_of(values, *value)?,
                        StackRoot::Local { name, .. } => builder.use_var(locals[name]),
                    };
                    builder.declare_value_needs_stack_map(root);
                    live.push(root);
                }
                let count = builder.ins().iconst(types::I64, i64::try_from(live.len())?);
                let status = if live.is_empty() {
                    // The compiler knows there is nothing to store. Keep the
                    // safepoint checks/GC without a prepare/commit round trip.
                    let empty = builder.ins().iconst(types::I64, 0);
                    let call = builder.ins().call(
                        owner_helper(
                            runtime,
                            "publish_roots_poll",
                            runtime.publish_roots,
                            owner_proven,
                        ),
                        &[context_value, empty, count],
                    );
                    builder.inst_results(call)[0]
                } else {
                    let prepared = builder
                        .ins()
                        .call(runtime.prepare_roots, &[context_value, count]);
                    let address = builder.inst_results(prepared)[0];
                    // The compiler owns this region. Only aligned root stores
                    // may appear between prepare and commit: no GC or reentry.
                    for (index, root) in live.iter().enumerate() {
                        builder.ins().store(
                            cranelift_codegen::ir::MemFlagsData::new(),
                            *root,
                            address,
                            i32::try_from(
                                index
                                    .checked_mul(8)
                                    .ok_or_else(|| anyhow!("managed root buffer is too large"))?,
                            )?,
                        );
                    }
                    let known = runtime
                        .proven_frame_memberships
                        .get(&(block.id, operation_index))
                        .copied()
                        .unwrap_or(0);
                    let all = if live.len() == 64 {
                        u64::MAX
                    } else if live.len() < 64 {
                        (1_u64 << live.len()) - 1
                    } else {
                        0
                    };
                    let (name, commit) = if runtime
                        .proven_frame_roots
                        .contains(&(block.id, operation_index))
                        || (all != 0 && known == all)
                    {
                        (
                            "commit_proven_roots_poll",
                            runtime
                                .commit_proven_roots
                                .expect("C membership proof entry"),
                        )
                    } else if known != 0 && all != 0 {
                        (
                            "commit_partial_roots_poll",
                            runtime
                                .commit_partial_roots
                                .expect("C partial membership proof entry"),
                        )
                    } else {
                        ("commit_roots_poll", runtime.commit_roots)
                    };
                    let mut arguments = vec![context_value, count];
                    if name == "commit_partial_roots_poll" {
                        arguments.push(builder.ins().iconst(types::I64, known as i64));
                    }
                    let call = builder.ins().call(
                        owner_helper(runtime, name, commit, owner_proven),
                        &arguments,
                    );
                    builder.inst_results(call)[0]
                };
                if !is_cleanup {
                    branch_after_runtime_status(
                        builder,
                        runtime,
                        context_value,
                        status,
                        blocks,
                        function.failure,
                        &cleanup_chain,
                    );
                }
                None
            }
            OperationKind::NativeCall {
                target,
                arguments,
                cleanup_chain: operation_cleanup,
                ..
            } if native_imports.contains_key(target) => {
                ensure!(
                    arguments.len() <= 8,
                    "native package calls accept at most eight scalar arguments"
                );
                let key = native_imports[target];
                let key = builder
                    .ins()
                    .iconst(types::I64, i64::from_ne_bytes(key.to_ne_bytes()));
                let count = builder
                    .ins()
                    .iconst(types::I64, i64::try_from(arguments.len())?);
                let zero = builder.ins().iconst(types::I64, 0);
                let mut native_arguments = vec![context_value, key, count];
                native_arguments.extend(
                    arguments
                        .iter()
                        .map(|argument| value_of(values, *argument))
                        .collect::<Result<Vec<_>>>()?,
                );
                native_arguments.resize(11, zero);
                let call = builder.ins().call(runtime.native_invoke, &native_arguments);
                let value = builder.inst_results(call)[0];
                branch_after_runtime_call(
                    builder,
                    runtime,
                    context_value,
                    blocks,
                    function.failure,
                    operation_cleanup,
                );
                Some(value)
            }
            OperationKind::FormatString { parts, error, .. } => {
                let mut words = Vec::with_capacity(parts.len());
                let mut indices = Vec::with_capacity(parts.len());
                for (value, ty) in parts {
                    words.push(value_of(values, *value)?);
                    let index = runtime.data_schema.index(ty)?;
                    indices.push(builder.ins().iconst(types::I64, i64::try_from(index)?));
                }
                let words = stack_array(builder, &words)?;
                let indices = stack_array(builder, &indices)?;
                let count = builder
                    .ins()
                    .iconst(types::I64, i64::try_from(parts.len())?);
                let call = builder.ins().call(
                    runtime.format_string,
                    &[context_value, words, indices, count],
                );
                let result = builder.inst_results(call)[0];
                if let Some(error) = error {
                    branch_after_error_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        error,
                        function.failure,
                        &cleanup_chain,
                    )?;
                } else {
                    branch_after_runtime_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        function.failure,
                        &cleanup_chain,
                    );
                }
                Some(result)
            }
            OperationKind::Stdout {
                operand,
                newline,
                error,
                cleanup_chain: operation_cleanup,
                ..
            } => {
                let ty = runtime.data_schema.index(&value_types[operand])?;
                let ty = builder.ins().iconst(types::I64, i64::try_from(ty)?);
                let operand = value_of(values, *operand)?;
                let newline = builder.ins().iconst(types::I64, i64::from(*newline));
                let call = builder
                    .ins()
                    .call(runtime.stdout, &[context_value, operand, ty, newline]);
                let value = builder.inst_results(call)[0];
                branch_after_error_call(
                    builder,
                    runtime,
                    context_value,
                    blocks,
                    error,
                    function.failure,
                    operation_cleanup,
                )?;
                Some(value)
            }
            OperationKind::EnterErrorGroup { .. }
            | OperationKind::FinishErrorGroup { .. }
            | OperationKind::RecordTaskError { .. }
            | OperationKind::ThrowError { .. }
            | OperationKind::RethrowCaught { .. }
            | OperationKind::CatchErrors { .. }
            | OperationKind::ErrorIs { .. }
            | OperationKind::ErrorCast { .. }
            | OperationKind::ErrorField { .. }
            | OperationKind::ErrorMatch { .. }
            | OperationKind::ErrorIndex { .. }
            | OperationKind::ErrorRest { .. } => {
                if matches!(
                    operation.kind,
                    OperationKind::CatchErrors { .. } | OperationKind::RecordTaskError { .. }
                ) && let Some((route, owner)) = unwind.saved_routes.borrow().get(&block.id)
                {
                    let route = builder.use_var(*route);
                    let owner = builder.use_var(*owner);
                    builder.def_var(unwind.variable, route);
                    builder.def_var(unwind.owner, owner);
                }
                let (opcode, operand, argument, expected) = match &operation.kind {
                    OperationKind::EnterErrorGroup { scope } => (
                        8,
                        None,
                        ((u64::from(scope.start_byte) << 32) | u64::from(scope.end_byte)) as i64,
                        Vec::new(),
                    ),
                    OperationKind::RecordTaskError { scope } => (
                        9,
                        None,
                        ((u64::from(scope.start_byte) << 32) | u64::from(scope.end_byte)) as i64,
                        Vec::new(),
                    ),
                    OperationKind::FinishErrorGroup { scope, .. } => (
                        10,
                        None,
                        ((u64::from(scope.start_byte) << 32) | u64::from(scope.end_byte)) as i64,
                        Vec::new(),
                    ),
                    OperationKind::ErrorIs { operand, target } => (
                        7,
                        Some(*operand),
                        i64::try_from(runtime.data_schema.index(target)?)?,
                        Vec::new(),
                    ),
                    OperationKind::ErrorCast { operand } => (0, Some(*operand), 0, Vec::new()),
                    OperationKind::ErrorField { error, field } => {
                        (1, Some(*error), i64::from(*field), Vec::new())
                    }
                    OperationKind::ErrorMatch {
                        errors,
                        types,
                        rest,
                    } => (
                        2,
                        Some(*errors),
                        i64::from(*rest),
                        types
                            .iter()
                            .map(|ty| runtime.data_schema.index(ty))
                            .collect::<Result<Vec<_>>>()?,
                    ),
                    OperationKind::ErrorIndex { errors, index } => {
                        (3, Some(*errors), i64::from(*index), Vec::new())
                    }
                    OperationKind::ErrorRest { errors, start } => {
                        (4, Some(*errors), i64::from(*start), Vec::new())
                    }
                    OperationKind::CatchErrors { preserve, .. } => {
                        (5, None, i64::from(*preserve), Vec::new())
                    }
                    OperationKind::RethrowCaught {
                        errors,
                        elements,
                        tail,
                        ..
                    } => (
                        12,
                        Some(*errors),
                        tail.map_or(-1, i64::from),
                        elements.iter().map(|index| *index as usize).collect(),
                    ),
                    OperationKind::ThrowError { value, .. } => (6, Some(*value), 0, Vec::new()),
                    _ => unreachable!(),
                };
                let ty = runtime.data_schema.index(
                    operation
                        .result
                        .as_ref()
                        .map_or(&Type::Error, |result| &result.ty),
                )?;
                let type_id = builder.ins().iconst(types::I64, i64::try_from(ty)?);
                let value = if let OperationKind::FinishErrorGroup {
                    handles, preserve, ..
                } = operation.kind
                {
                    builder
                        .ins()
                        .iconst(types::I64, i64::from(handles) | (i64::from(preserve) << 1))
                } else if let Some(operand) = operand {
                    value_of(values, operand)?
                } else {
                    builder.ins().iconst(types::I64, 0)
                };
                let argument = builder.ins().iconst(types::I64, argument);
                let length = builder
                    .ins()
                    .iconst(types::I64, i64::try_from(expected.len())?);
                let expected = expected
                    .iter()
                    .map(|ty| Ok(builder.ins().iconst(types::I64, i64::try_from(*ty)?)))
                    .collect::<Result<Vec<_>>>()?;
                let array = stack_array(builder, &expected)?;
                let opcode = builder.ins().iconst(types::I64, opcode);
                let call = builder.ins().call(
                    runtime.error_data,
                    &[
                        context_value,
                        opcode,
                        type_id,
                        value,
                        argument,
                        array,
                        length,
                    ],
                );
                let value = builder.inst_results(call)[0];
                if let OperationKind::ThrowError {
                    error,
                    cleanup_chain,
                    ..
                }
                | OperationKind::RethrowCaught {
                    error,
                    cleanup_chain,
                    ..
                } = &operation.kind
                {
                    branch_after_error_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        error,
                        function.failure,
                        cleanup_chain,
                    )?;
                } else {
                    branch_after_runtime_call(
                        builder,
                        runtime,
                        context_value,
                        blocks,
                        function.failure,
                        &cleanup_chain,
                    );
                }
                operation.result.as_ref().map(|_| value)
            }
            OperationKind::ErrorMessage { error } => {
                let error = value_of(values, *error)?;
                let call = builder
                    .ins()
                    .call(runtime.error_message, &[context_value, error]);
                let value = builder.inst_results(call)[0];
                branch_after_runtime_call(
                    builder,
                    runtime,
                    context_value,
                    blocks,
                    function.failure,
                    &cleanup_chain,
                );
                Some(value)
            }
            OperationKind::CoreLength {
                operand,
                kind: rinzu_compiler::CoreLengthKind::Utf8,
            } => {
                let operand = value_of(values, *operand)?;
                let call = builder
                    .ins()
                    .call(runtime.utf8_length, &[context_value, operand]);
                let value = builder.inst_results(call)[0];
                branch_after_runtime_call(
                    builder,
                    runtime,
                    context_value,
                    blocks,
                    function.failure,
                    &cleanup_chain,
                );
                Some(value)
            }
            OperationKind::CoreLength {
                operand,
                kind: rinzu_compiler::CoreLengthKind::Bytes,
            } => {
                let expected = match value_types[operand] {
                    Type::FixedBytes(n) => n,
                    Type::Bytes => u64::MAX,
                    _ => unreachable!("verified byte length"),
                };
                let operand = value_of(values, *operand)?;
                let expected = builder.ins().iconst(types::I64, expected as i64);
                let call = builder
                    .ins()
                    .call(runtime.bytes_length, &[context_value, operand, expected]);
                let result = builder.inst_results(call)[0];
                branch_after_runtime_call(
                    builder,
                    runtime,
                    context_value,
                    blocks,
                    function.failure,
                    &cleanup_chain,
                );
                Some(result)
            }
            OperationKind::BytesPad { operand, from, to } => {
                let operand = value_of(values, *operand)?;
                let from = builder.ins().iconst(types::I64, *from as i64);
                let to = builder.ins().iconst(types::I64, *to as i64);
                let call = builder
                    .ins()
                    .call(runtime.bytes_pad, &[context_value, operand, from, to]);
                let result = builder.inst_results(call)[0];
                branch_after_runtime_call(
                    builder,
                    runtime,
                    context_value,
                    blocks,
                    function.failure,
                    &cleanup_chain,
                );
                Some(result)
            }
            OperationKind::EnterScope { .. } | OperationKind::ExitScope { .. } => None,
            OperationKind::PrepareNativeCall {
                call,
                arguments,
                error,
                ..
            } => {
                let plan = runtime.native_plans.index(runtime.function_index, *call)?;
                let plan = builder.ins().iconst(types::I64, i64::try_from(plan)?);
                let arguments = arguments
                    .iter()
                    .map(|value| value_of(values, *value))
                    .collect::<Result<Vec<_>>>()?;
                let arguments = stack_array(builder, &arguments)?;
                builder.ins().call(
                    runtime.native_helpers["prepare"],
                    &[context_value, plan, arguments],
                );
                branch_after_error_call(
                    builder,
                    runtime,
                    context_value,
                    blocks,
                    error,
                    function.failure,
                    &cleanup_chain,
                )?;
                None
            }
            OperationKind::SourceNativeCall {
                call: Some(call),
                signature,
                ..
            } => {
                let index = runtime.native_plans.index(runtime.function_index, *call)?;
                ensure!(
                    runtime.native_plans.records[index].signature == *signature,
                    "physical native call changed its prepared signature"
                );
                let plan = builder.ins().iconst(types::I64, i64::try_from(index)?);
                let call = builder
                    .ins()
                    .call(runtime.native_helpers["address"], &[context_value, plan]);
                let address = builder.inst_results(call)[0];
                let call = builder
                    .ins()
                    .call(runtime.native_helpers["words"], &[context_value, plan]);
                let words = builder.inst_results(call)[0];
                let call = builder
                    .ins()
                    .call(runtime.native_stubs[&signature.name], &[address, words]);
                let result = builder.inst_results(call)[0];
                builder.ins().call(
                    runtime.native_helpers["returned"],
                    &[context_value, plan, result],
                );
                None
            }
            OperationKind::CloseNativeCall { call }
            | OperationKind::SettleNativeAdmissions { call, .. }
            | OperationKind::ReleaseNativeCall { call } => {
                let plan = runtime.native_plans.index(runtime.function_index, *call)?;
                let plan = builder.ins().iconst(types::I64, i64::try_from(plan)?);
                let helper = match operation.kind {
                    OperationKind::CloseNativeCall { .. } => "close",
                    OperationKind::SettleNativeAdmissions { .. } => "settle",
                    _ => "release",
                };
                builder
                    .ins()
                    .call(runtime.native_helpers[helper], &[context_value, plan]);
                None
            }
            OperationKind::ReadNativeResult { call, error, .. } => {
                let plan = runtime.native_plans.index(runtime.function_index, *call)?;
                let plan = builder.ins().iconst(types::I64, i64::try_from(plan)?);
                let call = builder
                    .ins()
                    .call(runtime.native_helpers["read"], &[context_value, plan]);
                let result = builder.inst_results(call)[0];
                branch_after_error_call(
                    builder,
                    runtime,
                    context_value,
                    blocks,
                    error,
                    function.failure,
                    &cleanup_chain,
                )?;
                Some(result)
            }
            OperationKind::CatchError { .. } => {
                if let Some((route, owner)) = unwind.saved_routes.borrow().get(&block.id) {
                    let route = builder.use_var(*route);
                    let owner = builder.use_var(*owner);
                    builder.def_var(unwind.variable, route);
                    builder.def_var(unwind.owner, owner);
                }
                let call = builder.ins().call(runtime.catch_error, &[context_value]);
                let value = builder.inst_results(call)[0];
                branch_after_runtime_call(
                    builder,
                    runtime,
                    context_value,
                    blocks,
                    function.failure,
                    &cleanup_chain,
                );
                Some(value)
            }
            OperationKind::ReserveSupplant {
                reservation,
                callee,
                arguments,
                ..
            } => {
                let async_runtime = runtime
                    .source_async
                    .as_ref()
                    .ok_or_else(|| anyhow!("supplant requires Main continuations"))?;
                let plans = runtime
                    .native_plans
                    .supplant(runtime.function_index, *reservation)?;
                let mut plan = builder.ins().iconst(types::I64, -1);
                if let Some(callee) = callee {
                    let selected = value_of(values, *callee)?;
                    for index in plans {
                        let target = runtime
                            .reference_targets
                            .identity(&runtime.native_plans.records[*index].signature.name)?;
                        let matches = builder.ins().icmp_imm_s(IntCC::Equal, selected, target);
                        let candidate = builder.ins().iconst(types::I64, i64::try_from(*index)?);
                        plan = builder.ins().select(matches, candidate, plan);
                    }
                } else {
                    ensure!(plans.len() == 1, "direct supplant requires a unique plan");
                    plan = builder.ins().iconst(types::I64, i64::try_from(plans[0])?);
                }
                let arguments = arguments
                    .iter()
                    .map(|value| value_of(values, *value))
                    .collect::<Result<Vec<_>>>()?;
                let arguments = stack_array(builder, &arguments)?;
                let call = builder.ins().call(
                    async_runtime.supplant_reserve,
                    &[context_value, plan, arguments],
                );
                let handle = builder.inst_results(call)[0];
                ensure!(
                    runtime
                        .supplant_values
                        .borrow_mut()
                        .insert(*reservation, handle)
                        .is_none(),
                    "duplicate supplant SSA reservation"
                );
                None
            }
            OperationKind::StartSupplant { reservation, .. } => {
                let handle = runtime.supplant_values.borrow()[reservation];
                builder.ins().call(
                    runtime.source_async.as_ref().unwrap().supplant_start,
                    &[context_value, handle],
                );
                None
            }
            OperationKind::SettleSupplant {
                reservation, error, ..
            } => {
                let handle = runtime.supplant_values.borrow()[reservation];
                let call = builder.ins().call(
                    runtime.source_async.as_ref().unwrap().supplant_settle,
                    &[context_value, handle],
                );
                let result = builder.inst_results(call)[0];
                branch_after_error_call(
                    builder,
                    runtime,
                    context_value,
                    blocks,
                    error,
                    function.failure,
                    &cleanup_chain,
                )?;
                Some(result)
            }
            OperationKind::FunctionReference { target } => {
                ensure!(
                    crate::function_reference::core_length(target).is_some()
                        || functions.contains_key(target)
                        || runtime.native_stubs.contains_key(target),
                    "function reference has no managed/core/validated native target"
                );
                Some(
                    builder
                        .ins()
                        .iconst(types::I64, runtime.reference_targets.identity(target)?),
                )
            }
            OperationKind::IndirectCall { .. } => Some(lower_reference_call(
                builder,
                operation,
                values,
                functions,
                managed_signatures,
                runtime,
                context_value,
                blocks,
                function,
            )?),
            OperationKind::SourceNativeCall {
                call: None,
                signature,
                arguments,
                error,
                cleanup_chain,
                ..
            } if runtime.source_async.is_some() => {
                ensure!(arguments.len() == 1, "closed source service argument count");
                let rinzu_compiler::ast::NativeBinding::Runtime(runtime_function) =
                    &signature.binding
                else {
                    bail!("async source native call has no closed service");
                };
                let service_id = match runtime_function {
                    rinzu_compiler::RuntimeFunction::ReadResourceSize => 1,
                    rinzu_compiler::RuntimeFunction::Sleep => 2,
                    rinzu_compiler::RuntimeFunction::NativeCompleted => 3,
                    rinzu_compiler::RuntimeFunction::NativeTransferred => 4,
                };
                let service = builder.ins().iconst(types::I64, service_id);
                let argument = value_of(values, arguments[0])?;
                let call = builder.ins().call(
                    runtime.source_async.as_ref().unwrap().service,
                    &[context_value, service, argument],
                );
                let value = builder.inst_results(call)[0];
                branch_after_error_call(
                    builder,
                    runtime,
                    context_value,
                    blocks,
                    error,
                    function.failure,
                    cleanup_chain,
                )?;
                Some(value)
            }
            OperationKind::SourceNativeCall { .. } => {
                bail!("source native execution requires R4 physical ABI backend")
            }
            OperationKind::NumericConvert {
                operand,
                source,
                error,
                cleanup_chain,
                ..
            } => {
                let source = builder.ins().iconst(
                    types::I64,
                    i64::from(crate::scalar_backend::kind(source.scalar().unwrap()) as u32),
                );
                let target = builder.ins().iconst(
                    types::I64,
                    i64::from(crate::scalar_backend::kind(
                        operation.result.as_ref().unwrap().ty.scalar().unwrap(),
                    ) as u32),
                );
                let operand = value_of(values, *operand)?;
                let call = builder.ins().call(
                    runtime.scalar_convert,
                    &[context_value, source, target, operand],
                );
                let value = builder.inst_results(call)[0];
                branch_after_error_call(
                    builder,
                    runtime,
                    context_value,
                    blocks,
                    error,
                    function.failure,
                    cleanup_chain,
                )?;
                Some(value)
            }
            OperationKind::NativeCall { target, .. } => {
                bail!("native call lowering is unavailable for {target}")
            }
            OperationKind::Await {
                future,
                consume,
                error,
                cleanup_chain: operation_cleanup,
                ..
            } if runtime.source_async.is_some() => {
                let future = value_of(values, *future)?;
                let consume = builder.ins().iconst(
                    types::I64,
                    i64::from(
                        *consume
                            || runtime
                                .fresh_return_consuming_awaits
                                .contains(&(block.id, operation_index)),
                    ),
                );
                let call = builder.ins().call(
                    runtime.source_async.as_ref().unwrap().await_value,
                    &[context_value, future, consume],
                );
                let value = builder.inst_results(call)[0];
                branch_after_error_call(
                    builder,
                    runtime,
                    context_value,
                    blocks,
                    error,
                    function.failure,
                    operation_cleanup,
                )?;
                branch_after_runtime_call(
                    builder,
                    runtime,
                    context_value,
                    blocks,
                    function.failure,
                    operation_cleanup,
                );
                Some(value)
            }
            OperationKind::Await { future, .. } => Some(value_of(values, *future)?),
            OperationKind::ErrorGroupOutcome {
                scope,
                error,
                cleanup_chain: operation_cleanup,
                ..
            }
            | OperationKind::TaskScopeOutcome {
                scope,
                error,
                cleanup_chain: operation_cleanup,
                ..
            } => {
                end_cleanup(builder, runtime, context_value, *scope);
                branch_after_error_call(
                    builder,
                    runtime,
                    context_value,
                    blocks,
                    error,
                    function.failure,
                    operation_cleanup,
                )?;
                branch_after_runtime_call(
                    builder,
                    runtime,
                    context_value,
                    blocks,
                    function.failure,
                    operation_cleanup,
                );
                None
            }
            OperationKind::EnterTaskScope { scope, .. }
            | OperationKind::JoinTaskScope { scope, .. } => {
                let runtime = runtime
                    .source_async
                    .as_ref()
                    .ok_or_else(|| anyhow!("task scopes require the async runtime"))?;
                let identity = builder.ins().iconst(
                    types::I64,
                    ((u64::from(scope.start_byte) << 32) | u64::from(scope.end_byte)) as i64,
                );
                let phase = builder.ins().iconst(
                    types::I64,
                    match operation.kind {
                        OperationKind::EnterTaskScope { all: true, .. } => 3,
                        OperationKind::EnterTaskScope { .. } => 0,
                        OperationKind::JoinTaskScope { cleanup: false, .. } => 1,
                        _ => 2,
                    },
                );
                builder
                    .ins()
                    .call(runtime.task_scope, &[context_value, identity, phase]);
                None
            }
            OperationKind::AsyncStart
            | OperationKind::EnterTry { .. }
            | OperationKind::LeaveTry { .. } => None,
        };
        if let (Some(result), Some(value)) = (&operation.result, result) {
            values.insert(result.id, value);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn lower_terminator(
    builder: &mut FunctionBuilder<'_>,
    block: &BasicBlock,
    function: &IrFunction,
    runtime: &RuntimeFunctionRefs,
    context_value: Value,
    blocks: &HashMap<BlockId, cranelift_codegen::ir::Block>,
    values: &HashMap<ValueId, Value>,
    cleanup_exit: Option<BlockId>,
) -> Result<()> {
    match &block.terminator {
        Terminator::Goto { target, .. } if cleanup_exit.is_some() => {
            let unwind = runtime.unwind.as_ref().unwrap();
            let scope = function
                .blocks
                .iter()
                .find(|block| block.id == cleanup_exit.unwrap())
                .unwrap()
                .range;
            end_cleanup(builder, runtime, context_value, scope);
            let owner = builder.use_var(unwind.owner);
            let pending = builder.ins().icmp_imm_s(
                IntCC::Equal,
                owner,
                i64::from(cleanup_exit.unwrap().0) + 1,
            );
            let pending_here =
                builder
                    .ins()
                    .icmp_imm_s(IntCC::Equal, owner, i64::from(block.id.0) + 1);
            let pending = builder.ins().bor(pending, pending_here);
            let normal = builder.create_block();
            builder
                .ins()
                .brif(pending, unwind.dispatch, &[], normal, &[]);
            builder.switch_to_block(normal);
            builder.seal_block(normal);
            let failed = failure_status(builder, runtime, context_value);
            builder
                .ins()
                .brif(failed, blocks[&function.failure], &[], blocks[target], &[]);
        }
        Terminator::Goto {
            target,
            cleanup_chain,
            ..
        } => {
            if cleanup_chain.is_empty() {
                builder.ins().jump(blocks[target], &[]);
            } else {
                jump_unwind(builder, runtime, blocks, cleanup_chain, *target);
            }
        }
        Terminator::Branch {
            condition,
            then_target,
            else_target,
        } => {
            builder.ins().brif(
                value_of(values, *condition)?,
                blocks[then_target],
                &[],
                blocks[else_target],
                &[],
            );
        }
        Terminator::Return {
            value,
            cleanup_chain,
            ..
        } => {
            let value = value
                .map(|value| value_of(values, value))
                .transpose()?
                .unwrap_or_else(|| builder.ins().iconst(types::I64, 0));

            if cleanup_chain.is_empty() {
                builder.ins().return_(&[value]);
            } else {
                let unwind = runtime.unwind.as_ref().unwrap();
                builder.def_var(unwind.return_value, value);
                if runtime
                    .data_schema
                    .is_reference(runtime.data_schema.index(&function.result_type)?)
                    || matches!(function.result_type, Type::Future(_))
                {
                    builder
                        .ins()
                        .call(runtime.retain_return, &[context_value, value]);
                }
                jump_unwind(
                    builder,
                    runtime,
                    blocks,
                    cleanup_chain,
                    unwind.return_target,
                );
            }
        }
        Terminator::Fail | Terminator::Unreachable => {
            let zero = builder.ins().iconst(types::I64, 0);
            builder.ins().return_(&[zero]);
        }
        Terminator::Switch {
            value,
            identity,
            cases,
        } => {
            let bits = value_of(values, *value)?;
            let variant = if runtime.virtual_bytes_options.contains(value) {
                let present = builder.ins().icmp_imm_s(IntCC::NotEqual, bits, 0);
                builder.ins().uextend(types::I64, present)
            } else {
                let ty = runtime.data_schema.index(&Type::Named(identity.clone()))?;
                let ty = builder.ins().iconst(types::I64, i64::try_from(ty)?);
                let call = builder
                    .ins()
                    .call(runtime.enum_variant, &[context_value, bits, ty]);
                builder.inst_results(call)[0]
            };
            let mut switch = Switch::new();
            for (tag, target) in cases {
                switch.set_entry(u128::from(*tag), blocks[target]);
            }
            switch.emit(builder, variant, blocks[&function.failure]);
        }
        Terminator::Pending => bail!("unverified pending terminator"),
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn lower_reference_call(
    builder: &mut FunctionBuilder<'_>,
    operation: &rinzu_compiler::Operation,
    values: &HashMap<ValueId, Value>,
    functions: &HashMap<String, FunctionInfo>,
    managed_signatures: &HashMap<String, cranelift_codegen::ir::SigRef>,
    runtime: &RuntimeFunctionRefs,
    context_value: Value,
    blocks: &HashMap<BlockId, cranelift_codegen::ir::Block>,
    function: &IrFunction,
) -> Result<Value> {
    let OperationKind::IndirectCall {
        callee,
        targets,
        arguments,
        error,
        cleanup_chain: operation_cleanup,
        ..
    } = &operation.kind
    else {
        unreachable!()
    };
    let callee = value_of(values, *callee)?;
    let arguments = arguments
        .iter()
        .map(|argument| value_of(values, *argument))
        .collect::<Result<Vec<_>>>()?;
    let result_block = builder.create_block();
    builder.append_block_param(result_block, types::I64);
    let invalid = builder.create_block();
    let target_blocks = targets
        .iter()
        .map(|_| builder.create_block())
        .collect::<Vec<_>>();
    let mut switch = Switch::new();
    for (target, block) in targets.iter().zip(&target_blocks) {
        switch.set_entry(runtime.reference_targets.identity(target)? as u128, *block);
    }
    switch.emit(builder, callee, invalid);
    builder.switch_to_block(invalid);
    builder
        .ins()
        .trap(cranelift_codegen::ir::TrapCode::unwrap_user(1));
    for (target, target_block) in targets.iter().zip(target_blocks) {
        builder.switch_to_block(target_block);
        let value = if let Some((kind, length)) = crate::function_reference::core_length(target) {
            ensure!(
                arguments.len() == 1,
                "length reference argument count mismatch"
            );
            let mut args = vec![context_value, arguments[0]];
            if let Some(length) = length {
                args.push(builder.ins().iconst(types::I64, length as i64));
            }
            let entry = if kind == rinzu_compiler::CoreLengthKind::Utf8 {
                runtime.utf8_length
            } else {
                runtime.bytes_length
            };
            let call = builder.ins().call(entry, &args);
            builder.inst_results(call)[0]
        } else {
            let (name, info) = find_function(functions, target)?;
            ensure!(
                info.parameter_count == arguments.len(),
                "indirect call argument count mismatch"
            );
            let index = builder.ins().iconst(types::I64, i64::try_from(info.index)?);
            if info.is_async
                && let Some(source_async) = &runtime.source_async
            {
                let address = stack_array(builder, &arguments)?;
                let count = builder
                    .ins()
                    .iconst(types::I64, i64::try_from(arguments.len())?);
                let Type::Future(inner) = &operation.result.as_ref().unwrap().ty else {
                    unreachable!()
                };
                let kind = builder
                    .ins()
                    .iconst(types::I64, async_transfer_kind(inner)?);
                let call = builder.ins().call(
                    source_async.call,
                    &[context_value, index, address, count, kind],
                );
                builder.inst_results(call)[0]
            } else {
                ensure!(
                    !info.is_async,
                    "async references require source continuation backend"
                );
                let resolve = builder
                    .ins()
                    .call(runtime.resolve_dispatch, &[context_value, index]);
                let address = builder.inst_results(resolve)[0];
                let mut args = vec![context_value];
                args.extend_from_slice(&arguments);
                let call = builder
                    .ins()
                    .call_indirect(managed_signatures[name], address, &args);
                let value = builder.inst_results(call)[0];
                builder.ins().call(runtime.leave_dispatch, &[context_value]);
                value
            }
        };
        branch_after_error_call(
            builder,
            runtime,
            context_value,
            blocks,
            error,
            function.failure,
            operation_cleanup,
        )?;
        branch_after_runtime_call(
            builder,
            runtime,
            context_value,
            blocks,
            function.failure,
            operation_cleanup,
        );
        builder.ins().jump(result_block, &[value.into()]);
    }
    builder.switch_to_block(result_block);
    Ok(builder.block_params(result_block)[0])
}

fn lower_constant(
    builder: &mut FunctionBuilder<'_>,
    value: &ConstantValue,
    ty: &rinzu_compiler::Type,
    id: ValueId,
    runtime: &RuntimeFunctionRefs,
    context_value: Value,
) -> Result<Value> {
    match value {
        ConstantValue::Unit => Ok(builder.ins().iconst(types::I64, 0)),
        ConstantValue::Bool(value) => Ok(builder.ins().iconst(types::I64, i64::from(*value))),
        ConstantValue::Int(value) => Ok(builder.ins().iconst(types::I64, *value)),
        ConstantValue::Float(value) => {
            let bits = if ty.scalar() == Some(rinzu_compiler::AbiScalar::F32) {
                u64::from((*value as f32).to_bits())
            } else {
                value.to_bits()
            };
            Ok(builder.ins().iconst(types::I64, bits as i64))
        }
        ConstantValue::String(text) => {
            let address = match runtime.string_literals[&id] {
                None => builder.ins().iconst(types::I64, 0),
                Some(global) => builder.ins().symbol_value(types::I64, global),
            };
            let length = builder.ins().iconst(types::I64, i64::try_from(text.len())?);
            let call = builder
                .ins()
                .call(runtime.string, &[context_value, address, length]);
            Ok(builder.inst_results(call)[0])
        }
        ConstantValue::Bytes(bytes) => {
            let address = match runtime.string_literals[&id] {
                None => builder.ins().iconst(types::I64, 0),
                Some(global) => builder.ins().symbol_value(types::I64, global),
            };
            let length = builder
                .ins()
                .iconst(types::I64, i64::try_from(bytes.len())?);
            let call = builder
                .ins()
                .call(runtime.bytes, &[context_value, address, length]);
            Ok(builder.inst_results(call)[0])
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn lower_binary(
    builder: &mut FunctionBuilder<'_>,
    operator: BinaryOperator,
    left: Value,
    right: Value,
    operand_type: &rinzu_compiler::Type,
    runtime: &RuntimeFunctionRefs,
    context_value: Value,
    blocks: &HashMap<BlockId, cranelift_codegen::ir::Block>,
    failure: BlockId,
    cleanup_chain: &[BlockId],
) -> Value {
    if *operand_type == rinzu_compiler::Type::String {
        let code = match operator {
            BinaryOperator::Equal => 0,
            BinaryOperator::NotEqual => 1,
            _ => unreachable!("verified String equality operation"),
        };
        let code = builder.ins().iconst(types::I64, code);
        let call = builder
            .ins()
            .call(runtime.string_compare, &[context_value, code, left, right]);
        let value = builder.inst_results(call)[0];
        branch_after_runtime_call(
            builder,
            runtime,
            context_value,
            blocks,
            failure,
            cleanup_chain,
        );
        return value;
    }
    if let Some(scalar) = operand_type
        .scalar()
        .filter(|_| !matches!(operator, BinaryOperator::Or | BinaryOperator::And))
    {
        let (code, compare) = match operator {
            BinaryOperator::Equal => (0, true),
            BinaryOperator::NotEqual => (1, true),
            BinaryOperator::Less => (2, true),
            BinaryOperator::LessEqual => (3, true),
            BinaryOperator::Greater => (4, true),
            BinaryOperator::GreaterEqual => (5, true),
            BinaryOperator::Add => (OP_ADD, false),
            BinaryOperator::Subtract => (OP_SUBTRACT, false),
            BinaryOperator::Multiply => (OP_MULTIPLY, false),
            BinaryOperator::Divide => (OP_DIVIDE, false),
            BinaryOperator::Remainder => (OP_REMAINDER, false),
            _ => unreachable!(),
        };
        if compare
            && (operand_type.is_integer()
                || (scalar == rinzu_compiler::AbiScalar::Bool && code < 2))
        {
            // Typed IR values use canonical sign/zero-extended bits. Native
            // ingress, conversions and field reads retain their validation.
            let signed = matches!(
                scalar,
                rinzu_compiler::AbiScalar::I8
                    | rinzu_compiler::AbiScalar::I16
                    | rinzu_compiler::AbiScalar::I32
                    | rinzu_compiler::AbiScalar::I64
            );
            let condition = match operator {
                BinaryOperator::Equal => IntCC::Equal,
                BinaryOperator::NotEqual => IntCC::NotEqual,
                BinaryOperator::Less if signed => IntCC::SignedLessThan,
                BinaryOperator::LessEqual if signed => IntCC::SignedLessThanOrEqual,
                BinaryOperator::Greater if signed => IntCC::SignedGreaterThan,
                BinaryOperator::GreaterEqual if signed => IntCC::SignedGreaterThanOrEqual,
                BinaryOperator::Less => IntCC::UnsignedLessThan,
                BinaryOperator::LessEqual => IntCC::UnsignedLessThanOrEqual,
                BinaryOperator::Greater => IntCC::UnsignedGreaterThan,
                BinaryOperator::GreaterEqual => IntCC::UnsignedGreaterThanOrEqual,
                _ => unreachable!("verified integer/Bool comparison"),
            };
            let compared = builder.ins().icmp(condition, left, right);
            let value = builder.ins().uextend(types::I64, compared);
            // A comparison adds no failure, but preserve dispatch of any
            // already pending failure and its native/storage cleanup.
            branch_after_runtime_call(
                builder,
                runtime,
                context_value,
                blocks,
                failure,
                cleanup_chain,
            );
            return value;
        }
        let kind = builder.ins().iconst(
            types::I64,
            i64::from(crate::scalar_backend::kind(scalar) as u32),
        );
        let code = builder.ins().iconst(types::I64, code);
        let helper = if compare {
            runtime.scalar_compare
        } else {
            runtime.scalar_arithmetic
        };
        let call = builder
            .ins()
            .call(helper, &[context_value, kind, code, left, right]);
        let value = builder.inst_results(call)[0];
        branch_after_runtime_call(
            builder,
            runtime,
            context_value,
            blocks,
            failure,
            cleanup_chain,
        );
        return value;
    }
    if matches!(operator, BinaryOperator::Equal | BinaryOperator::NotEqual) {
        let ty = runtime
            .data_schema
            .index(operand_type)
            .expect("closed verified operand type");
        let ty = builder
            .ins()
            .iconst(types::I64, i64::try_from(ty).expect("bounded catalog"));
        let code = builder
            .ins()
            .iconst(types::I64, i64::from(operator == BinaryOperator::NotEqual));
        let call = builder.ins().call(
            runtime.value_compare,
            &[context_value, ty, code, left, right],
        );
        let value = builder.inst_results(call)[0];
        branch_after_runtime_call(
            builder,
            runtime,
            context_value,
            blocks,
            failure,
            cleanup_chain,
        );
        return value;
    }
    let comparison = |builder: &mut FunctionBuilder<'_>, condition| {
        let compared = builder.ins().icmp(condition, left, right);
        builder.ins().uextend(types::I64, compared)
    };
    match operator {
        BinaryOperator::Or => builder.ins().bor(left, right),
        BinaryOperator::And => builder.ins().band(left, right),
        BinaryOperator::Equal => comparison(builder, IntCC::Equal),
        BinaryOperator::NotEqual => comparison(builder, IntCC::NotEqual),
        BinaryOperator::Less => comparison(builder, IntCC::SignedLessThan),
        BinaryOperator::LessEqual => comparison(builder, IntCC::SignedLessThanOrEqual),
        BinaryOperator::Greater => comparison(builder, IntCC::SignedGreaterThan),
        BinaryOperator::GreaterEqual => comparison(builder, IntCC::SignedGreaterThanOrEqual),
        BinaryOperator::Add
        | BinaryOperator::Subtract
        | BinaryOperator::Multiply
        | BinaryOperator::Divide
        | BinaryOperator::Remainder => {
            let operation = match operator {
                BinaryOperator::Add => OP_ADD,
                BinaryOperator::Subtract => OP_SUBTRACT,
                BinaryOperator::Multiply => OP_MULTIPLY,
                BinaryOperator::Divide => OP_DIVIDE,
                BinaryOperator::Remainder => OP_REMAINDER,
                _ => unreachable!(),
            };
            let operation = builder.ins().iconst(types::I64, operation);
            let call = builder
                .ins()
                .call(runtime.arithmetic, &[context_value, operation, left, right]);
            let value = builder.inst_results(call)[0];
            branch_after_runtime_call(
                builder,
                runtime,
                context_value,
                blocks,
                failure,
                cleanup_chain,
            );
            value
        }
    }
}

fn end_cleanup(
    builder: &mut FunctionBuilder<'_>,
    runtime: &RuntimeFunctionRefs,
    context: Value,
    scope: rinzu_compiler::SourceRange,
) {
    let region = builder.ins().iconst(
        types::I64,
        runtime.unwind.as_ref().unwrap().cleanup_ids[&scope],
    );
    builder.ins().call(runtime.end_cleanup, &[context, region]);
}

// The C Context is native, nonmoving storage. Its private field offset and
// width are asserted by the generated C runtime. Keep every load at the
// original helper point; Rust reference contexts retain their typed entry.
fn failure_status(
    builder: &mut FunctionBuilder<'_>,
    runtime: &RuntimeFunctionRefs,
    context: Value,
) -> Value {
    if runtime.direct_source {
        builder.ins().load(
            types::I64,
            cranelift_codegen::ir::MemFlagsData::new(),
            context,
            crate::C_CONTEXT_PENDING_PRIMARY_OFFSET,
        )
    } else {
        let call = builder.ins().call(runtime.failure, &[context]);
        builder.inst_results(call)[0]
    }
}

fn branch_if_recoverable(
    builder: &mut FunctionBuilder<'_>,
    runtime: &RuntimeFunctionRefs,
    context: Value,
    recoverable: cranelift_codegen::ir::Block,
    terminal: cranelift_codegen::ir::Block,
) {
    if runtime.direct_source {
        // Only primary == 0 can bypass classification. A nonzero primary
        // may have secondary diagnostics, so preserve the existing helper.
        let primary = failure_status(builder, runtime, context);
        let inspect = builder.create_block();
        builder.set_cold_block(inspect);
        builder.ins().brif(primary, inspect, &[], terminal, &[]);
        builder.switch_to_block(inspect);
        builder.seal_block(inspect);
    }
    let call = builder.ins().call(runtime.has_error, &[context]);
    let pending = builder.inst_results(call)[0];
    builder.ins().brif(pending, recoverable, &[], terminal, &[]);
}

#[allow(clippy::too_many_arguments)]
fn branch_after_error_call(
    builder: &mut FunctionBuilder<'_>,
    runtime: &RuntimeFunctionRefs,
    context: Value,
    blocks: &HashMap<BlockId, cranelift_codegen::ir::Block>,
    error: &rinzu_compiler::ErrorEdge,
    failure: BlockId,
    cleanup: &[BlockId],
) -> Result<()> {
    let continuation = builder.create_block();
    let failed = builder.create_block();
    branch_if_recoverable(builder, runtime, context, failed, continuation);
    builder.switch_to_block(failed);
    builder.seal_block(failed);
    for call in &error.release_native_calls {
        let index = runtime.native_plans.index(runtime.function_index, *call)?;
        let plan = builder.ins().iconst(types::I64, i64::try_from(index)?);
        builder
            .ins()
            .call(runtime.native_helpers["release"], &[context, plan]);
    }
    let unwind = runtime.unwind.as_ref().unwrap();
    if let Some((saved_route, saved_owner)) = unwind.saved_routes.borrow().get(&error.target) {
        let route = builder.use_var(unwind.variable);
        let owner = builder.use_var(unwind.owner);
        builder.def_var(*saved_route, route);
        builder.def_var(*saved_owner, owner);
    }
    let released: Vec<_> = error
        .release_scopes
        .iter()
        .filter(|scope| unwind.cleanup_scopes.contains(scope))
        .collect();
    for scope in &released {
        end_cleanup(builder, runtime, context, **scope);
    }
    if released.is_empty() {
        jump_unwind(builder, runtime, blocks, &error.cleanup_chain, error.target);
    } else {
        let catch = builder.create_block();
        let terminal = builder.create_block();
        branch_if_recoverable(builder, runtime, context, catch, terminal);
        builder.switch_to_block(catch);
        builder.seal_block(catch);

        jump_unwind(builder, runtime, blocks, &error.cleanup_chain, error.target);
        builder.switch_to_block(terminal);
        builder.seal_block(terminal);
        for scope in unwind
            .active_cleanup
            .borrow()
            .iter()
            .rev()
            .filter(|scope| !error.release_scopes.contains(scope))
        {
            end_cleanup(builder, runtime, context, *scope);
        }

        jump_unwind(builder, runtime, blocks, cleanup, failure);
    }
    builder.switch_to_block(continuation);
    builder.seal_block(continuation);
    branch_after_runtime_call(builder, runtime, context, blocks, failure, cleanup);
    Ok(())
}

fn branch_after_runtime_call(
    builder: &mut FunctionBuilder<'_>,
    runtime: &RuntimeFunctionRefs,
    context_value: Value,
    blocks: &HashMap<BlockId, cranelift_codegen::ir::Block>,
    failure: BlockId,
    cleanup_chain: &[BlockId],
) {
    let failed = failure_status(builder, runtime, context_value);
    branch_after_runtime_status(
        builder,
        runtime,
        context_value,
        failed,
        blocks,
        failure,
        cleanup_chain,
    );
}

fn branch_after_runtime_status(
    builder: &mut FunctionBuilder<'_>,
    runtime: &RuntimeFunctionRefs,
    context_value: Value,
    failed: Value,
    blocks: &HashMap<BlockId, cranelift_codegen::ir::Block>,
    failure: BlockId,
    cleanup_chain: &[BlockId],
) {
    let continuation = builder.create_block();
    let failure_block = builder.create_block();
    builder
        .ins()
        .brif(failed, failure_block, &[], continuation, &[]);
    builder.switch_to_block(failure_block);
    builder.seal_block(failure_block);

    for scope in runtime
        .unwind
        .as_ref()
        .unwrap()
        .active_cleanup
        .borrow()
        .iter()
        .rev()
    {
        end_cleanup(builder, runtime, context_value, *scope);
    }
    jump_unwind(builder, runtime, blocks, cleanup_chain, failure);
    builder.switch_to_block(continuation);
    builder.seal_block(continuation);
}

fn jump_unwind(
    builder: &mut FunctionBuilder<'_>,
    runtime: &RuntimeFunctionRefs,
    blocks: &HashMap<BlockId, cranelift_codegen::ir::Block>,
    cleanup: &[BlockId],
    target: BlockId,
) {
    let unwind = runtime.unwind.as_ref().unwrap();
    let mut path = cleanup.to_vec();
    path.push(target);
    let next = builder.ins().iconst(types::I64, unwind.id(&path[1..]));
    builder.def_var(unwind.variable, next);
    let owner = builder.ins().iconst(types::I64, i64::from(path[0].0) + 1);
    builder.def_var(unwind.owner, owner);
    builder.ins().jump(blocks[&path[0]], &[]);
}

fn value_of(values: &HashMap<ValueId, Value>, value: ValueId) -> Result<Value> {
    values
        .get(&value)
        .copied()
        .ok_or_else(|| anyhow!("missing lowered value {value:?}"))
}

fn async_transfer_kind(ty: &Type) -> Result<i64> {
    if *ty == Type::Unit {
        return Ok(11);
    }
    if matches!(
        ty,
        Type::String
            | Type::Bytes
            | Type::FixedBytes(_)
            | Type::Named(_)
            | Type::Native {
                call_only: false,
                ..
            }
            | Type::List(_)
            | Type::Map(_, _)
            | Type::Error
    ) {
        return Ok(12);
    }
    if matches!(ty, Type::Future(_)) {
        return Ok(13);
    }
    ty.scalar()
        .map(|scalar| i64::from(crate::scalar_backend::kind(scalar) as u32))
        .ok_or_else(|| anyhow!("AOT async result {ty:?} requires the R3 data transport backend"))
}

fn stack_array(builder: &mut FunctionBuilder<'_>, values: &[Value]) -> Result<Value> {
    if values.is_empty() {
        return Ok(builder.ins().iconst(types::I64, 0));
    }
    let size = values
        .len()
        .checked_mul(8)
        .and_then(|size| u32::try_from(size).ok())
        .ok_or_else(|| anyhow!("managed argument stack array is too large"))?;
    let slot = builder.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
        cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
        size,
        3,
    ));
    for (index, value) in values.iter().enumerate() {
        builder
            .ins()
            .stack_store(types::I64, *value, slot, i32::try_from(index * 8)?);
    }
    Ok(builder.ins().stack_addr(types::I64, slot, 0))
}
