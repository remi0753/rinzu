use std::collections::{HashMap, HashSet};

mod calls;
mod errors;
mod statements;
mod types;

pub(crate) use statements::block_always_returns;
pub(crate) use types::source_abi_type;
pub use types::{FunctionReferenceType, StaticHandlerType, Type};

use crate::ast::{
    Arguments, BinaryOperator, Block, Declaration, Expression, ExpressionKind, FunctionDecl,
    Literal, Module, TypeRef, TypeRefKind, UnaryOperator,
};
use crate::error_effects::{ErrorConversion, conversion_error, display_bound};
use crate::{
    AbiScalar, Diagnostic, DiagnosticCode, IntegerConstant, SourceRange, evaluate_module_constants,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeCallClass {
    NativeSafe,
    OwnerThreadSync,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalType {
    pub call_only: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeFunction {
    pub parameters: Vec<Type>,
    pub result: Type,
    pub call_class: NativeCallClass,
    pub metadata_complete: bool,
    pub cleanup_safe: bool,
}

#[derive(Clone, Debug, Default)]
pub struct CompilerOptions {
    pub sdk_files: HashSet<crate::FileId>,
    pub is_library: bool,
    pub owner_sync_handlers: HashSet<String>,
    pub external_types: HashMap<String, ExternalType>,
    pub native_functions: HashMap<String, NativeFunction>,
    /// Exported method identities, including templates before specialization.
    pub exported_methods: HashSet<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FunctionFacts {
    pub is_async: bool,
    pub may_fail: bool,
    pub may_await: bool,
    pub may_spawn: bool,
    pub worker_safe: bool,
    pub requires_owner: bool,
    pub cleanup_safe: bool,
    pub native_requirements: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct CheckedFunction {
    pub declaration: FunctionDecl,
    pub parameter_types: Vec<Type>,
    pub result_type: Type,
    pub callback_fallback: Option<crate::FallbackConstant>,
    pub facts: FunctionFacts,
    /// Published ordinary Error bound after inference or explicit annotation.
    pub error_types: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct SupplantCall {
    pub targets: Vec<String>,
    pub reference: Option<String>,
    pub signature: FunctionReferenceType,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceTypeSchema {
    pub kind: String,
    /// Private storage remains in the complete safety/layout graph.
    pub private_fields: Vec<u32>,
    pub fields: Vec<(String, Type)>,
    /// Field indices which a verified StoreField must never write.
    pub const_fields: Vec<u32>,
    pub variants: Vec<(String, Vec<(String, Type)>)>,
}

#[derive(Clone, Debug)]
pub(crate) struct BoundFunction {
    pub target: String,
    pub signature: FunctionReferenceType,
    pub captures: Vec<(String, Type)>,
}

#[derive(Clone, Debug)]
pub(crate) struct IndirectSignature {
    pub signature: FunctionReferenceType,
    pub contract: Option<SourceRange>,
}

#[derive(Clone, Debug)]
pub struct CheckedModule {
    pub(crate) error_upcasts: HashMap<SourceRange, Type>,
    pub(crate) error_patterns: HashMap<SourceRange, Type>,
    pub(crate) rethrows: HashMap<SourceRange, errors::Rethrow>,
    pub(crate) error_tests: HashMap<SourceRange, Type>,
    pub(crate) error_narrowings: HashMap<SourceRange, Type>,
    pub(crate) bindings: HashMap<SourceRange, BoundFunction>,
    pub(crate) static_references: HashMap<SourceRange, String>,
    pub(crate) type_references: HashMap<SourceRange, Type>,
    pub(crate) closure_source: Option<crate::FileId>,
    pub(crate) indirect_fields: HashMap<SourceRange, String>,
    pub(crate) native_captures: HashMap<SourceRange, Type>,
    pub(crate) native_record_reads: HashSet<SourceRange>,
    pub source_types: std::collections::BTreeMap<String, SourceTypeSchema>,
    pub(crate) source_values: std::collections::BTreeMap<String, Type>,
    pub core_lengths: HashMap<SourceRange, crate::CoreLengthKind>,
    pub task_controls: HashMap<SourceRange, crate::TaskControlKind>,
    pub data_operations: HashMap<SourceRange, crate::DataOperation>,
    pub(crate) in_bounds_collection_updates: HashSet<SourceRange>,
    pub stdout_calls: HashMap<SourceRange, bool>,
    pub syntax: Module,
    pub functions: Vec<CheckedFunction>,
    pub expression_types: HashMap<SourceRange, Type>,
    pub native_calls: HashMap<SourceRange, NativeCallClass>,
    pub source_native_calls: HashMap<SourceRange, crate::SourceNativeSignature>,
    pub reference_targets: HashMap<SourceRange, Vec<String>>,
    pub indirect_calls: HashMap<SourceRange, Vec<String>>,
    pub(crate) indirect_signatures: HashMap<SourceRange, IndirectSignature>,
    pub(crate) error_conversions: HashMap<SourceRange, ErrorConversion>,
    pub supplant_calls: HashMap<SourceRange, SupplantCall>,
    pub source_native_signatures: Vec<crate::SourceNativeSignature>,
    pub constants: std::collections::BTreeMap<String, IntegerConstant>,
    pub constant_references: HashMap<SourceRange, IntegerConstant>,
    pub byte_coercions: HashMap<SourceRange, (u64, u64)>,
}

impl CheckedModule {
    #[must_use]
    pub fn expression_type(&self, range: SourceRange) -> Option<&Type> {
        self.expression_types.get(&range)
    }
}

pub(crate) fn display_reads_native_record(
    types: &std::collections::BTreeMap<String, SourceTypeSchema>,
    ty: &Type,
) -> bool {
    fn visit(
        types: &std::collections::BTreeMap<String, SourceTypeSchema>,
        ty: &Type,
        seen: &mut HashSet<String>,
    ) -> bool {
        match ty {
            Type::List(inner) => visit(types, inner, seen),
            Type::Map(key, value) => visit(types, key, seen) || visit(types, value, seen),
            Type::Named(name) if seen.insert(name.clone()) => {
                let result = types.get(name).is_some_and(|schema| {
                    schema.kind == "native-record"
                        || matches!(schema.kind.as_str(), "record" | "enum")
                            && schema
                                .fields
                                .iter()
                                .chain(schema.variants.iter().flat_map(|(_, fields)| fields))
                                .any(|(_, ty)| visit(types, ty, seen))
                });
                seen.remove(name);
                result
            }
            _ => false,
        }
    }
    visit(types, ty, &mut HashSet::new())
}

#[derive(Clone)]
struct FunctionSignature {
    parameters: Vec<Type>,
    result: Type,
    is_async: bool,
    error_bound: Option<Vec<String>>,
    range: SourceRange,
}

#[derive(Clone)]
struct Local {
    ty: Type,
    mutable: bool,
    constant: Option<IntegerConstant>,
    targets: Vec<String>,
    callback_contract: Option<SourceRange>,
}

fn callback_type_contract(ty: &Type, source: Option<&TypeRef>) -> Option<SourceRange> {
    let Type::FunctionReference(signature) = ty else {
        return None;
    };
    signature.error_bound.as_ref()?;
    let source = source?;
    let range = source.range;
    if let TypeRefKind::Function {
        error_contract: Some(contract),
        ..
    } = &source.kind
        && contract.range.file_id == range.file_id
        && range.start_byte <= contract.range.start_byte
        && contract.range.end_byte <= range.end_byte
    {
        return Some(contract.range);
    }
    Some(range)
}

#[derive(Clone, Copy)]
enum ExpectedErrorBound<'a> {
    Parameter { function: &'a str, index: usize },
    Annotation(SourceRange),
    Result(SourceRange),
}

#[derive(Clone, Default)]
struct DirectFacts {
    indirect_sites: Vec<(String, SourceRange, Vec<String>)>,
    may_fail: bool,
    may_await: bool,
    may_spawn: bool,
    has_native: bool,
    requires_owner: bool,
    has_cleanup_unsafe: bool,
    native_requirements: HashSet<String>,
    calls: HashSet<String>,
    cleanup_calls: HashMap<String, SourceRange>,
    spawn_calls: Vec<(String, SourceRange)>,
}

pub fn check_module(module: &Module) -> Result<CheckedModule, Vec<Diagnostic>> {
    check_module_with_options(module, &CompilerOptions::default())
}

pub(crate) fn local_spawn_error(range: SourceRange) -> Diagnostic {
    Diagnostic::error(
        DiagnosticCode::InvalidSpawn,
        range,
        "spawn requires an explicit qualified function, not a receiver or function value",
        "write spawn Owner.method(receiver, arguments)",
    )
}

pub(crate) struct ProbeScope<'a> {
    pub function: Option<&'a FunctionDecl>,
    pub locals: &'a HashMap<String, TypeRef>,
    pub option_shadow_modules: &'a HashSet<String>,
}

/// Generic constraint collection uses the ordinary expression checker, including
/// literal defaults, record canonicalization and numeric rules. This probe has
/// no effect/escape authority; the fully specialized program is checked again.
pub(crate) fn probe_expression_type(
    module: &Module,
    options: &CompilerOptions,
    expression: &Expression,
    expected: Option<&TypeRef>,
    scope: ProbeScope<'_>,
    data_operation: Option<crate::DataOperation>,
) -> Option<TypeRef> {
    let mut checker = Checker::new(module, options, scope.option_shadow_modules);
    checker.collect_declarations();
    checker.constants = evaluate_module_constants(module).unwrap_or_default();
    checker.prepare_error_types();
    checker.collect_function_signatures();
    if let Ok(signatures) =
        crate::native_interface::source_native_signatures_with_records(module, |ty, constants| {
            checker.native_context_record(ty, constants)
        })
    {
        checker.source_signatures = signatures
            .into_iter()
            .map(|s| (s.name.clone(), s))
            .collect();
    }
    let function = FunctionDecl {
        error_contract: None,
        // The caller's identity determines which methods are visible in this
        // module, including in doc examples, SDKs and nested closures.
        name: scope
            .function
            .map_or_else(|| "$type_probe".into(), |function| function.name.clone()),
        is_private: false,
        lexical_owner: scope
            .function
            .and_then(|function| function.lexical_owner.clone()),
        type_parameters: Vec::new(),
        owner_type_parameter_count: 0,
        is_async: true,
        parameters: Vec::new(),
        result: TypeRef::unit(expression.range),
        body: crate::ast::Block {
            statements: Vec::new(),
            range: expression.range,
        },
        fallback: None,
        range: expression.range,
    };
    let mut context = FunctionContext::new(&function, Type::Unit, false);
    for (name, ty) in scope.locals {
        if let Some(ty) = checker.resolve_type(ty) {
            let targets = if let Type::FunctionReference(signature) = &ty {
                checker.compatible_reference_targets(signature)
            } else {
                Vec::new()
            };
            context.locals.insert(
                name.clone(),
                Local {
                    ty,
                    mutable: false,
                    constant: None,
                    targets,
                    callback_contract: None,
                },
            );
        }
    }
    let expected = expected.and_then(|ty| checker.resolve_type(ty));
    checker.diagnostics.clear();
    let mut ty = checker.check_expression(expression, expected.as_ref(), &mut context)?;
    if !checker.diagnostics.is_empty() {
        return None;
    }
    if let Some(operation) = data_operation {
        let (parameters, result) = crate::core::data_signature(operation, &ty)?;
        ty = Type::FunctionReference(Box::new(FunctionReferenceType {
            supplant: false,
            parameters,
            result: Box::new(result),
            error_bound: None,
        }));
    }
    fn source(checker: &Checker<'_>, ty: &Type, range: SourceRange) -> Option<TypeRef> {
        let kind = match ty {
            Type::Named(name) if name.starts_with("$rinzu_record_") => {
                let mut fields = checker.record_declaration(name)?.fields.clone();
                for field in &mut fields {
                    let ty = checker.resolve_type_readonly(&field.ty)?;
                    field.ty = source(checker, &ty, range)?;
                }
                TypeRefKind::Record(fields)
            }
            // Specialization already tracks the arguments of Option instances
            // present in the input. Reconstruct the types synthesized by this
            // checker, which will disappear when the probe finishes.
            Type::Named(name)
                if name.starts_with("Option<")
                    && !checker.module.declarations.iter().any(|d| d.name() == name)
                    && let Some(Declaration::Enum(enumeration)) =
                        checker.declarations.get(name)
                    && let [_, some] = enumeration.variants.as_slice()
                    && let [field] = some.fields.as_slice() =>
            {
                let payload = checker.resolve_type_readonly(&field.ty)?;
                TypeRefKind::Generic {
                    path: vec!["Option".into()],
                    arguments: vec![source(checker, &payload, range)?],
                }
            }
            Type::List(t) => TypeRefKind::List(Box::new(source(checker, t, range)?)),
            Type::Future(t) => TypeRefKind::Future(Box::new(source(checker, t, range)?)),
            Type::Map(k, v) => TypeRefKind::Map(
                Box::new(source(checker, k, range)?),
                Box::new(source(checker, v, range)?),
            ),
            Type::FunctionReference(signature) => TypeRefKind::Function {
                supplant: signature.supplant,
                parameters: signature
                    .parameters
                    .iter()
                    .map(|t| source(checker, t, range))
                    .collect::<Option<Vec<_>>>()?,
                result: Box::new(source(checker, &signature.result, range)?),
                error_contract: signature.contract(range),
            },
            _ => return Some(crate::core::type_ref(ty, range)),
        };
        Some(TypeRef { kind, range })
    }
    source(&checker, &ty, expression.range)
}

pub(crate) fn option_instance_name(
    module: &Module,
    options: &CompilerOptions,
    payload: &TypeRef,
) -> Option<String> {
    let payload = Checker::new(module, options, &HashSet::new()).resolve_type_readonly(payload)?;
    Some(crate::core::option_name(&payload))
}

pub fn check_module_with_options(
    module: &Module,
    options: &CompilerOptions,
) -> Result<CheckedModule, Vec<Diagnostic>> {
    let _timing = crate::timing::span("frontend.check");
    let mut module = module.clone();
    crate::package::link_builtin_imports(&mut module)?;
    let module = crate::collections::prepare(module);
    let module = crate::methods::expand_exports(&module)?;
    crate::package::validate_public_signatures(
        &module,
        &module
            .exports
            .iter()
            .map(|export| export.range.file_id)
            .collect(),
        &options.sdk_files,
    )?;
    let option_shadow_modules = crate::core::option_shadow_modules(&module);
    let source_syntax = &module;
    let module = crate::records::expand_aliases(&module, true)?;
    let mut module = crate::value_constants::expand(&module)?;
    crate::error_effects::validate_annotations(&mut module)?;
    let mut module = crate::interfaces::prepare(&module)?;
    module.declarations.retain(|declaration| !matches!(declaration, Declaration::TypeAlias(alias) if !alias.type_parameters.is_empty()));
    crate::methods::validate(&module)?;
    // Expansion removes template exports. Keep their visibility for both
    // inference probes and the checker of the specialized functions.
    let mut options = options.clone();
    options.exported_methods.extend(
        module
            .exports
            .iter()
            .map(|export| export.path.join("."))
            .filter(|name| crate::methods::parts(name).is_some()),
    );
    let (module, requirements) =
        crate::generics::expand(&module, &options, &option_shadow_modules)?;
    let mut checker = Checker::new(&module, &options, &option_shadow_modules);
    checker.source_syntax = source_syntax;
    checker.generic_requirements = requirements;
    let checked = checker.run()?;
    crate::error_effects::verify_conversions(&checked)?;
    let mut checked = if let Some((converted, file)) = crate::closures::convert(&checked)? {
        let mut checker = Checker::new(&converted, &options, &option_shadow_modules);
        checker.source_syntax = source_syntax;
        checker.source_indirect_fields = checked.indirect_fields.clone();
        checker.closure_conversion = true;
        let mut result = checker.run()?;
        result.source_values = checked.source_values;
        // Runtime closure shapes erase bounds. Analyze each invocation with
        // its source bound while retaining the lowered target and captures.
        result.indirect_signatures.extend(
            checked
                .indirect_signatures
                .into_iter()
                .filter(|(_, call)| call.signature.error_bound.is_some()),
        );
        result.closure_source = Some(file);
        result
    } else {
        checked
    };
    checked.in_bounds_collection_updates = crate::ir::collection_buffers::in_bounds(&checked);
    let summaries = crate::error_effects::analyze(&checked, !options.is_library)?;
    for function in &mut checked.functions {
        function.error_types =
            crate::error_effects::public_types(&summaries[&function.declaration.name]);
    }
    Ok(checked)
}

pub(crate) fn resolve_source_native_signatures(
    module: &Module,
) -> Result<Vec<crate::SourceNativeSignature>, Vec<Diagnostic>> {
    let options = CompilerOptions::default();
    let option_shadow_modules = crate::core::option_shadow_modules(module);
    let mut checker = Checker::new(module, &options, &option_shadow_modules);
    checker.collect_declarations();
    checker.constants = evaluate_module_constants(module)?;
    let signatures =
        crate::native_interface::source_native_signatures_with_records(module, |ty, constants| {
            checker.native_context_record(ty, constants)
        });
    if checker.diagnostics.is_empty() {
        signatures
    } else {
        if let Err(errors) = signatures {
            checker.diagnostics.extend(errors);
        }
        Err(checker.diagnostics)
    }
}

struct Checker<'a> {
    error_schemas: std::collections::BTreeMap<String, SourceTypeSchema>,
    error_upcasts: HashMap<SourceRange, Type>,
    error_patterns: HashMap<SourceRange, Type>,
    rethrows: HashMap<SourceRange, errors::Rethrow>,
    error_tests: HashMap<SourceRange, Type>,
    error_narrowings: HashMap<SourceRange, Type>,
    lambda_replacements: HashMap<SourceRange, ExpressionKind>,
    generated_instance: u32,
    record_literals: HashMap<SourceRange, String>,
    method_calls: HashMap<SourceRange, crate::ast::ExpressionKind>,
    bindings: HashMap<SourceRange, BoundFunction>,
    static_references: HashMap<SourceRange, String>,
    type_references: HashMap<SourceRange, Type>,
    generic_requirements: Vec<crate::generics::Requirement>,
    option_shadow_modules: &'a HashSet<String>,
    stdout_calls: HashMap<SourceRange, bool>,
    core_lengths: HashMap<SourceRange, crate::CoreLengthKind>,
    task_controls: HashMap<SourceRange, crate::TaskControlKind>,
    data_operations: HashMap<SourceRange, crate::DataOperation>,
    module: &'a Module,
    source_syntax: &'a Module,
    options: &'a CompilerOptions,
    declarations: HashMap<String, Declaration>,
    functions: HashMap<String, FunctionSignature>,
    expression_types: HashMap<SourceRange, Type>,
    direct_facts: HashMap<String, DirectFacts>,
    native_calls: HashMap<SourceRange, NativeCallClass>,
    source_native_calls: HashMap<SourceRange, crate::SourceNativeSignature>,
    native_record_reads: HashSet<SourceRange>,
    source_signatures: HashMap<String, crate::SourceNativeSignature>,
    indirect_fields: HashMap<SourceRange, String>,
    source_indirect_fields: HashMap<SourceRange, String>,
    closure_conversion: bool,
    native_captures: HashMap<SourceRange, Type>,
    reference_targets: HashMap<SourceRange, Vec<String>>,
    indirect_calls: HashMap<SourceRange, Vec<String>>,
    indirect_signatures: HashMap<SourceRange, IndirectSignature>,
    error_conversions: HashMap<SourceRange, ErrorConversion>,
    supplant_calls: HashMap<SourceRange, SupplantCall>,
    constants: std::collections::BTreeMap<String, IntegerConstant>,
    constant_references: HashMap<SourceRange, IntegerConstant>,
    byte_coercions: HashMap<SourceRange, (u64, u64)>,
    fallbacks: HashMap<String, crate::FallbackConstant>,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Checker<'a> {
    fn new(
        module: &'a Module,
        options: &'a CompilerOptions,
        option_shadow_modules: &'a HashSet<String>,
    ) -> Self {
        Self {
            error_schemas: std::collections::BTreeMap::new(),
            error_upcasts: HashMap::new(),
            error_patterns: HashMap::new(),
            rethrows: HashMap::new(),
            error_tests: HashMap::new(),
            error_narrowings: HashMap::new(),
            lambda_replacements: HashMap::new(),
            generated_instance: 0x80000000,
            record_literals: HashMap::new(),
            method_calls: HashMap::new(),
            bindings: HashMap::new(),
            static_references: HashMap::new(),
            type_references: HashMap::new(),
            core_lengths: HashMap::new(),
            task_controls: HashMap::new(),
            data_operations: HashMap::new(),
            generic_requirements: Vec::new(),
            option_shadow_modules,
            stdout_calls: HashMap::new(),
            module,
            source_syntax: module,
            options,
            declarations: HashMap::new(),
            functions: HashMap::new(),
            expression_types: HashMap::new(),
            direct_facts: HashMap::new(),
            native_calls: HashMap::new(),
            source_native_calls: HashMap::new(),
            native_record_reads: HashSet::new(),
            source_signatures: HashMap::new(),
            indirect_fields: HashMap::new(),
            source_indirect_fields: HashMap::new(),
            closure_conversion: false,
            native_captures: HashMap::new(),
            reference_targets: HashMap::new(),
            indirect_calls: HashMap::new(),
            indirect_signatures: HashMap::new(),
            error_conversions: HashMap::new(),
            supplant_calls: HashMap::new(),
            constants: std::collections::BTreeMap::new(),
            constant_references: HashMap::new(),
            byte_coercions: HashMap::new(),
            fallbacks: HashMap::new(),
            diagnostics: Vec::new(),
        }
    }

    fn run(mut self) -> Result<CheckedModule, Vec<Diagnostic>> {
        let _timing = crate::timing::span("frontend.typecheck");
        self.collect_declarations();
        match evaluate_module_constants(self.module) {
            Ok(values) => self.constants = values,
            Err(errors) => self.diagnostics.extend(errors),
        }
        self.prepare_error_types();
        self.validate_type_declarations();
        self.source_signatures = crate::native_interface::source_native_signatures_with_records(
            self.module,
            |ty, constants| self.native_context_record(ty, constants),
        )?
        .into_iter()
        .map(|signature| (signature.name.clone(), signature))
        .collect();
        self.validate_generic_requirements();
        self.collect_function_signatures();
        self.collect_bindings();
        self.validate_entry();
        self.check_functions();
        let facts = self.infer_facts();
        self.validate_spawn_targets(&facts);
        self.validate_cleanup_targets(&facts);
        if !self.diagnostics.is_empty() {
            return Err(self.diagnostics);
        }
        let fields = |fields: &[crate::ast::FieldDecl]| {
            fields
                .iter()
                .map(|field| {
                    (
                        field.name.clone(),
                        self.resolve_type_readonly(&field.ty)
                            .expect("validated field type"),
                    )
                })
                .collect()
        };
        let mut source_types = std::collections::BTreeMap::new();
        for declaration in self.declarations.values() {
            let schema = match declaration {
                Declaration::Record(record) => SourceTypeSchema {
                    private_fields: record
                        .fields
                        .iter()
                        .enumerate()
                        .filter(|(_, f)| f.is_private)
                        .map(|(i, _)| i as u32)
                        .collect(),
                    kind: if record.mutable && record.fields.iter().any(|field| !field.is_const) {
                        "state"
                    } else {
                        "record"
                    }
                    .to_owned(),
                    fields: fields(&record.fields),
                    const_fields: record
                        .fields
                        .iter()
                        .enumerate()
                        .filter(|(_, field)| field.is_const)
                        .map(|(index, _)| index as u32)
                        .collect(),
                    variants: Vec::new(),
                },
                Declaration::Enum(enumeration) => SourceTypeSchema {
                    private_fields: Vec::new(),
                    kind: "enum".to_owned(),
                    fields: Vec::new(),
                    const_fields: Vec::new(),
                    variants: enumeration
                        .variants
                        .iter()
                        .map(|variant| (variant.name.clone(), fields(&variant.fields)))
                        .collect(),
                },
                Declaration::Error(error) => self.error_schemas[&error.name].clone(),
                _ => continue,
            };
            source_types.insert(declaration.name().to_owned(), schema);
        }
        for member in self
            .module
            .native_blocks
            .iter()
            .flat_map(|block| &block.members)
        {
            match member {
                crate::ast::NativeMember::Opaque { name, .. } => {
                    source_types.insert(
                        name.clone(),
                        SourceTypeSchema {
                            private_fields: Vec::new(),
                            kind: "opaque".to_owned(),
                            fields: Vec::new(),
                            const_fields: Vec::new(),
                            variants: Vec::new(),
                        },
                    );
                }
                crate::ast::NativeMember::Record(record) => {
                    source_types.insert(
                        record.name.clone(),
                        SourceTypeSchema {
                            private_fields: Vec::new(),
                            kind: "native-record".to_owned(),
                            fields: fields(&record.fields),
                            // Source assignment is prohibited by the declaration,
                            // but native code can mutate every ABI field.
                            const_fields: Vec::new(),
                            variants: Vec::new(),
                        },
                    );
                }
                _ => {}
            }
        }
        let mut source_native_signatures: Vec<_> = std::mem::take(&mut self.source_signatures)
            .into_values()
            .collect();
        source_native_signatures.sort_by(|a, b| a.name.cmp(&b.name));
        let mut syntax = self.module.clone();
        let mut synthetic: Vec<_> = self
            .declarations
            .values()
            .filter(|d| {
                !syntax
                    .declarations
                    .iter()
                    .any(|original| original.name() == d.name())
            })
            .cloned()
            .collect();
        synthetic.sort_by(|a, b| a.name().cmp(b.name()));
        syntax.declarations.extend(synthetic);
        crate::lambdas::normalize(&mut syntax, &self.lambda_replacements);
        crate::records::normalize(&mut syntax, &self.record_literals, |t| {
            self.resolve_type_readonly(t)
                .or_else(|| self.type_references.get(&t.range).cloned())
        });
        crate::methods::normalize(&mut syntax, &self.method_calls);
        let functions: Vec<_> = syntax
            .declarations
            .iter()
            .filter_map(|declaration| {
                let Declaration::Function(function) = declaration else {
                    return None;
                };
                let signature = self.functions.get(&function.name)?;
                Some(CheckedFunction {
                    error_types: Vec::new(),
                    declaration: function.clone(),
                    parameter_types: signature.parameters.clone(),
                    result_type: signature.result.clone(),
                    callback_fallback: self.fallbacks.get(&function.name).cloned(),
                    facts: facts.get(&function.name).cloned().unwrap_or_default(),
                })
            })
            .collect();
        let source_values = functions
            .iter()
            .filter_map(|function| {
                let name = function
                    .declaration
                    .name
                    .strip_prefix(crate::value_constants::ACCESSOR_PREFIX)?;
                Some((name.to_owned(), function.result_type.clone()))
            })
            .collect();
        Ok(CheckedModule {
            error_upcasts: self.error_upcasts,
            error_patterns: self.error_patterns,
            rethrows: self.rethrows,
            error_tests: self.error_tests,
            error_narrowings: self.error_narrowings,
            bindings: self.bindings,
            static_references: self.static_references,
            type_references: self.type_references,
            closure_source: None,
            indirect_fields: self.indirect_fields,
            native_captures: self.native_captures,
            source_types,
            source_values,
            core_lengths: self.core_lengths,
            task_controls: self.task_controls,
            data_operations: self.data_operations,
            in_bounds_collection_updates: HashSet::new(),
            stdout_calls: self.stdout_calls,
            syntax,
            functions,
            expression_types: self.expression_types,
            native_calls: self.native_calls,
            source_native_calls: self.source_native_calls,
            native_record_reads: self.native_record_reads,
            reference_targets: self.reference_targets,
            indirect_calls: self.indirect_calls,
            indirect_signatures: self.indirect_signatures,
            error_conversions: self.error_conversions,
            supplant_calls: self.supplant_calls,
            source_native_signatures,
            constants: self.constants,
            constant_references: self.constant_references,
            byte_coercions: self.byte_coercions,
        })
    }

    fn native_context_record(
        &mut self,
        type_ref: &TypeRef,
        constants: &std::collections::BTreeMap<String, IntegerConstant>,
    ) -> Option<String> {
        let Type::Named(identity) = self.resolve_type_with_constants(type_ref, constants)? else {
            return None;
        };
        // Const-only classes may share the immutable physical representation.
        // Their nominal declaration still does not grant a context ABI.
        matches!(self.declarations.get(&identity), Some(Declaration::Record(record)) if !record.mutable)
            .then_some(identity)
    }

    fn collect_declarations(&mut self) {
        for declaration in &self.module.declarations {
            if matches!(declaration.name(), "Error" | "SourceLocation") {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::DuplicateName,
                    declaration.range(),
                    format!("{} is a compiler-known type", declaration.name()),
                    "choose a different declaration name",
                ));
            }
            if let Some(previous) = self
                .declarations
                .insert(declaration.name().to_owned(), declaration.clone())
            {
                self.diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::DuplicateName,
                        declaration.range(),
                        format!("duplicate top-level name `{}`", declaration.name()),
                        "rename or remove one declaration",
                    )
                    .with_secondary(previous.range(), "first declaration is here"),
                );
            }
        }
    }

    fn validate_type_declarations(&mut self) {
        for declaration in &self.module.declarations {
            match declaration {
                Declaration::Interface(_) => {
                    unreachable!("interfaces are erased before type checking")
                }
                Declaration::Record(record) => {
                    for i in &record.interfaces {
                        if let TypeRefKind::Generic { arguments, .. } = &i.kind {
                            for a in arguments {
                                self.resolve_type(a);
                            }
                        }
                    }
                    self.check_unique_fields(&record.fields, &record.name);
                    for field in &record.fields {
                        self.resolve_type(&field.ty);
                    }
                }
                Declaration::Enum(enum_decl) => {
                    for i in &enum_decl.interfaces {
                        if let TypeRefKind::Generic { arguments, .. } = &i.kind {
                            for a in arguments {
                                self.resolve_type(a);
                            }
                        }
                    }
                    let mut variants = HashMap::new();
                    for variant in &enum_decl.variants {
                        if let Some(previous) = variants.insert(&variant.name, variant.range) {
                            self.diagnostics.push(
                                Diagnostic::error(
                                    DiagnosticCode::DuplicateName,
                                    variant.range,
                                    format!("duplicate variant `{}`", variant.name),
                                    "rename or remove one variant",
                                )
                                .with_secondary(previous, "first variant is here"),
                            );
                        }
                        self.check_unique_fields(&variant.fields, &variant.name);
                        for field in &variant.fields {
                            self.resolve_type(&field.ty);
                        }
                    }
                }
                Declaration::Constant(_) => {}
                Declaration::Error(_) => {}
                Declaration::TypeAlias(alias) => {
                    self.resolve_type(&alias.ty);
                }
                Declaration::Function(_) => {}
            }
        }
    }

    fn check_unique_fields(&mut self, fields: &[crate::ast::FieldDecl], owner: &str) {
        let mut names = HashMap::new();
        for field in fields {
            if let Some(previous) = names.insert(&field.name, field.range) {
                self.diagnostics.push(
                    Diagnostic::error(
                        DiagnosticCode::DuplicateName,
                        field.range,
                        format!("duplicate field `{}` in `{owner}`", field.name),
                        "rename or remove one field",
                    )
                    .with_secondary(previous, "first field is here"),
                );
            }
        }
    }

    fn collect_bindings(&mut self) {
        #[derive(Default)]
        struct Collector(Vec<Expression>);
        impl crate::ast_walk::Visitor for Collector {
            fn expression(&mut self, e: &mut Expression) {
                if matches!(e.kind, ExpressionKind::Bind { .. }) {
                    self.0.push(e.clone());
                }
            }
        }
        let mut collector = Collector::default();
        crate::ast_walk::module(&mut self.module.clone(), &mut collector);
        for expression in collector.0 {
            let ExpressionKind::Bind {
                target, arguments, ..
            } = &expression.kind
            else {
                unreachable!()
            };
            let Arguments::Positional(values) = arguments else {
                self.type_error(
                    expression.range,
                    "bind captures require positional arguments",
                );
                continue;
            };
            let name = target.join(".");
            let Some(signature) = self.functions.get(&name).cloned() else {
                self.type_error(
                    expression.range,
                    "bind target must be a concrete managed function",
                );
                continue;
            };
            if values.len() > signature.parameters.len() {
                self.type_error(
                    expression.range,
                    "bind has more captures than target parameters",
                );
                continue;
            }
            let Some(Declaration::Function(declaration)) = self.declarations.get(&name) else {
                unreachable!()
            };
            let error_bound = self.reference_bound(&signature);
            self.bindings.insert(
                expression.range,
                BoundFunction {
                    target: name,
                    signature: FunctionReferenceType {
                        parameters: signature.parameters[values.len()..].to_vec(),
                        result: Box::new(if signature.is_async {
                            Type::Future(Box::new(signature.result))
                        } else {
                            signature.result
                        }),
                        supplant: false,
                        error_bound,
                    },
                    captures: declaration
                        .parameters
                        .iter()
                        .zip(signature.parameters)
                        .take(values.len())
                        .map(|(parameter, ty)| (parameter.name.clone(), ty))
                        .collect(),
                },
            );
        }
    }
    fn capture_failure_path(
        &self,
        ty: &Type,
        path: &str,
        visiting: &mut HashSet<String>,
    ) -> String {
        match ty {
            Type::Future(_)
            | Type::Native {
                call_only: true, ..
            } => format!("{path} -> {ty}"),
            Type::List(value) => self.capture_failure_path(value, &format!("{path}[]"), visiting),
            Type::Map(_, value) => {
                self.capture_failure_path(value, &format!("{path}.value"), visiting)
            }
            Type::Named(name) if visiting.insert(name.clone()) => {
                let fields = match self.declarations.get(name) {
                    Some(Declaration::Record(r)) => r
                        .fields
                        .iter()
                        .map(|f| (f.name.clone(), &f.ty))
                        .collect::<Vec<_>>(),
                    Some(Declaration::Enum(e)) => e
                        .variants
                        .iter()
                        .flat_map(|v| {
                            v.fields
                                .iter()
                                .map(move |f| (format!("{}.{}", v.name, f.name), &f.ty))
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                for (name, field) in fields {
                    if let Some(child) = self.resolve_type_readonly(field)
                        && (self.contains_call_only_type(&child, &mut HashSet::new())
                            || self.contains_future(&child, &mut HashSet::new()))
                    {
                        let result =
                            self.capture_failure_path(&child, &format!("{path}.{name}"), visiting);
                        visiting.remove(match ty {
                            Type::Named(name) => name,
                            _ => unreachable!(),
                        });
                        return result;
                    }
                }
                visiting.remove(name);
                path.to_owned()
            }
            _ => path.to_owned(),
        }
    }
    fn contains_future(&self, ty: &Type, visiting: &mut HashSet<String>) -> bool {
        match ty {
            Type::Future(_) => true,
            // A closure schema contains every compatible environment variant.
            // Its actual retained variant is checked by the effect analysis.
            Type::Named(name) if self.closure_conversion && name.starts_with("$rinzu_closure_") => {
                false
            }
            Type::List(value) => self.contains_future(value, visiting),
            Type::Map(k, v) => {
                self.contains_future(k, visiting) || self.contains_future(v, visiting)
            }
            Type::Named(name) if visiting.insert(name.clone()) => {
                let fields = match self.declarations.get(name) {
                    Some(Declaration::Record(r)) => {
                        r.fields.iter().map(|field| &field.ty).collect::<Vec<_>>()
                    }
                    Some(Declaration::Enum(e)) => e
                        .variants
                        .iter()
                        .flat_map(|v| v.fields.iter().map(|field| &field.ty))
                        .collect(),
                    Some(Declaration::Error(error)) => {
                        error.payload.iter().map(|payload| &payload.ty).collect()
                    }
                    _ => Vec::new(),
                };
                let result = fields.iter().any(|f| {
                    self.resolve_type_readonly(f)
                        .is_some_and(|child| self.contains_future(&child, visiting))
                });
                visiting.remove(name);
                result
            }
            _ => false,
        }
    }

    fn collect_function_signatures(&mut self) {
        for declaration in &self.module.declarations {
            let Declaration::Function(function) = declaration else {
                continue;
            };
            let mut signature_constants = self.constants.clone();
            for parameter in &function.parameters {
                signature_constants.remove(&parameter.name);
            }
            let mut parameter_names = HashMap::new();
            let mut parameters = Vec::new();
            for parameter in &function.parameters {
                if let Some(previous) = parameter_names.insert(&parameter.name, parameter.range) {
                    self.diagnostics.push(
                        Diagnostic::error(
                            DiagnosticCode::DuplicateName,
                            parameter.range,
                            format!("duplicate parameter `{}`", parameter.name),
                            "rename one parameter",
                        )
                        .with_secondary(previous, "first parameter is here"),
                    );
                }
                if let Some(ty) =
                    self.resolve_type_with_constants(&parameter.ty, &signature_constants)
                {
                    parameters.push(ty);
                }
            }
            let Some(result) =
                self.resolve_type_with_constants(&function.result, &signature_constants)
            else {
                continue;
            };
            let error_bound = function
                .error_contract
                .as_ref()
                .map(|contract| self.resolve_error_bound(contract, &signature_constants));
            if function.is_async && matches!(result, Type::Future(_)) {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::TypeMismatch,
                    function.result.range,
                    "an async function source result cannot be `Future<T>`",
                    "declare the inner result type and return it after awaiting nested work",
                ));
            }
            self.functions.insert(
                function.name.clone(),
                FunctionSignature {
                    parameters,
                    result,
                    is_async: function.is_async,
                    error_bound,
                    range: function.range,
                },
            );
        }
    }

    fn resolve_error_bound(
        &mut self,
        contract: &crate::ast::ErrorContract,
        constants: &std::collections::BTreeMap<String, IntegerConstant>,
    ) -> Vec<String> {
        let mut types = std::collections::BTreeSet::new();
        for source in &contract.types {
            if let Some(ty) = self.resolve_type_with_constants(source, constants) {
                if !self.is_error_type(&ty) {
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::InvalidErrorContract,
                        source.range,
                        format!("`{ty}` is not an Error type"),
                        "list concrete Error types in throws(...), or use nothrow",
                    ));
                }
                if !types.insert(ty.to_string()) {
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::InvalidErrorContract,
                        source.range,
                        "duplicate Error type in throws contract",
                        "remove the duplicate type",
                    ));
                }
            }
        }
        if types.contains("Error") && contract.types.len() > 1 {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::InvalidErrorContract,
                contract.range,
                "throws(Error) already includes every ordinary Error",
                "write throws(Error) without additional types",
            ));
        }
        types.into_iter().collect()
    }

    fn validate_entry(&mut self) {
        if self.options.is_library {
            return;
        }
        let Some(signature) = self.functions.get("main") else {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::InvalidEntryPoint,
                self.module.range,
                "entry module has no `main` function",
                "add `fn main() { ... }` or `async fn main() { ... }`",
            ));
            return;
        };
        if !signature.parameters.is_empty() || signature.result != Type::Unit {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::InvalidEntryPoint,
                signature.range,
                "`main` must have no parameters and return `Unit`",
                "use `fn main() { ... }` or `async fn main() { ... }`",
            ));
        }
    }

    fn check_functions(&mut self) {
        let functions = self
            .module
            .declarations
            .iter()
            .filter_map(|d| {
                if let Declaration::Function(f) = d {
                    Some(f.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        for function in functions {
            self.check_function(&function, &HashMap::new());
        }
    }
    fn check_function(&mut self, function: &FunctionDecl, captures: &HashMap<String, Local>) {
        let Some(signature) = self.functions.get(&function.name).cloned() else {
            return;
        };
        let mut context = FunctionContext::new(
            function,
            signature.result.clone(),
            self.options.owner_sync_handlers.contains(&function.name),
        );
        if let Some(fallback) = &function.fallback {
            if function.is_async {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::InvalidConstant,
                    fallback.range,
                    "fallback is only allowed on synchronous Rinzu functions",
                    "remove fallback or use a synchronous callback target",
                ));
            } else {
                match crate::fallback::evaluate_fallback(fallback, &signature.result) {
                    Ok(value) => {
                        let mut fallback_context =
                            FunctionContext::new(function, signature.result.clone(), false);
                        self.expect_expression(fallback, &signature.result, &mut fallback_context);
                        self.fallbacks.insert(function.name.clone(), value);
                    }
                    Err(error) => self.diagnostics.push(error),
                }
            }
        }
        if context.owner_sync && function.is_async {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::InvalidAwait,
                function.range,
                format!("OwnerSync handler `{}` cannot be async", function.name),
                "make the handler synchronous and hand work to a pre-existing application task",
            ));
        }
        if !context.owner_sync
            && signature
                .parameters
                .iter()
                .any(|ty| self.contains_call_only_type(ty, &mut HashSet::new()))
        {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::CallOnlyEscape,
                function.range,
                format!(
                    "CallOnly parameter is only valid on an OwnerSync handler such as `{}`",
                    function.name
                ),
                "mark this generated callback entry as OwnerSync in binding metadata",
            ));
        }
        for (parameter, ty) in function.parameters.iter().zip(&signature.parameters) {
            context
                .declared_names
                .insert(parameter.name.clone(), parameter.range);
            context.locals.insert(
                parameter.name.clone(),
                Local {
                    ty: ty.clone(),
                    mutable: false,
                    constant: None,
                    callback_contract: callback_type_contract(ty, Some(&parameter.ty)),
                    targets: if let Type::FunctionReference(expected) = ty {
                        self.compatible_reference_targets(expected)
                    } else {
                        Vec::new()
                    },
                },
            );
        }
        for (name, local) in captures {
            let mut local = local.clone();
            local.mutable = false;
            context.locals.insert(name.clone(), local);
        }
        self.check_block(&function.body, &mut context);
        if signature.result != Type::Unit && !block_always_returns(&function.body) {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::TypeMismatch,
                function.body.range,
                format!(
                    "function `{}` can finish without returning `{}`",
                    function.name, signature.result
                ),
                "add a value-returning `return` on every normal path",
            ));
        }
        self.direct_facts
            .insert(function.name.clone(), context.facts);
    }

    fn lexical_constants(
        &self,
        context: &FunctionContext<'_>,
    ) -> std::collections::BTreeMap<String, IntegerConstant> {
        let mut values = self.constants.clone();
        for (name, local) in &context.locals {
            if let Some(value) = local.constant {
                values.insert(name.clone(), value);
            } else {
                values.remove(name);
            }
        }
        values
    }

    fn check_literal(
        &mut self,
        literal: &Literal,
        expected: Option<&Type>,
        range: SourceRange,
    ) -> Option<Type> {
        match literal {
            Literal::Int(value) => self.check_integer_literal(*value as i128, expected, range),
            Literal::Unsigned(value) => self.check_integer_literal(*value as i128, expected, range),
            Literal::DecimalInteger(spelling) => {
                if let Some(ty) = expected
                    .filter(|ty| matches!(ty.scalar(), Some(AbiScalar::F32 | AbiScalar::F64)))
                {
                    Some(ty.clone())
                } else if let Ok(value) = spelling.parse::<i128>() {
                    self.check_integer_literal(value, expected, range)
                } else {
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::IntegerOutOfRange,
                        range,
                        "integer literal is outside supported integer ranges",
                        "use a representable integer or a floating expected type",
                    ));
                    None
                }
            }
            Literal::Float { .. } => Some(
                expected
                    .filter(|ty| matches!(ty.scalar(), Some(AbiScalar::F32 | AbiScalar::F64)))
                    .cloned()
                    .unwrap_or(Type::Float),
            ),
            Literal::Bool(_) => Some(Type::Bool),
            Literal::String(_) => Some(Type::String),
            Literal::Bytes(bytes) => Some(Type::FixedBytes(bytes.len() as u64)),
        }
    }
    fn check_integer_literal(
        &mut self,
        value: i128,
        expected: Option<&Type>,
        range: SourceRange,
    ) -> Option<Type> {
        let ty = expected
            .filter(|ty| ty.is_numeric())
            .cloned()
            .unwrap_or(Type::Int);
        if ty.is_integer()
            && let Err(error) = ty
                .scalar()
                .expect("integer scalar")
                .check_integer(value, range)
        {
            self.diagnostics.push(error);
            return None;
        }
        Some(ty)
    }
    fn numeric_hint(&self, expression: &Expression, context: &FunctionContext<'_>) -> Option<Type> {
        let ty = match &expression.kind {
            ExpressionKind::Name(path) => context
                .locals
                .get(&path[0])
                .map(|local| local.ty.clone())
                .or_else(|| {
                    self.constants
                        .get(&path.join("."))
                        .map(|value| Type::from_scalar(value.scalar))
                }),
            ExpressionKind::Call { target, .. } => self
                .functions
                .get(&target.join("."))
                .map(|function| function.result.clone())
                .or_else(|| {
                    self.receiver_method_name(target, context)
                        .and_then(|name| self.functions.get(&name))
                        .filter(|function| !function.is_async)
                        .map(|function| function.result.clone())
                })
                .or_else(|| {
                    self.source_signatures
                        .get(&target.join("."))
                        .filter(|signature| !signature.is_async)
                        .map(|signature| source_abi_type(&signature.result))
                })
                .or_else(|| {
                    self.options
                        .native_functions
                        .get(&target.join("."))
                        .map(|function| function.result.clone())
                })
                .or_else(|| {
                    (target.len() == 1)
                        .then(|| AbiScalar::from_name(&target[0]))
                        .flatten()
                        .map(Type::from_scalar)
                }),
            ExpressionKind::Unary { operand, .. } => self.numeric_hint(operand, context),
            ExpressionKind::Binary { left, right, .. } => self
                .numeric_hint(left, context)
                .or(self.numeric_hint(right, context)),
            _ => None,
        };
        ty.filter(|ty| ty.is_numeric())
    }

    #[inline(always)]
    fn check_expression(
        &mut self,
        expression: &Expression,
        expected: Option<&Type>,
        context: &mut FunctionContext<'_>,
    ) -> Option<Type> {
        self.check_expression_with_bound(expression, expected, None, context)
    }

    fn check_expression_with_bound(
        &mut self,
        expression: &Expression,
        expected: Option<&Type>,
        bound_site: Option<ExpectedErrorBound<'_>>,
        context: &mut FunctionContext<'_>,
    ) -> Option<Type> {
        let ty = match &expression.kind {
            ExpressionKind::Is { value, ty } => {
                self.check_error_test(value, ty, expression.range, context)
            }
            ExpressionKind::Function(function) => {
                self.check_lambda(function, expression.range, context)
            }
            ExpressionKind::Record(fields) => {
                self.check_record_literal(fields, expression.range, expected, context)
            }
            ExpressionKind::GenericCall { .. } | ExpressionKind::GenericReference { .. } => {
                unreachable!("generic calls expanded before checking")
            }
            ExpressionKind::Bind { arguments, .. } => {
                let binding = self.bindings.get(&expression.range)?.clone();
                if !self.method_visible(&binding.target, expression.range, context) {
                    return None;
                }
                let Arguments::Positional(values) = arguments else {
                    self.type_error(
                        expression.range,
                        "bind captures require positional arguments",
                    );
                    return None;
                };
                for ((name, ty), value) in binding.captures.iter().zip(values) {
                    self.expect_expression(value, ty, context);
                    if self.contains_call_only_type(ty, &mut HashSet::new()) {
                        self.diagnostics.push(Diagnostic::error(
                            DiagnosticCode::CallOnlyEscape,
                            value.range,
                            format!(
                                "capture `{name}` cannot escape; field path: {}",
                                self.capture_failure_path(ty, name, &mut HashSet::new())
                            ),
                            "capture an owned value with a valid same-Domain lifetime",
                        ));
                    }
                }
                self.reference_targets
                    .insert(expression.range, vec![binding.target]);
                Some(Type::FunctionReference(Box::new(binding.signature)))
            }
            ExpressionKind::Literal(literal) => {
                self.check_literal(literal, expected, expression.range)
            }
            ExpressionKind::FormatString(parts) => {
                for part in parts {
                    self.check_expression(part, None, context)?;
                }
                Some(Type::String)
            }
            ExpressionKind::Unit => Some(Type::Unit),
            ExpressionKind::Name(path) => {
                if crate::core::function_name(path) == Some("byteLength")
                    && !context.locals.contains_key(&path[0])
                    && !self.declarations.contains_key(&path.join("."))
                    && let Some(Type::FunctionReference(signature)) = expected
                    && let [parameter @ (Type::FixedBytes(_) | Type::Bytes)] =
                        signature.parameters.as_slice()
                    && !signature.supplant
                    && *signature.result == Type::Scalar(AbiScalar::U64)
                {
                    self.reference_targets.insert(
                        expression.range,
                        vec![format!(
                            "rinzu.core::byteLength:{}",
                            match parameter {
                                Type::FixedBytes(n) => n.to_string(),
                                _ => "dynamic".into(),
                            }
                        )],
                    );
                    self.static_references.insert(
                        expression.range,
                        self.reference_targets[&expression.range][0].clone(),
                    );
                    Some(Type::FunctionReference(signature.clone()))
                } else {
                    self.check_name(path, expression.range, context)
                }
            }
            ExpressionKind::Call { target, arguments } => {
                self.check_call(target, arguments, expression.range, context)
            }
            ExpressionKind::RecordUpdate { base, fields } => {
                let ty = self.check_expression(base, None, context)?;
                let Type::Named(identity) = &ty else {
                    self.type_error(base.range, "record update requires a language record");
                    return None;
                };
                let Some(Declaration::Record(record)) = self.declarations.get(identity).cloned()
                else {
                    self.type_error(base.range, "record update requires a language record");
                    return None;
                };
                if record.mutable || !identity.starts_with("$rinzu_record_") {
                    self.type_error(
                        base.range,
                        "record update requires a structural record; classes retain their identity",
                    );
                    return None;
                }
                let mut seen = HashSet::new();
                for (name, value, range) in fields {
                    if !seen.insert(name) {
                        self.type_error(*range, format!("duplicate update field `{name}`"));
                    }
                    if let Some(field) = record.fields.iter().find(|f| &f.name == name) {
                        let field_ty = self.resolve_type(&field.ty)?;
                        self.expect_expression(value, &field_ty, context);
                    } else {
                        self.type_error(*range, format!("unknown update field `{name}`"));
                    }
                }
                Some(ty)
            }
            ExpressionKind::List(values) => {
                self.check_list(values, expected, expression.range, context)
            }
            ExpressionKind::Map(values) => {
                self.check_map(values, expected, expression.range, context)
            }
            ExpressionKind::Unary { operator, operand } => {
                if *operator == UnaryOperator::Negate {
                    let literal_value = match &operand.kind {
                        ExpressionKind::Literal(Literal::Int(value)) => Some(*value as i128),
                        ExpressionKind::Literal(Literal::Unsigned(value)) => Some(*value as i128),
                        _ => None,
                    };
                    let hint = self
                        .numeric_hint(operand, context)
                        .or_else(|| expected.filter(|ty| ty.is_numeric()).cloned());
                    if let Some(value) = literal_value {
                        let ty = hint.clone().unwrap_or(Type::Int);
                        if ty.is_integer() {
                            let scalar = ty.scalar().expect("integer scalar");
                            if scalar.integer_bounds().is_some_and(|(min, _)| min == 0) {
                                self.invalid_operator(
                                    expression.range,
                                    "unsigned values do not support unary minus".into(),
                                );
                                return None;
                            }
                            if let Err(error) = scalar.check_integer(-value, expression.range) {
                                self.diagnostics.push(error);
                                return None;
                            }
                            self.expression_types.insert(operand.range, ty.clone());
                            self.expression_types.insert(expression.range, ty.clone());
                            return Some(ty);
                        }
                    }
                    let operand_type = self.check_expression(operand, hint.as_ref(), context)?;
                    if operand_type
                        .scalar()
                        .and_then(AbiScalar::integer_bounds)
                        .is_some_and(|(min, _)| min == 0)
                    {
                        self.invalid_operator(
                            expression.range,
                            "unsigned values do not support unary minus".into(),
                        );
                        None
                    } else if operand_type.is_numeric() {
                        context.facts.may_fail |= operand_type.is_integer();
                        Some(operand_type)
                    } else {
                        self.invalid_operator(
                            expression.range,
                            format!("operator is not defined for `{operand_type}`"),
                        );
                        None
                    }
                } else {
                    let operand_type = self.check_expression(operand, None, context)?;
                    if operand_type == Type::Bool {
                        Some(Type::Bool)
                    } else {
                        self.invalid_operator(
                            expression.range,
                            format!("operator is not defined for `{operand_type}`"),
                        );
                        None
                    }
                }
            }
            ExpressionKind::Binary {
                operator,
                left,
                right,
            } => self.check_binary(*operator, left, right, expected, context),
            ExpressionKind::Await(operand) => {
                if !context.function.is_async
                    || (context.in_finally && !context.in_task_cleanup)
                    || context.owner_sync
                {
                    self.diagnostics.push(Diagnostic::error(
                        if context.in_finally {
                            DiagnosticCode::InvalidFinally
                        } else {
                            DiagnosticCode::InvalidAwait
                        },
                        expression.range,
                        if context.in_finally {
                            "`await` is not allowed in `finally`"
                        } else if context.owner_sync {
                            "`await` is not allowed in an OwnerSync handler"
                        } else {
                            "`await` is only allowed in an async function"
                        },
                        "move the await into an async function body outside `finally`",
                    ));
                }
                context.facts.may_await = true;
                context.facts.may_fail = true;
                let previous = context.direct_await_operand.replace(operand.range);
                let checked = self.check_expression(operand, None, context);
                context.direct_await_operand = previous;
                if self.supplant_calls.contains_key(&operand.range) {
                    checked
                } else {
                    match checked {
                        Some(Type::Future(result)) => Some(*result),
                        Some(actual) => {
                            self.diagnostics.push(Diagnostic::error(
                                DiagnosticCode::InvalidAwait,
                                operand.range,
                                format!("`await` requires `Future<T>`, found `{actual}`"),
                                "await an async call or another Future value",
                            ));
                            None
                        }
                        None => None,
                    }
                }
            }
            ExpressionKind::Spawn { target, arguments } => {
                if context.in_finally && !context.in_task_cleanup {
                    self.invalid_finally(expression.range, "`spawn` is not allowed in `finally`");
                }
                if context.owner_sync {
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::InvalidSpawn,
                        expression.range,
                        "`spawn` is not allowed in an OwnerSync handler",
                        "start asynchronous work outside the synchronous callback",
                    ));
                }
                context.facts.may_spawn = true;
                context.facts.may_fail = true;
                let result =
                    self.check_function_call(target, arguments, expression.range, context, true);
                if let Some(result) = result {
                    if !self.is_transferable(&result, &mut HashSet::new()) {
                        self.diagnostics.push(Diagnostic::error(
                            DiagnosticCode::InvalidSpawn,
                            expression.range,
                            format!(
                                "spawn result type `{result}` is not Transferable; field path: {}",
                                self.sealed_failure_path(&result, "Transferable")
                            ),
                            "return only immutable Transferable values from a spawn callee",
                        ));
                    }
                    Some(Type::Future(Box::new(result)))
                } else {
                    None
                }
            }
        };
        let ty = if let (
            Some(Type::FunctionReference(actual)),
            Some(Type::FunctionReference(expected)),
        ) = (&ty, expected)
            && actual.same_signature(expected)
        {
            self.check_callback_bound(
                expression,
                actual.error_bound.as_deref(),
                expected.error_bound.as_deref(),
                bound_site,
                context,
            )
            .then(|| Type::FunctionReference(expected.clone()))
        } else {
            ty
        };
        let ty = if expected == Some(&Type::Error)
            && let Some(actual) = &ty
            && *actual != Type::Error
            && self.is_error_type(actual)
        {
            self.error_upcasts.insert(expression.range, actual.clone());
            Some(Type::Error)
        } else {
            ty
        };
        let ty = if let (Some(Type::FixedBytes(from)), Some(Type::FixedBytes(to))) = (&ty, expected)
        {
            if from > to {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::TypeMismatch,
                    expression.range,
                    format!("Bytes({from}) cannot fit Bytes({to})"),
                    "use a destination at least as long as the source",
                ));
                None
            } else {
                if from < to {
                    self.byte_coercions.insert(expression.range, (*from, *to));
                }
                Some(Type::FixedBytes(*to))
            }
        } else {
            ty
        };
        if let Some(ty) = &ty {
            self.expression_types.insert(expression.range, ty.clone());
        }
        ty
    }

    fn check_callback_bound(
        &mut self,
        expression: &Expression,
        actual: Option<&[String]>,
        expected: Option<&[String]>,
        site: Option<ExpectedErrorBound<'_>>,
        context: &FunctionContext<'_>,
    ) -> bool {
        let Some(expected) = expected else {
            return true;
        };
        match actual {
            Some(actual) if !crate::error_effects::within_bound(actual, expected) => {
                let related = site
                    .and_then(|site| self.expected_callback_contract(site, expected))
                    .into_iter()
                    .chain(self.actual_callback_contract(expression, actual, context))
                    .collect();
                self.diagnostics.push(conversion_error(
                    expression.range,
                    expected,
                    Some(actual),
                    related,
                ));
                false
            }
            None if expected != ["Error"] => {
                let related = site
                    .and_then(|site| self.expected_callback_contract(site, expected))
                    .into_iter()
                    .collect();
                self.error_conversions.insert(
                    expression.range,
                    ErrorConversion {
                        expected: expected.to_vec(),
                        related,
                    },
                );
                true
            }
            _ => true,
        }
    }

    fn expected_callback_contract(
        &self,
        site: ExpectedErrorBound<'_>,
        expected: &[String],
    ) -> Option<(SourceRange, String)> {
        match site {
            ExpectedErrorBound::Parameter { function, index } => {
                let Declaration::Function(declaration) = self.declarations.get(function)? else {
                    return None;
                };
                let parameter = declaration.parameters.get(index)?;
                // Alias expansion preserves the use's range but replaces its
                // kind. Recover the spelling in the selected declaration.
                let source = self.source_syntax.declarations.iter().find_map(|source| {
                    let Declaration::Function(source) = source else {
                        return None;
                    };
                    (source.range.file_id == declaration.range.file_id
                        && source.range.start_byte == declaration.range.start_byte
                        && source.range.end_byte == declaration.range.end_byte)
                        .then(|| source.parameters.get(index))
                        .flatten()
                });
                let spelling = source
                    .and_then(|source| match &source.ty.kind {
                        TypeRefKind::Named(path) => path
                            .last()
                            .map(|name| name.rsplit("::").next().unwrap().to_owned()),
                        TypeRefKind::Generic { .. } => {
                            Some(crate::interfaces::spelling(&source.ty).replace("::", "."))
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| display_bound(expected));
                Some((
                    parameter.ty.range,
                    format!("parameter `{}` expects `{spelling}`", parameter.name),
                ))
            }
            ExpectedErrorBound::Annotation(range) => Some((
                range,
                format!(
                    "this type annotation requires `{}`",
                    display_bound(expected)
                ),
            )),
            ExpectedErrorBound::Result(range) => Some((
                range,
                format!("this return type requires `{}`", display_bound(expected)),
            )),
        }
    }

    fn actual_callback_contract(
        &self,
        expression: &Expression,
        actual: &[String],
        context: &FunctionContext<'_>,
    ) -> Option<(SourceRange, String)> {
        let actual_note = match &expression.kind {
            ExpressionKind::Function(function) => function
                .error_contract
                .as_ref()
                .map(|contract| (contract.range, "this callback".to_owned())),
            ExpressionKind::Name(path)
                if path.len() == 1 && context.locals.contains_key(&path[0]) =>
            {
                context.locals[&path[0]]
                    .callback_contract
                    .map(|range| (range, format!("`{}`", path[0])))
            }
            _ => self
                .static_references
                .get(&expression.range)
                .or_else(|| {
                    self.bindings
                        .get(&expression.range)
                        .map(|binding| &binding.target)
                })
                .and_then(|name| match self.declarations.get(name)? {
                    Declaration::Function(function) => {
                        function.error_contract.as_ref().map(|contract| {
                            (
                                contract.range,
                                format!("`{}`", function.name.replace("::", ".")),
                            )
                        })
                    }
                    _ => None,
                }),
        };
        actual_note.map(|(range, name)| {
            (
                range,
                format!("{name} declares `{}` here", display_bound(actual)),
            )
        })
    }

    fn callback_call_contract(
        &self,
        target: &[String],
        context: &FunctionContext<'_>,
    ) -> Option<SourceRange> {
        let (root, fields) = target.split_first()?;
        let local = context.locals.get(root)?;
        let Some((last, fields)) = fields.split_last() else {
            return local.callback_contract;
        };
        let field = |ty: &Type, field: &str| {
            let Type::Named(name) = ty else { return None };
            self.record_declaration(name)?
                .fields
                .iter()
                .find(|f| f.name == field)
        };
        let mut ty = local.ty.clone();
        for name in fields {
            ty = self.resolve_type_readonly(&field(&ty, name)?.ty)?;
        }
        let annotation = &field(&ty, last)?.ty;
        callback_type_contract(&self.resolve_type_readonly(annotation)?, Some(annotation))
    }

    fn check_name(
        &mut self,
        path: &[String],
        range: SourceRange,
        context: &FunctionContext<'_>,
    ) -> Option<Type> {
        let first = path.first()?;
        if !context.locals.contains_key(first)
            && let Some(value) = self.constants.get(&path.join("."))
        {
            self.constant_references.insert(range, *value);
            return Some(Type::from_scalar(value.scalar));
        }
        if let Some(local) = context.locals.get(first) {
            let mut ty = context
                .narrowed_errors
                .get(first)
                .cloned()
                .unwrap_or_else(|| local.ty.clone());
            if ty != local.ty {
                self.error_narrowings.insert(range, ty.clone());
            }
            for field in &path[1..] {
                if self.is_error_type(&ty) {
                    ty = self.error_field_type(&ty, field, range, &context.function.name)?;
                    continue;
                }
                let method_owner = match &ty {
                    Type::Named(name) | Type::Native { name, .. } => Some(name),
                    _ => None,
                };
                if let Some(owner) = method_owner {
                    let method = format!("{owner}.{field}");
                    if self.functions.contains_key(&method) {
                        self.diagnostics.push(Diagnostic::error(
                            DiagnosticCode::InvalidHandlerReference,range,
                            format!("method `{method}` needs an explicit call or fn expression"),
                            format!("write receiver.{field}(...) or fn(arguments) {{ return receiver.{field}(arguments); }}; field access does not call a getter"),
                        ));
                        return None;
                    }
                }
                let Type::Named(name) = &ty else {
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::UnknownName,
                        range,
                        format!("type `{ty}` has no field `{field}`"),
                        "access a field declared on a record value",
                    ));
                    return None;
                };
                let Some(record) = self.record_declaration(name) else {
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::UnknownName,
                        range,
                        format!("type `{name}` has no fields"),
                        "use a record value for field access",
                    ));
                    return None;
                };
                let Some(declaration) = record
                    .fields
                    .iter()
                    .find(|candidate| &candidate.name == field)
                else {
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::UnknownName,
                        range,
                        format!("record `{name}` has no field `{field}`"),
                        "use a field declared on the record",
                    ));
                    return None;
                };
                let field_type = declaration.ty.clone();
                if declaration.is_private && !self.owner_access(name, context) {
                    self.private_field_error(field, range);
                    return None;
                }
                if self.module.native_blocks.iter().flat_map(|block| &block.members).any(|member| matches!(member, crate::ast::NativeMember::Record(record) if record.name == *name)) {
                    self.native_record_reads.insert(range);
                }
                ty = self.resolve_type(&field_type)?;
            }
            if let Type::FunctionReference(signature) = &ty {
                self.reference_targets.insert(
                    range,
                    if path.len() == 1 && !local.targets.is_empty() {
                        local.targets.clone()
                    } else {
                        self.compatible_reference_targets(signature)
                    },
                );
            }
            return Some(ty);
        }
        let mut name = path.join(".");
        if !self.declarations.contains_key(&name)
            && crate::core::function_name(path) == Some("utf8ByteLength")
        {
            name = "rinzu.core::utf8ByteLength".to_owned();
        }
        if let Some(signature) = self.reference_signature(&name) {
            if !self.method_visible(&name, range, context) {
                return None;
            }
            self.static_references.insert(range, name.clone());
            self.reference_targets.insert(range, vec![name]);
            return Some(Type::FunctionReference(Box::new(signature)));
        }
        self.unknown_name(&path.join("."), range);
        None
    }

    fn reference_signature(&self, name: &str) -> Option<FunctionReferenceType> {
        if let Some(signature) = crate::core::reference_signature(name) {
            return Some(signature);
        }
        if let Some(signature) = self.functions.get(name) {
            return Some(FunctionReferenceType {
                parameters: signature.parameters.clone(),
                result: Box::new(if signature.is_async {
                    Type::Future(Box::new(signature.result.clone()))
                } else {
                    signature.result.clone()
                }),
                supplant: false,
                error_bound: self.reference_bound(signature),
            });
        }
        self.source_signatures
            .get(name)
            .map(|signature| FunctionReferenceType {
                parameters: signature.parameters.iter().map(source_abi_type).collect(),
                result: Box::new(if signature.is_async {
                    Type::Future(Box::new(source_abi_type(&signature.result)))
                } else {
                    source_abi_type(&signature.result)
                }),
                supplant: signature.is_supplant,
                error_bound: None,
            })
    }

    fn reference_bound(&self, signature: &FunctionSignature) -> Option<Vec<String>> {
        if self.closure_conversion || signature.is_async {
            None
        } else {
            signature.error_bound.clone()
        }
    }

    fn compatible_reference_targets(&self, expected: &FunctionReferenceType) -> Vec<String> {
        let compatible = |actual: &FunctionReferenceType| {
            actual.same_signature(expected)
                && match (&actual.error_bound, &expected.error_bound) {
                    (Some(actual), Some(expected)) => {
                        crate::error_effects::within_bound(actual, expected)
                    }
                    _ => true,
                }
        };
        let mut targets: Vec<_> = self
            .functions
            .keys()
            .chain(self.source_signatures.keys())
            .filter(|name| {
                self.reference_signature(name)
                    .as_ref()
                    .is_some_and(compatible)
            })
            .cloned()
            .collect();
        targets.extend(
            self.bindings
                .values()
                .filter(|binding| compatible(&binding.signature))
                .map(|binding| binding.target.clone()),
        );
        targets.sort();
        if crate::core::reference_signature("rinzu.core::utf8ByteLength")
            .as_ref()
            .is_some_and(compatible)
        {
            targets.push("rinzu.core::utf8ByteLength".to_owned());
            targets.sort();
        }
        if let [parameter @ (Type::FixedBytes(_) | Type::Bytes)] = expected.parameters.as_slice()
            && !expected.supplant
            && *expected.result == Type::Scalar(AbiScalar::U64)
        {
            targets.push(format!(
                "rinzu.core::byteLength:{}",
                match parameter {
                    Type::FixedBytes(n) => n.to_string(),
                    _ => "dynamic".into(),
                }
            ));
            targets.sort();
        }
        targets.dedup();
        targets
    }

    fn type_error(&mut self, range: SourceRange, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic::error(
            DiagnosticCode::TypeMismatch,
            range,
            message,
            "use the documented data operation signature",
        ));
    }

    fn record_declaration(&self, name: &str) -> Option<&crate::ast::RecordDecl> {
        if let Some(Declaration::Record(record)) = self.declarations.get(name) {
            return Some(record);
        }
        self.module
            .native_blocks
            .iter()
            .flat_map(|block| &block.members)
            .find_map(|member| match member {
                crate::ast::NativeMember::Record(record) if record.name == name => Some(record),
                _ => None,
            })
    }

    fn ensure_option(&mut self, payload: &Type, range: SourceRange, scope: &str) -> Option<Type> {
        use crate::ast::{EnumDecl, EnumVariant, FieldDecl};
        let name = crate::core::option_name(payload);
        if self.option_shadow_modules.contains("")
            || self
                .option_shadow_modules
                .contains(crate::methods::module_name(scope))
            || self.declarations.get(&name).is_some_and(|declaration| {
                let Declaration::Enum(value) = declaration else {
                    return true;
                };
                value.variants.len() != 2
                    || value.variants[0].name != "None"
                    || !value.variants[0].fields.is_empty()
                    || value.variants[1].name != "Some"
                    || value.variants[1].fields.len() != 1
                    || self
                        .resolve_type_readonly(&value.variants[1].fields[0].ty)
                        .as_ref()
                        != Some(payload)
            })
            || self.declarations.contains_key("Option")
        {
            self.type_error(
                range,
                "standard data API requires the prelude Option; a local declaration shadows it",
            );
            return None;
        }
        if !self.declarations.contains_key(&name) {
            let payload = self.generated_type_ref(payload, range);
            self.declarations.insert(
                name.clone(),
                Declaration::Enum(EnumDecl {
                    interfaces: Vec::new(),
                    name: name.clone(),
                    type_parameters: Vec::new(),
                    range,
                    variants: vec![
                        EnumVariant {
                            name: "None".to_owned(),
                            fields: Vec::new(),
                            range,
                        },
                        EnumVariant {
                            name: "Some".to_owned(),
                            fields: vec![FieldDecl {
                                is_const: true,
                                is_private: false,
                                name: "value".to_owned(),
                                ty: payload,
                                range,
                            }],
                            range,
                        },
                    ],
                }),
            );
        }
        Some(Type::Named(name))
    }

    fn owner_access(&self, owner: &str, context: &FunctionContext<'_>) -> bool {
        self.closure_conversion
            || context
                .function
                .lexical_owner
                .as_deref()
                .is_some_and(|lexical| lexical.split('<').next() == owner.split('<').next())
    }

    fn private_field_error(&mut self, field: &str, range: SourceRange) {
        self.diagnostics.push(Diagnostic::error(
            DiagnosticCode::UnknownName,
            range,
            format!("private field `{field}` can only be used by methods of its owner type"),
            "use a public method of the owning class",
        ));
    }

    fn method_visible(
        &mut self,
        name: &str,
        range: SourceRange,
        context: &FunctionContext<'_>,
    ) -> bool {
        // Generated dispatch code only references targets checked in the source pass.
        if self.closure_conversion
            || crate::methods::parts(name)
                .is_some_and(|(_, member)| member.starts_with("$interface_"))
        {
            return true;
        }
        if let Some(Declaration::Function(function)) = self.declarations.get(name)
            && function.is_private
        {
            if function.lexical_owner.is_some()
                && function.lexical_owner == context.function.lexical_owner
            {
                return true;
            }
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::UnknownName,
                range,
                format!("private method `{name}` can only be used by methods of its owner type"),
                "access the private method from a method of the same type",
            ));
            return false;
        }
        if crate::methods::parts(name).is_none()
            || crate::methods::module_name(name)
                == crate::methods::module_name(&context.function.name)
            || self.options.exported_methods.contains(name)
            || self
                .options
                .exported_methods
                .contains(&crate::methods::template_name(name))
            || self
                .module
                .exports
                .iter()
                .any(|export| export.path.join(".") == name)
        {
            return true;
        }
        self.diagnostics.push(Diagnostic::error(
            DiagnosticCode::UnknownName,
            range,
            format!("method `{name}` is not exported from its owner module"),
            "export the owner type before using its non-private methods from another module",
        ));
        false
    }

    fn check_list(
        &mut self,
        values: &[Expression],
        expected: Option<&Type>,
        range: SourceRange,
        context: &mut FunctionContext<'_>,
    ) -> Option<Type> {
        context.facts.may_fail = true;
        let contextual = match expected {
            Some(Type::List(element)) => Some(element.as_ref().clone()),
            _ => None,
        };
        if values.is_empty() && contextual.is_none() {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::TypeMismatch,
                range,
                "an empty list needs an explicit contextual type",
                "add a local annotation such as `let values: List<Int> = []`",
            ));
            return None;
        }
        let element = if let Some(value) = contextual {
            value
        } else {
            self.check_expression(&values[0], None, context)?
        };
        for value in values {
            self.expect_expression(value, &element, context);
        }
        Some(Type::List(Box::new(element)))
    }

    fn generated_range(&mut self, mut range: SourceRange) -> SourceRange {
        range.instance = self.generated_instance;
        self.generated_instance += 1;
        range
    }
    fn generated_type_ref(&mut self, ty: &Type, range: SourceRange) -> TypeRef {
        struct Unique<'a>(&'a mut u32);
        impl crate::ast_walk::Visitor for Unique<'_> {
            fn ty(&mut self, ty: &mut TypeRef) {
                ty.range.instance = *self.0;
                *self.0 += 1;
            }
        }
        let mut reference = crate::core::type_ref(ty, range);
        crate::ast_walk::ty(&mut reference, &mut Unique(&mut self.generated_instance));
        self.resolve_type(&reference);
        reference
    }
    fn check_lambda(
        &mut self,
        original: &FunctionDecl,
        range: SourceRange,
        context: &mut FunctionContext<'_>,
    ) -> Option<Type> {
        let mut parameters_seen = HashSet::new();
        for parameter in &original.parameters {
            if !parameters_seen.insert(&parameter.name) {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::DuplicateName,
                    parameter.range,
                    format!("duplicate function parameter `{}`", parameter.name),
                    "give each parameter a distinct name",
                ));
                return None;
            }
        }
        let captures =
            crate::lambdas::captures(original, &context.locals.keys().cloned().collect());
        let mut function = original.clone();
        function.lexical_owner = context.function.lexical_owner.clone();
        let namespace = crate::methods::module_name(&context.function.name);
        function.name = format!(
            "{namespace}::$rinzu_lambda_{}_{}_{}",
            range.file_id.0, range.start_byte, range.instance
        );
        let mut names = HashMap::new();
        let mut capture_locals = HashMap::new();
        let mut parameters = Vec::new();
        let mut arguments = Vec::new();
        let mut capture_types = Vec::new();
        for (index, (name, _)) in captures.into_iter().enumerate() {
            let mut local = context.locals[&name].clone();
            if let Some(narrowed) = context.narrowed_errors.get(&name) {
                local.ty = narrowed.clone();
            }
            if self.contains_call_only_type(&local.ty, &mut HashSet::new()) {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::CallOnlyEscape,
                    range,
                    format!(
                        "capture `{name}` cannot escape; field path: {}",
                        self.capture_failure_path(&local.ty, &name, &mut HashSet::new())
                    ),
                    "capture an owned value with a valid same-Domain lifetime",
                ));
                return None;
            }
            let capture = format!("$capture{index}");
            names.insert(name.clone(), capture.clone());
            let parameter_range = self.generated_range(range);
            parameters.push(crate::ast::Parameter {
                name: capture.clone(),
                ty: self.generated_type_ref(&local.ty, parameter_range),
                range: parameter_range,
            });
            arguments.push(Expression {
                kind: ExpressionKind::Name(vec![name.clone()]),
                range: self.generated_range(range),
            });
            capture_types.push((name, local.ty.clone()));
            capture_locals.insert(capture, local);
        }
        let mut constants = self.lexical_constants(context);
        for parameter in &original.parameters {
            constants.remove(&parameter.name);
        }
        let mut parameter_types = parameters
            .iter()
            .map(|p| self.resolve_type(&p.ty))
            .collect::<Option<Vec<_>>>()?;
        let mut source_parameters = Vec::new();
        for mut parameter in function.parameters.clone() {
            let ty = self.resolve_type_with_constants(&parameter.ty, &constants)?;
            parameter.ty = self.generated_type_ref(&ty, parameter.ty.range);
            parameter_types.push(ty.clone());
            source_parameters.push(ty);
            parameters.push(parameter);
        }
        let result = self.resolve_type_with_constants(&function.result, &constants)?;
        let error_bound = function
            .error_contract
            .as_ref()
            .map(|contract| self.resolve_error_bound(contract, &constants));
        function.result = self.generated_type_ref(&result, function.result.range);
        crate::lambdas::rewrite(&mut function, &names);
        function.parameters = parameters;
        self.functions.insert(
            function.name.clone(),
            FunctionSignature {
                parameters: parameter_types,
                result: result.clone(),
                is_async: function.is_async,
                error_bound,
                range,
            },
        );
        self.declarations.insert(
            function.name.clone(),
            Declaration::Function(function.clone()),
        );
        let signature = FunctionReferenceType {
            parameters: source_parameters,
            result: Box::new(if function.is_async {
                Type::Future(Box::new(result))
            } else {
                result
            }),
            supplant: false,
            error_bound: self.reference_bound(&self.functions[&function.name]),
        };
        self.bindings.insert(
            range,
            BoundFunction {
                target: function.name.clone(),
                signature: signature.clone(),
                captures: capture_types,
            },
        );
        self.check_function(&function, &capture_locals);
        let expression = Expression {
            range,
            kind: ExpressionKind::Bind {
                target: vec![function.name],
                type_arguments: Vec::new(),
                arguments: Arguments::Positional(arguments),
            },
        };
        self.lambda_replacements
            .insert(range, expression.kind.clone());
        self.check_expression(
            &expression,
            Some(&Type::FunctionReference(Box::new(signature))),
            context,
        )
    }

    fn unsupported_json_type(&self, ty: &Type) -> Option<String> {
        match ty {
            Type::Bool | Type::Int | Type::Float | Type::String => None,
            Type::List(element) => self
                .unsupported_json_type(element)
                .map(|path| format!("list element / {path}")),
            Type::Map(key, value) if **key == Type::String => self
                .unsupported_json_type(value)
                .map(|path| format!("map value / {path}")),
            Type::Named(name) if name.starts_with("$rinzu_record_") => {
                let record = self.record_declaration(name)?;
                record.fields.iter().find_map(|field| {
                    self.unsupported_json_type(&self.resolve_type_readonly(&field.ty)?)
                        .map(|path| format!("field `{}` / {path}", field.name))
                })
            }
            Type::Named(name) if name.starts_with("Option<") => {
                if let Some(Declaration::Enum(enumeration)) = self.declarations.get(name)
                    && let [none, some] = enumeration.variants.as_slice()
                    && none.name == "None"
                    && some.name == "Some"
                    && let [field] = some.fields.as_slice()
                    && let Some(child) = self.resolve_type_readonly(&field.ty)
                {
                    return self
                        .unsupported_json_type(&child)
                        .map(|path| format!("option value / {path}"));
                }
                Some(format!("`{ty}`"))
            }
            _ => Some(format!("`{ty}`")),
        }
    }

    fn intern_record(
        &mut self,
        mut fields: Vec<(String, Type)>,
        range: SourceRange,
    ) -> Option<Type> {
        fields.sort_by(|a, b| a.0.cmp(&b.0));
        let identity = crate::records::identity(&fields);
        if let Some(Declaration::Record(record)) = self.declarations.get(&identity) {
            let existing = record
                .fields
                .iter()
                .map(|f| Some((f.name.clone(), self.resolve_type_readonly(&f.ty)?)))
                .collect::<Option<Vec<_>>>()?;
            if existing != fields {
                self.type_error(range, "structural type identity collision");
                return None;
            }
        } else {
            let fields = fields
                .into_iter()
                .map(|(name, ty)| crate::ast::FieldDecl {
                    name,
                    is_const: true,
                    is_private: false,
                    ty: self.generated_type_ref(&ty, range),
                    range,
                })
                .collect();
            self.declarations.insert(
                identity.clone(),
                Declaration::Record(crate::ast::RecordDecl {
                    interfaces: Vec::new(),
                    name: identity.clone(),
                    mutable: false,
                    type_parameters: Vec::new(),
                    range,
                    fields,
                }),
            );
        }
        Some(Type::Named(identity))
    }

    fn check_record_literal(
        &mut self,
        fields: &[crate::ast::RecordField],
        range: SourceRange,
        expected: Option<&Type>,
        context: &mut FunctionContext<'_>,
    ) -> Option<Type> {
        context.facts.may_fail = true;
        let expected_fields = match expected {
            Some(Type::Named(name)) if name.starts_with("$rinzu_record_") => {
                self.record_declaration(name).map(|r| r.fields.clone())
            }
            _ => None,
        };
        let mut names = HashSet::new();
        let mut types = Vec::new();
        for field in fields {
            if !names.insert(field.name.clone()) {
                self.type_error(
                    field.range,
                    format!("duplicate record field `{}`", field.name),
                );
                continue;
            }
            let contextual = expected_fields
                .as_ref()
                .and_then(|fields| fields.iter().find(|f| f.name == field.name))
                .and_then(|f| self.resolve_type(&f.ty));
            let explicit = field.annotation.as_ref().and_then(|t| {
                self.resolve_type_with_constants(t, &self.lexical_constants(context))
            });
            if let (Some(a), Some(b)) = (&contextual, &explicit) {
                self.require_same(a, b, field.range);
            }
            let hint = explicit.as_ref().or(contextual.as_ref());
            fn untyped_number(e: &Expression) -> bool {
                matches!(
                    &e.kind,
                    ExpressionKind::Literal(
                        Literal::Int(_)
                            | Literal::Unsigned(_)
                            | Literal::DecimalInteger(_)
                            | Literal::Float { .. }
                    )
                ) || matches!(&e.kind,ExpressionKind::Unary{operator:UnaryOperator::Negate,operand} if untyped_number(operand))
            }
            if hint.is_none() && untyped_number(&field.value) {
                self.type_error(field.value.range,format!("numeric record field `{}` requires a field annotation or an expected record type",field.name));
                return None;
            }
            let actual = self.check_expression(&field.value, hint, context)?;
            if let Some(expected) = hint {
                self.require_same(expected, &actual, field.value.range);
            }
            if self.contains_call_only_type(&actual, &mut HashSet::new()) {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::CallOnlyEscape,
                    field.value.range,
                    format!(
                        "borrowed value cannot escape into record field `{}`",
                        field.name
                    ),
                    "store an owned value",
                ));
            }
            types.push((field.name.clone(), actual));
        }
        if let Some(expected_fields) = expected_fields {
            let missing = expected_fields
                .iter()
                .filter(|f| !names.contains(&f.name))
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                self.type_error(
                    range,
                    format!("record is missing fields: {}", missing.join(", ")),
                );
            }
            for name in &names {
                if !expected_fields.iter().any(|f| &f.name == name) {
                    self.type_error(range, format!("record has extra field `{name}`"));
                }
            }
        }
        if fields.is_empty() {
            self.type_error(range, "record values require at least one field");
            return None;
        }
        let ty = self.intern_record(types, range)?;
        if let Type::Named(name) = &ty {
            self.record_literals.insert(range, name.clone());
        }
        Some(ty)
    }

    fn check_map(
        &mut self,
        entries: &[(Expression, Expression)],
        expected: Option<&Type>,
        range: SourceRange,
        context: &mut FunctionContext<'_>,
    ) -> Option<Type> {
        if entries.is_empty()
            && matches!(expected,Some(Type::Named(name)) if name.starts_with("$rinzu_record_"))
        {
            return self.check_record_literal(&[], range, expected, context);
        }
        context.facts.may_fail = true;
        let contextual = match expected {
            Some(Type::Map(key, value)) => Some((key.as_ref().clone(), value.as_ref().clone())),
            _ => None,
        };
        if entries.is_empty() && contextual.is_none() {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::TypeMismatch,
                range,
                "an empty map needs an explicit contextual type",
                "add a local annotation such as `let values: Map<String, Int> = {}`",
            ));
            return None;
        }
        let (key, value) = if let Some(types) = contextual {
            types
        } else {
            (
                self.check_expression(&entries[0].0, None, context)?,
                self.check_expression(&entries[0].1, None, context)?,
            )
        };
        if !self.is_key(&key, &mut HashSet::new()) {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::TypeMismatch,
                range,
                format!("map key type `{key}` does not implement sealed `Key`; field path: {}", self.sealed_failure_path(&key, "Key")),
                "use integers, Bool, String, bytes, or language record/enum containing only Key fields",
            ));
        }
        for (entry_key, entry_value) in entries {
            self.expect_expression(entry_key, &key, context);
            self.expect_expression(entry_value, &value, context);
        }
        Some(Type::Map(Box::new(key), Box::new(value)))
    }

    fn check_binary(
        &mut self,
        operator: BinaryOperator,
        left: &Expression,
        right: &Expression,
        expected: Option<&Type>,
        context: &mut FunctionContext<'_>,
    ) -> Option<Type> {
        let hint = self
            .numeric_hint(left, context)
            .or(self.numeric_hint(right, context))
            .or_else(|| expected.filter(|ty| ty.is_numeric()).cloned());
        let left_type = self.check_expression(left, hint.as_ref(), context)?;
        let previous = context.narrowed_errors.clone();
        if matches!(operator, BinaryOperator::And | BinaryOperator::Or) {
            context
                .narrowed_errors
                .extend(self.error_condition_refinements(
                    left,
                    operator == BinaryOperator::And,
                    context,
                ));
        }
        let right_type = self.check_expression(right, Some(&left_type), context);
        context.narrowed_errors = previous;
        let right_type = right_type?;
        if left_type != right_type {
            self.require_same(&left_type, &right_type, right.range);
            return None;
        }
        match operator {
            BinaryOperator::Or | BinaryOperator::And if left_type == Type::Bool => Some(Type::Bool),
            BinaryOperator::Equal | BinaryOperator::NotEqual
                if self.has_equality(&left_type, &mut HashSet::new()) =>
            {
                Some(Type::Bool)
            }
            BinaryOperator::Less
            | BinaryOperator::LessEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterEqual
                if left_type.is_numeric() =>
            {
                Some(Type::Bool)
            }
            BinaryOperator::Add
            | BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
                if left_type.is_numeric() =>
            {
                context.facts.may_fail |= left_type.is_integer();
                Some(left_type)
            }
            BinaryOperator::Remainder if left_type.is_integer() => {
                context.facts.may_fail = true;
                Some(left_type)
            }
            _ => {
                self.invalid_operator(
                    left.range.join(right.range),
                    format!("operator is not defined for `{left_type}` operands"),
                );
                None
            }
        }
    }

    fn expect_expression(
        &mut self,
        expression: &Expression,
        expected: &Type,
        context: &mut FunctionContext<'_>,
    ) {
        if let Some(actual) = self.check_expression(expression, Some(expected), context) {
            self.require_same(expected, &actual, expression.range);
        }
    }

    fn require_same(&mut self, expected: &Type, actual: &Type, range: SourceRange) {
        if expected != actual
            && *actual != Type::Never
            && !(*expected == Type::Error && self.is_error_type(actual))
        {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::TypeMismatch,
                range,
                format!("expected `{expected}`, found `{actual}`"),
                "change the expression or its declared type so both types match exactly",
            ));
        }
    }

    fn infer_facts(&self) -> HashMap<String, FunctionFacts> {
        let mut result = HashMap::new();
        for (name, signature) in &self.functions {
            let direct = self.direct_facts.get(name).cloned().unwrap_or_default();
            result.insert(
                name.clone(),
                FunctionFacts {
                    is_async: signature.is_async,
                    may_fail: direct.may_fail || matches!(self.declarations.get(name), Some(Declaration::Function(f)) if f.error_contract.as_ref().is_some_and(|c| !c.types.is_empty())),
                    may_await: direct.may_await,
                    may_spawn: direct.may_spawn,
                    requires_owner: direct.requires_owner,
                    worker_safe: !direct.requires_owner
                        && !signature.is_async
                        && !direct.may_await
                        && !direct.may_spawn
                        && !direct.has_native,
                    cleanup_safe: !signature.is_async
                        && !direct.may_await
                        && !direct.may_spawn
                        && !direct.has_cleanup_unsafe,
                    native_requirements: {
                        let mut values: Vec<_> =
                            direct.native_requirements.iter().cloned().collect();
                        values.sort();
                        values
                    },
                },
            );
        }
        let mut changed = true;
        while changed {
            changed = false;
            for name in self.functions.keys() {
                let direct = self.direct_facts.get(name).cloned().unwrap_or_default();
                let mut next = result.get(name).cloned().unwrap_or_default();
                for callee in &direct.calls {
                    if let Some(callee_facts) = result.get(callee) {
                        next.may_fail |= callee_facts.may_fail;
                        next.may_await |= callee_facts.may_await;
                        next.may_spawn |= callee_facts.may_spawn;
                        next.requires_owner |= callee_facts.requires_owner;
                        next.worker_safe &=
                            callee_facts.worker_safe && !callee_facts.requires_owner;
                        next.cleanup_safe &= callee_facts.cleanup_safe;
                        next.native_requirements
                            .extend(callee_facts.native_requirements.iter().cloned());
                        next.native_requirements.sort();
                        next.native_requirements.dedup();
                    } else if crate::core::reference_signature(callee).is_none() {
                        next.worker_safe = false;
                    }
                }
                for callee in direct.cleanup_calls.keys() {
                    next.cleanup_safe &= crate::core::reference_signature(callee).is_some()
                        || result.get(callee).is_some_and(|facts| facts.cleanup_safe);
                }
                if result.get(name) != Some(&next) {
                    result.insert(name.clone(), next);
                    changed = true;
                }
            }
        }
        result
    }

    fn validate_cleanup_targets(&mut self, facts: &HashMap<String, FunctionFacts>) {
        let invalid: Vec<_> = self
            .direct_facts
            .values()
            .flat_map(|direct| &direct.cleanup_calls)
            .filter(|(target, _)| crate::core::reference_signature(target).is_none())
            .filter(|(callee, _)| !facts.get(*callee).is_some_and(|f| f.cleanup_safe))
            .map(|(callee, range)| (callee.clone(), *range))
            .collect();
        for (callee, range) in invalid {
            self.invalid_finally(range, &format!("callee `{callee}` is not CleanupSafe"));
        }
    }

    fn worker_indirect_failure(
        &self,
        facts: &HashMap<String, FunctionFacts>,
        name: &str,
        visiting: &mut HashSet<String>,
    ) -> Option<(String, SourceRange, String)> {
        if !visiting.insert(name.to_owned()) {
            return None;
        }
        let direct = self.direct_facts.get(name)?;
        fn bad(
            facts: &HashMap<String, FunctionFacts>,
            sources: &HashMap<String, crate::SourceNativeSignature>,
            name: &str,
        ) -> bool {
            facts.get(name).is_some_and(|f| !f.worker_safe)
                || sources
                    .get(name)
                    .is_some_and(|f| f.is_async || f.is_supplant)
        }
        fn concrete(
            checker: &Checker<'_>,
            facts: &HashMap<String, FunctionFacts>,
            name: &str,
            seen: &mut HashSet<String>,
        ) -> String {
            if !name.starts_with("$rinzu_closure_") || !seen.insert(name.to_owned()) {
                return name.to_owned();
            }
            let mut calls = checker
                .direct_facts
                .get(name)
                .map(|f| f.calls.iter().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            calls.sort();
            for call in calls {
                if bad(facts, &checker.source_signatures, &call) {
                    return concrete(checker, facts, &call, seen);
                }
            }
            name.to_owned()
        }
        for (path, range, targets) in &direct.indirect_sites {
            for target in targets {
                if bad(facts, &self.source_signatures, target) {
                    return Some((
                        path.clone(),
                        *range,
                        concrete(self, facts, target, &mut HashSet::new()),
                    ));
                }
            }
        }
        let mut calls = direct.calls.iter().collect::<Vec<_>>();
        calls.sort();
        for call in calls {
            if let Some(path) = self.worker_indirect_failure(facts, call, visiting) {
                return Some(path);
            }
        }
        None
    }

    fn validate_spawn_targets(&mut self, facts: &HashMap<String, FunctionFacts>) {
        for direct in self.direct_facts.values() {
            for (callee, range) in &direct.spawn_calls {
                if !facts.get(callee).is_some_and(|value| value.worker_safe) {
                    let mut diagnostic = Diagnostic::error(
                        DiagnosticCode::InvalidSpawn,
                        *range,
                        format!("spawn callee `{callee}` is not WorkerSafe"),
                        "remove await, spawn, native/Main-Domain calls, and async callees from its call graph",
                    );
                    if let Some((path, site, target)) =
                        self.worker_indirect_failure(facts, callee, &mut HashSet::new())
                    {
                        diagnostic
                            .message
                            .push_str(&format!("; field path: {path} -> target `{target}`"));
                        diagnostic = diagnostic
                            .with_secondary(site, "indirect call includes this target's effects");
                    }
                    self.diagnostics.push(diagnostic);
                }
            }
        }
    }

    fn unknown_name(&mut self, name: &str, range: SourceRange) {
        self.diagnostics.push(Diagnostic::error(
            DiagnosticCode::UnknownName,
            range,
            format!("unknown name `{name}`"),
            "declare the name or qualify it through an imported module",
        ));
    }

    fn invalid_operator(&mut self, range: SourceRange, message: String) {
        self.diagnostics.push(Diagnostic::error(
            DiagnosticCode::InvalidOperator,
            range,
            message,
            "use an operator defined for two operands of exactly the same supported type",
        ));
    }

    fn invalid_finally(&mut self, range: SourceRange, message: &str) {
        self.diagnostics.push(Diagnostic::error(
            DiagnosticCode::InvalidFinally,
            range,
            message,
            "keep `finally` synchronous and allow it to fall through normally",
        ));
    }
}

struct FunctionContext<'a> {
    caught_bindings: HashMap<String, errors::CatchSource>,
    all_depth: usize,
    narrowed_errors: HashMap<String, Type>,
    function: &'a FunctionDecl,
    result: Type,
    locals: HashMap<String, Local>,
    declared_names: HashMap<String, SourceRange>,
    loop_depth: usize,
    in_finally: bool,
    in_task_cleanup: bool,
    owner_sync: bool,
    direct_await_operand: Option<SourceRange>,
    scope_stack: Vec<SourceRange>,
    facts: DirectFacts,
}

impl<'a> FunctionContext<'a> {
    fn new(function: &'a FunctionDecl, result: Type, owner_sync: bool) -> Self {
        Self {
            all_depth: 0,
            caught_bindings: HashMap::new(),
            narrowed_errors: HashMap::new(),
            function,
            result,
            locals: HashMap::new(),
            declared_names: HashMap::new(),
            loop_depth: 0,
            in_finally: false,
            in_task_cleanup: false,
            owner_sync,
            direct_await_operand: None,
            scope_stack: Vec::new(),
            facts: DirectFacts::default(),
        }
    }
}
