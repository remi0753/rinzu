use std::collections::{HashMap, HashSet};
use std::fmt;

mod errors;

use crate::ast::{
    Arguments, BinaryOperator, Block, Declaration, Expression, ExpressionKind, FunctionDecl,
    Literal, Module, Statement, StatementKind, TypeRef, TypeRefKind, UnaryOperator,
};
use crate::{
    AbiScalar, Diagnostic, DiagnosticCode, IntegerConstant, SourceRange, evaluate_module_constants,
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Type {
    Unit,
    Never,
    Bool,
    Int,
    Float,
    Scalar(AbiScalar),
    String,
    Bytes,
    FixedBytes(u64),
    Named(String),
    List(Box<Type>),
    Map(Box<Type>, Box<Type>),
    Future(Box<Type>),
    Native { name: String, call_only: bool },
    StaticHandler(Box<StaticHandlerType>),
    FunctionReference(Box<FunctionReferenceType>),
    Error,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct StaticHandlerType {
    pub parameters: Vec<Type>,
    pub result: Box<Type>,
    pub is_async: bool,
    pub owner_sync: bool,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FunctionReferenceType {
    pub parameters: Vec<Type>,
    pub result: Box<Type>,
    pub supplant: bool,
}

impl Type {
    #[must_use]
    pub fn from_scalar(scalar: AbiScalar) -> Self {
        match scalar {
            AbiScalar::I64 => Self::Int,
            AbiScalar::F64 => Self::Float,
            AbiScalar::Bool => Self::Bool,
            _ => Self::Scalar(scalar),
        }
    }
    #[must_use]
    pub fn scalar(&self) -> Option<AbiScalar> {
        match self {
            Self::Int => Some(AbiScalar::I64),
            Self::Float => Some(AbiScalar::F64),
            Self::Bool => Some(AbiScalar::Bool),
            Self::Scalar(value) => Some(*value),
            _ => None,
        }
    }
    #[must_use]
    pub fn is_integer(&self) -> bool {
        self.scalar()
            .is_some_and(|scalar| scalar.integer_bounds().is_some())
    }
    #[must_use]
    pub fn is_numeric(&self) -> bool {
        self.scalar()
            .is_some_and(|scalar| scalar != AbiScalar::Bool)
    }

    #[must_use]
    pub fn is_managed_reference(&self) -> bool {
        matches!(
            self,
            Self::String
                | Self::Bytes
                | Self::FixedBytes(_)
                | Self::Error
                | Self::Named(_)
                | Self::List(_)
                | Self::Map(_, _)
                | Self::Future(_)
                | Self::Native {
                    call_only: false,
                    ..
                }
        )
    }
}

impl fmt::Display for Type {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unit => write!(formatter, "Unit"),
            Self::Never => write!(formatter, "Never"),
            Self::Bool => write!(formatter, "Bool"),
            Self::Int => write!(formatter, "Int"),
            Self::Float => write!(formatter, "Float"),
            Self::Scalar(value) => formatter.write_str(value.name()),
            Self::String => write!(formatter, "String"),
            Self::Bytes => write!(formatter, "ByteArray"),
            Self::FixedBytes(length) => write!(formatter, "Bytes({length})"),
            Self::Named(name) if name == errors::SOURCE_LOCATION => {
                formatter.write_str("SourceLocation")
            }
            Self::Named(name) => write!(formatter, "{name}"),
            Self::List(element) => write!(formatter, "List<{element}>"),
            Self::Map(key, value) => write!(formatter, "Map<{key}, {value}>"),
            Self::Future(value) => write!(formatter, "Future<{value}>"),
            Self::Native { name, .. } => write!(formatter, "{name}"),
            Self::StaticHandler(_) => formatter.write_str("generated static handler reference"),
            Self::FunctionReference(signature) => {
                if signature.supplant {
                    formatter.write_str("supplant ")?;
                }
                formatter.write_str("fn(")?;
                for (index, parameter) in signature.parameters.iter().enumerate() {
                    if index != 0 {
                        formatter.write_str(",")?;
                    }
                    write!(formatter, "{parameter}")?;
                }
                write!(formatter, ")->{}", signature.result)
            }
            Self::Error => formatter.write_str("Error"),
        }
    }
}

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
pub struct CheckedModule {
    pub(crate) error_upcasts: HashMap<SourceRange, Type>,
    pub(crate) error_patterns: HashMap<SourceRange, Type>,
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
    range: SourceRange,
}

#[derive(Clone)]
struct Local {
    ty: Type,
    mutable: bool,
    constant: Option<IntegerConstant>,
    targets: Vec<String>,
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
    let module = crate::records::expand_aliases(&module)?;
    let mut module = crate::value_constants::expand(&module)?;
    crate::error_effects::validate_annotations(&mut module)?;
    let module = crate::interfaces::prepare(&module)?;
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
    checker.generic_requirements = requirements;
    let checked = checker.run()?;
    let mut checked = if let Some((converted, file)) = crate::closures::convert(&checked)? {
        let mut checker = Checker::new(&converted, &options, &option_shadow_modules);
        checker.source_indirect_fields = checked.indirect_fields.clone();
        checker.closure_conversion = true;
        let mut result = checker.run()?;
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
        let functions = syntax
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
        Ok(CheckedModule {
            error_upcasts: self.error_upcasts,
            error_patterns: self.error_patterns,
            error_tests: self.error_tests,
            error_narrowings: self.error_narrowings,
            bindings: self.bindings,
            static_references: self.static_references,
            type_references: self.type_references,
            closure_source: None,
            indirect_fields: self.indirect_fields,
            native_captures: self.native_captures,
            source_types,
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
            if let Some(contract) = &function.error_contract {
                let mut seen = HashSet::new();
                let mut any = false;
                for source in &contract.types {
                    if let Some(ty) = self.resolve_type_with_constants(source, &signature_constants)
                    {
                        if !self.is_error_type(&ty) {
                            self.diagnostics.push(Diagnostic::error(
                                DiagnosticCode::InvalidErrorContract,
                                source.range,
                                format!("`{ty}` is not an Error type"),
                                "list concrete Error types, Error, or use nothrow",
                            ));
                        }
                        any |= ty == Type::Error;
                        if !seen.insert(ty.to_string()) {
                            self.diagnostics.push(Diagnostic::error(
                                DiagnosticCode::InvalidErrorContract,
                                source.range,
                                "duplicate Error type in throws contract",
                                "remove the duplicate type",
                            ));
                        }
                    }
                }
                if any && contract.types.len() > 1 {
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::InvalidErrorContract,
                        contract.range,
                        "throws Error already includes every ordinary Error",
                        "write throws Error without additional types",
                    ));
                }
            }
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
                    range: function.range,
                },
            );
        }
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

    fn check_block(&mut self, block: &Block, context: &mut FunctionContext<'_>) {
        context.scope_stack.push(block.range);
        let active_before: HashSet<_> = context.locals.keys().cloned().collect();
        for statement in &block.statements {
            self.check_statement(statement, context);
        }
        context
            .locals
            .retain(|name, _| active_before.contains(name));
        context.scope_stack.pop();
    }

    fn check_statement(&mut self, statement: &Statement, context: &mut FunctionContext<'_>) {
        match &statement.kind {
            StatementKind::Throw(value) => {
                self.expect_expression(value, &Type::Error, context);
                context.facts.may_fail = true;
            }
            StatementKind::TryErrors {
                all,
                body,
                handlers,
                cleanup,
            } => {
                self.check_error_handlers(body, handlers, cleanup.as_ref(), context, *all);
            }
            StatementKind::Block(block) => self.check_block(block, context),
            StatementKind::Local {
                mutable,
                name,
                annotation,
                initializer,
            } => {
                let values = self.lexical_constants(context);
                let expected = annotation
                    .as_ref()
                    .and_then(|value| self.resolve_type_with_constants(value, &values));
                let initializer_type =
                    self.check_expression(initializer, expected.as_ref(), context);
                let ty = match (expected, initializer_type) {
                    (Some(expected), Some(actual)) => {
                        self.require_same(&expected, &actual, initializer.range);
                        expected
                    }
                    (None, Some(actual)) => actual,
                    (Some(expected), None) => expected,
                    (None, None) => return,
                };
                if self.contains_call_only_type(&ty, &mut HashSet::new()) {
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::CallOnlyEscape,
                        initializer.range,
                        "CallOnly value cannot be stored in a local binding",
                        "read the view or pass it directly to an approved synchronous native call",
                    ));
                }
                if context.owner_sync && matches!(ty, Type::Future(_)) {
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::InvalidAwait,
                        initializer.range,
                        "OwnerSync handler cannot create or store a Future",
                        "move asynchronous work outside the synchronous callback",
                    ));
                }
                let constant = if !*mutable && ty.is_integer() {
                    crate::constants::evaluate_integer_expression(initializer, ty.scalar(), &values)
                        .ok()
                } else {
                    None
                };
                if let Some(previous) = context.declared_names.get(name).copied() {
                    self.diagnostics.push(
                        Diagnostic::error(
                            DiagnosticCode::DuplicateName,
                            statement.range,
                            format!("local `{name}` shadows an existing name"),
                            "choose a name not used elsewhere in this function",
                        )
                        .with_secondary(previous, "existing declaration is here"),
                    );
                } else {
                    context.declared_names.insert(name.clone(), statement.range);
                    context.locals.insert(
                        name.clone(),
                        Local {
                            ty: ty.clone(),
                            mutable: *mutable,
                            constant,
                            targets: if *mutable {
                                if let Type::FunctionReference(expected) = &ty {
                                    self.compatible_reference_targets(expected)
                                } else {
                                    Vec::new()
                                }
                            } else {
                                self.reference_targets
                                    .get(&initializer.range)
                                    .cloned()
                                    .unwrap_or_default()
                            },
                        },
                    );
                }
            }
            StatementKind::FieldAssign { base, field, value } => {
                let Some(Type::Named(identity)) = self.check_expression(base, None, context) else {
                    self.type_error(base.range, "field assignment requires a class receiver");
                    return;
                };
                let Some(Declaration::Record(record)) = self.declarations.get(&identity).cloned()
                else {
                    self.type_error(base.range, "field assignment requires a class receiver");
                    return;
                };
                if !record.mutable {
                    self.type_error(
                        base.range,
                        "immutable record fields cannot be assigned; create a new record value",
                    );
                    return;
                }
                let Some(declaration) = record.fields.iter().find(|f| &f.name == field) else {
                    self.type_error(statement.range, format!("unknown class field `{field}`"));
                    return;
                };
                if declaration.is_private && !self.owner_access(&identity, context) {
                    self.private_field_error(field, statement.range);
                    return;
                }
                if declaration.is_const {
                    self.type_error(
                        statement.range,
                        format!("const field `{field}` cannot be assigned after construction"),
                    );
                    return;
                }
                let Some(expected) = self.resolve_type(&declaration.ty) else {
                    return;
                };
                if let Some(actual) = self.check_expression(value, Some(&expected), context) {
                    self.require_same(&expected, &actual, value.range);
                    if self.contains_call_only_type(&actual, &mut HashSet::new()) {
                        self.diagnostics.push(Diagnostic::error(
                            DiagnosticCode::CallOnlyEscape,
                            value.range,
                            "borrowed value cannot escape into state",
                            "store a copied owned value",
                        ));
                    }
                }
            }
            StatementKind::Assign { name, value } => {
                let Some(local) = context.locals.get(name).cloned() else {
                    self.unknown_name(name, statement.range);
                    return;
                };
                if !local.mutable {
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::InvalidAssignment,
                        statement.range,
                        format!("`{name}` is immutable"),
                        "declare it with `var` if reassignment is required",
                    ));
                }
                if let Some(actual) = self.check_expression(value, Some(&local.ty), context) {
                    if self.contains_call_only_type(&actual, &mut HashSet::new()) {
                        self.diagnostics.push(Diagnostic::error(
                            DiagnosticCode::CallOnlyEscape,
                            value.range,
                            "CallOnly value cannot be assigned to local state",
                            "use the value only during the current synchronous callback",
                        ));
                    }
                    self.require_same(&local.ty, &actual, value.range);
                    if let Some(targets) = self.reference_targets.get(&value.range) {
                        let local = context.locals.get_mut(name).expect("checked local");
                        local.targets.extend(targets.iter().cloned());
                        local.targets.sort();
                        local.targets.dedup();
                    }
                }
            }
            StatementKind::Expression(expression) => {
                if let Some(ty) = self.check_expression(expression, Some(&Type::Unit), context) {
                    // Future statement values are checked by the task
                    // responsibility analysis, including their success value.
                    if !matches!(ty, Type::Future(_)) && ty != Type::Unit && ty != Type::Never {
                        self.diagnostics.push(Diagnostic::error(
                            DiagnosticCode::TypeMismatch,
                            expression.range,
                            format!("expression statement has type `{ty}`, not `Unit`"),
                            "store, return, or otherwise use the value",
                        ));
                    }
                }
            }
            StatementKind::Return(value) => {
                if context.in_finally {
                    self.invalid_finally(statement.range, "`return` is not allowed in `finally`");
                }
                let result_type = context.result.clone();
                let actual = value
                    .as_ref()
                    .and_then(|expression| {
                        self.check_expression(expression, Some(&result_type), context)
                    })
                    .unwrap_or(Type::Unit);
                if self.contains_call_only_type(&actual, &mut HashSet::new()) {
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::CallOnlyEscape,
                        statement.range,
                        "CallOnly value cannot be returned",
                        "consume the view synchronously inside the OwnerSync handler",
                    ));
                }
                self.require_same(&result_type, &actual, statement.range);
            }
            StatementKind::Break | StatementKind::Continue => {
                if context.in_finally {
                    self.invalid_finally(
                        statement.range,
                        "loop control is not allowed in `finally`",
                    );
                }
                if context.loop_depth == 0 {
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::InvalidControlFlow,
                        statement.range,
                        "loop control is only valid inside `while` or `for`",
                        "move this statement into a loop body or remove it",
                    ));
                }
            }
            StatementKind::If {
                condition,
                then_block,
                else_branch,
            } => {
                self.expect_expression(condition, &Type::Bool, context);
                let previous = context.narrowed_errors.clone();
                context
                    .narrowed_errors
                    .extend(self.error_condition_refinements(condition, true, context));
                self.check_block(then_block, context);
                context.narrowed_errors = previous.clone();
                if let Some(branch) = else_branch {
                    context
                        .narrowed_errors
                        .extend(self.error_condition_refinements(condition, false, context));
                    self.check_statement(branch, context);
                }
                context.narrowed_errors = previous;
            }
            StatementKind::While { condition, body } => {
                self.expect_expression(condition, &Type::Bool, context);
                context.loop_depth += 1;
                let previous = context.narrowed_errors.clone();
                context
                    .narrowed_errors
                    .extend(self.error_condition_refinements(condition, true, context));
                self.check_block(body, context);
                context.narrowed_errors = previous;
                context.loop_depth -= 1;
            }
            StatementKind::For {
                binding,
                binding_range,
                iterable,
                body,
            } => {
                let Some(ty) = self.check_expression(iterable, None, context) else {
                    return;
                };
                let Type::List(element) = ty else {
                    self.type_error(iterable.range, "`for` requires a List value");
                    return;
                };
                let declared_names = context.declared_names.clone();
                if binding != "_" {
                    if let Some(previous) = context.declared_names.get(binding).copied() {
                        self.diagnostics.push(
                            Diagnostic::error(
                                DiagnosticCode::DuplicateName,
                                *binding_range,
                                format!("local `{binding}` shadows an existing name"),
                                "choose an unused loop binding name",
                            )
                            .with_secondary(previous, "existing declaration is here"),
                        );
                        return;
                    }
                    context
                        .declared_names
                        .insert(binding.clone(), *binding_range);
                    let targets = if let Type::FunctionReference(signature) = element.as_ref() {
                        self.compatible_reference_targets(signature)
                    } else {
                        Vec::new()
                    };
                    context.locals.insert(
                        binding.clone(),
                        Local {
                            ty: *element,
                            mutable: false,
                            constant: None,
                            targets,
                        },
                    );
                }
                context.loop_depth += 1;
                self.check_block(body, context);
                context.loop_depth -= 1;
                context.locals.remove(binding);
                context.declared_names = declared_names;
            }
            StatementKind::Switch { value, cases } => {
                let value_type = self.check_expression(value, None, context);
                let Some(Type::Named(enum_name)) = value_type else {
                    if value_type.is_some() {
                        self.diagnostics.push(Diagnostic::error(
                            DiagnosticCode::TypeMismatch,
                            value.range,
                            "`switch` requires a closed enum value",
                            "switch on a declared enum value",
                        ));
                    }
                    return;
                };
                self.check_switch(&enum_name, cases, context);
            }
            StatementKind::TryCatch {
                body,
                binding,
                binding_range,
                handler,
                cleanup,
            } => {
                let handler = crate::ast::CatchClause::whole(binding, *binding_range, handler);
                self.check_error_handlers(body, &[handler], cleanup.as_ref(), context, false);
            }
            StatementKind::TaskScope { body, cleanup } => {
                if context.in_finally && !context.in_task_cleanup {
                    self.invalid_finally(
                        statement.range,
                        "`taskScope` is not allowed in `finally`",
                    );
                }
                if !context.function.is_async || context.owner_sync {
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::InvalidAwait,
                        statement.range,
                        "`taskScope` requires an async Main function",
                        "use a taskScope inside an async function",
                    ));
                }
                context.facts.requires_owner = true;
                context.facts.may_await = true;
                self.check_block(body, context);
                let previous = (context.in_finally, context.in_task_cleanup);
                context.in_finally = true;
                context.in_task_cleanup = true;
                self.check_block(cleanup, context);
                (context.in_finally, context.in_task_cleanup) = previous;
            }
            StatementKind::TryAll {
                body,
                handler,
                cleanup,
            } => {
                let handlers = handler
                    .as_ref()
                    .map(|handler| {
                        crate::ast::CatchClause::whole(
                            &handler.binding,
                            handler.binding_range,
                            &handler.body,
                        )
                    })
                    .into_iter()
                    .collect::<Vec<_>>();
                self.check_error_handlers(body, &handlers, cleanup.as_ref(), context, true);
            }
            StatementKind::TryFinally { body, cleanup } => {
                self.check_block(body, context);
                let previous = (context.in_finally, context.in_task_cleanup);
                context.in_finally = true;
                context.in_task_cleanup = context.function.is_async;
                self.check_block(cleanup, context);
                (context.in_finally, context.in_task_cleanup) = previous;
            }
        }
    }

    fn check_switch(
        &mut self,
        enum_name: &str,
        cases: &[crate::ast::SwitchCase],
        context: &mut FunctionContext<'_>,
    ) {
        let Some(Declaration::Enum(enum_decl)) = self.declarations.get(enum_name).cloned() else {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::TypeMismatch,
                cases.first().map_or(self.module.range, |case| case.range),
                format!("`{enum_name}` is not a closed enum"),
                "switch on a declared enum value",
            ));
            return;
        };
        let mut seen = HashSet::new();
        for case in cases {
            let Some(variant) = case.variant(&enum_decl) else {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::UnknownName,
                    case.range,
                    format!("unknown `{enum_name}` variant `{}`", case.path.join(".")),
                    format!("use a variant declared by `{enum_name}`"),
                ));
                continue;
            };
            if !seen.insert(variant.name.clone()) {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::DuplicateName,
                    case.range,
                    format!("variant `{}` appears more than once", variant.name),
                    "keep exactly one case for each variant",
                ));
            }
            if variant.fields.len() != case.bindings.len() {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::TypeMismatch,
                    case.range,
                    format!(
                        "variant `{}` has {} payload fields but the case binds {}",
                        variant.name,
                        variant.fields.len(),
                        case.bindings.len()
                    ),
                    "bind one name for each payload field in declaration order",
                ));
            }
            let case_names = context.declared_names.clone();
            let mut inserted = Vec::new();
            for (name, field) in case.bindings.iter().zip(&variant.fields) {
                if let Some(previous) = context.declared_names.get(name).copied() {
                    self.diagnostics.push(
                        Diagnostic::error(
                            DiagnosticCode::DuplicateName,
                            case.range,
                            format!("case binding `{name}` shadows an existing name"),
                            "choose a unique binding name",
                        )
                        .with_secondary(previous, "existing declaration is here"),
                    );
                    continue;
                }
                if let Some(ty) = self.resolve_type(&field.ty) {
                    context.declared_names.insert(name.clone(), case.range);
                    context.locals.insert(
                        name.clone(),
                        Local {
                            ty,
                            mutable: false,
                            constant: None,
                            targets: Vec::new(),
                        },
                    );
                    inserted.push(name.clone());
                }
            }
            self.check_block(&case.body, context);
            for name in inserted {
                context.locals.remove(&name);
            }
            context.declared_names = case_names;
        }
        let missing: Vec<_> = enum_decl
            .variants
            .iter()
            .filter(|variant| !seen.contains(&variant.name))
            .map(|variant| variant.name.as_str())
            .collect();
        if !missing.is_empty() {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::NonExhaustiveSwitch,
                cases.first().map_or(enum_decl.range, |case| case.range),
                format!("switch is missing variants: {}", missing.join(", ")),
                "add one case for every missing variant; language 0.1 has no default case",
            ));
        }
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

    fn check_expression(
        &mut self,
        expression: &Expression,
        expected: Option<&Type>,
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
                    && (path.len() == 1
                        || self
                            .module
                            .imports
                            .iter()
                            .any(|import| import.path == ["rinzu", "core"]))
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
        if path.len() > 1
            && crate::core::function_name(path).is_some()
            && !self
                .module
                .imports
                .iter()
                .any(|import| import.path == ["rinzu", "core"])
        {
            self.unknown_name(&name, range);
            return None;
        }
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
            })
    }

    fn compatible_reference_targets(&self, expected: &FunctionReferenceType) -> Vec<String> {
        let mut targets: Vec<_> = self
            .functions
            .keys()
            .chain(self.source_signatures.keys())
            .filter(|name| self.reference_signature(name).as_ref() == Some(expected))
            .cloned()
            .collect();
        targets.extend(
            self.bindings
                .values()
                .filter(|binding| &binding.signature == expected)
                .map(|binding| binding.target.clone()),
        );
        targets.sort();
        if crate::core::reference_signature("rinzu.core::utf8ByteLength").as_ref() == Some(expected)
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

    fn receiver_method_name(
        &self,
        target: &[String],
        context: &FunctionContext<'_>,
    ) -> Option<String> {
        if target.len() < 2 {
            return None;
        }
        let mut ty = context.locals.get(&target[0])?.ty.clone();
        for field in &target[1..target.len() - 1] {
            let Type::Named(name) = &ty else {
                return None;
            };
            let field = self
                .record_declaration(name)?
                .fields
                .iter()
                .find(|f| &f.name == field)?;
            ty = self.resolve_type_readonly(&field.ty)?;
        }
        let owner = match ty {
            Type::Named(name) | Type::Native { name, .. } => name,
            _ => return None,
        };
        Some(format!("{owner}.{}", target.last()?))
    }

    fn receiver_method(
        &mut self,
        target: &[String],
        range: SourceRange,
        context: &FunctionContext<'_>,
    ) -> Result<Option<(String, Expression)>, ()> {
        let receiver_path = &target[..target.len() - 1];
        let mut receiver_range = range;
        receiver_range.end_byte = range.start_byte;
        let ty = self
            .check_name(receiver_path, receiver_range, context)
            .ok_or(())?;
        let owner = match ty {
            Type::Named(name) | Type::Native { name, .. } => name,
            _ => return Ok(None),
        };
        let method = format!("{owner}.{}", target.last().expect("receiver method name"));
        let Some(Declaration::Function(declaration)) = self.declarations.get(&method) else {
            return Ok(None);
        };
        if declaration
            .parameters
            .first()
            .is_none_or(|p| p.name != "self")
        {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::TypeMismatch,
                range,
                format!("static method `{method}` cannot be called through a value"),
                format!("write {method}(arguments)"),
            ));
            return Err(());
        }
        if !self.method_visible(&method, range, context) {
            return Err(());
        }
        Ok(Some((
            method,
            Expression {
                kind: ExpressionKind::Name(receiver_path.to_vec()),
                range: receiver_range,
            },
        )))
    }

    fn check_call(
        &mut self,
        target: &[String],
        arguments: &Arguments,
        range: SourceRange,
        context: &mut FunctionContext<'_>,
    ) -> Option<Type> {
        let qualified = target.join(".");
        if target.len() > 1 && context.locals.contains_key(&target[0]) {
            match self.receiver_method(target, range, context) {
                Ok(Some((method, receiver))) => {
                    let Arguments::Positional(values) = arguments else {
                        self.type_error(range, "method calls require positional arguments");
                        return None;
                    };
                    let mut values = values.clone();
                    values.insert(0, receiver);
                    let arguments = Arguments::Positional(values);
                    let target = vec![method];
                    let result =
                        self.check_function_call(&target, &arguments, range, context, false);
                    self.method_calls
                        .insert(range, ExpressionKind::Call { target, arguments });
                    return result;
                }
                Ok(None) => {}
                Err(()) => return None,
            }
        }
        if let Some(path) = self.source_indirect_fields.get(&range) {
            context
                .facts
                .indirect_sites
                .push((path.clone(), range, vec![qualified.clone()]));
            self.indirect_fields.insert(range, path.clone());
        }
        if context.locals.contains_key(&target[0]) {
            let callee_type = self.check_name(target, range, context)?;
            let possible = self
                .reference_targets
                .get(&range)
                .cloned()
                .unwrap_or_default();
            let Type::FunctionReference(signature) = callee_type else {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::TypeMismatch,
                    range,
                    "local call target is not a function reference",
                    "call a function or typed function reference",
                ));
                return None;
            };
            if signature.supplant && context.direct_await_operand != Some(range) {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::InvalidAwait,
                    range,
                    "supplant call must be the direct operand of await",
                    "write await reference(arguments)",
                ));
                return None;
            }
            let Arguments::Positional(values) = arguments else {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::TypeMismatch,
                    range,
                    "function references require positional arguments",
                    "pass arguments in signature order",
                ));
                return None;
            };
            if values.len() != signature.parameters.len() {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::TypeMismatch,
                    range,
                    "function reference argument count mismatch",
                    "pass one argument per parameter",
                ));
            }
            for (value, parameter) in values.iter().zip(&signature.parameters) {
                if signature.supplant {
                    self.expect_native_callback_argument(value, parameter, context);
                    if let Some(targets) = self.reference_targets.get(&value.range) {
                        context.facts.calls.extend(
                            targets
                                .iter()
                                .filter(|t| self.functions.contains_key(*t))
                                .cloned(),
                        );
                    }
                } else {
                    self.expect_expression(value, parameter, context);
                }
            }
            let path = if target.len() > 1 {
                format!(
                    "{}.{}",
                    context.locals[&target[0]].ty,
                    target[1..].join(".")
                )
            } else {
                target[0].clone()
            };
            self.indirect_fields.insert(range, path.clone());
            context
                .facts
                .indirect_sites
                .push((path, range, possible.clone()));
            for name in &possible {
                if let Some(native) = self.source_signatures.get(name) {
                    context.facts.native_requirements.insert(name.clone());
                    context.facts.has_cleanup_unsafe |= native.is_async;
                    context.facts.has_native |= native.is_async || native.is_supplant;
                } else {
                    context.facts.calls.insert(name.clone());
                    if context.in_finally && !context.in_task_cleanup {
                        context.facts.cleanup_calls.insert(name.clone(), range);
                    }
                }
            }
            if context.owner_sync && matches!(*signature.result, Type::Future(_)) {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::InvalidAwait,
                    range,
                    "OwnerSync handler cannot call async references",
                    "use a synchronous reference",
                ));
            }
            if signature.supplant {
                context.facts.requires_owner = true;
                context.facts.has_cleanup_unsafe = true;
                self.supplant_calls.insert(
                    range,
                    SupplantCall {
                        targets: possible,
                        reference: Some(target.join(".")),
                        signature: (*signature).clone(),
                    },
                );
            } else {
                self.indirect_calls.insert(range, possible);
            }
            context.facts.may_fail = true;
            return Some(*signature.result);
        }
        if let [root, module, ..] = target
            && root == "rinzu"
            && matches!(module.as_str(), "list" | "map")
        {
            // A valid import links the dotted path to a canonical symbol.
            self.unknown_name(&qualified, range);
            return None;
        }
        if let Some(newline) = crate::stdio::call(target) {
            if target.len() > 1
                && !self
                    .module
                    .imports
                    .iter()
                    .any(|import| import.path == ["rinzu", "stdio"])
            {
                self.unknown_name(&qualified, range);
                return None;
            }
            let Arguments::Positional(values) = arguments else {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::TypeMismatch,
                    range,
                    "stdout requires one positional argument",
                    "pass one value to display",
                ));
                return None;
            };
            if values.len() != 1 {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::TypeMismatch,
                    range,
                    "stdout requires one argument",
                    "pass one value to display",
                ));
                return None;
            }
            self.check_expression(&values[0], None, context)?;
            context.facts.may_fail = true;
            self.stdout_calls.insert(range, newline);
            return Some(Type::Unit);
        }
        let core = crate::core::function_name(target)
            .filter(|_| !self.declarations.contains_key(&qualified));
        if target.len() > 1
            && core.is_some()
            && !self
                .module
                .imports
                .iter()
                .any(|import| import.path == ["rinzu", "core"])
        {
            self.unknown_name(&qualified, range);
            return None;
        }
        if let Some(function) = crate::RuntimeFunction::from_path(target)
            .filter(|_| !self.declarations.contains_key(&qualified))
        {
            if function == crate::RuntimeFunction::Sleep
                && target.len() > 1
                && !self
                    .module
                    .imports
                    .iter()
                    .any(|import| import.path == ["rinzu", "timer"])
            {
                self.unknown_name(&qualified, range);
                return None;
            }
            return self.check_runtime_call(function, arguments, range, context);
        }
        if let Some(kind) = core.and_then(crate::TaskControlKind::from_name) {
            let Arguments::Positional(values) = arguments else {
                self.type_error(range, "task control requires positional arguments");
                return None;
            };
            let count = usize::from(kind != crate::TaskControlKind::Shutdown);
            if values.len() != count {
                self.type_error(range, "task control argument count is invalid");
                return None;
            }
            if let Some(value) = values.first() {
                let actual = self.check_expression(value, None, context)?;
                if kind == crate::TaskControlKind::ExitStatus {
                    if actual != Type::Int {
                        self.type_error(value.range, "process exit status requires Int");
                        return None;
                    }
                } else if !matches!(actual, Type::Future(_)) {
                    self.type_error(value.range, "task control requires a typed Future");
                    return None;
                }
            }
            if context.owner_sync {
                self.type_error(
                    range,
                    "task control requires async Main context, not OwnerSync",
                );
                return None;
            }
            context.facts.requires_owner = true;
            self.task_controls.insert(range, kind);
            return Some(Type::Bool);
        }
        if let Some(op) = crate::core::data_operation(target)
            .filter(|_| !self.declarations.contains_key(&qualified))
        {
            let Arguments::Positional(values) = arguments else {
                self.type_error(range, "data operation requires positional arguments");
                return None;
            };
            if values.len() != op.arity() {
                self.type_error(
                    range,
                    format!("{} requires {} arguments", op.name(), op.arity()),
                );
                return None;
            }
            if op == crate::DataOperation::ProcessArguments {
                context.facts.requires_owner = true;
                self.data_operations.insert(range, op);
                let name = if self
                    .declarations
                    .contains_key("rinzu.process::ArgumentSnapshot")
                {
                    "rinzu.process::ArgumentSnapshot"
                } else {
                    "ArgumentSnapshot"
                };
                return Some(Type::Named(name.into()));
            }
            if op.is_json() {
                let types = values
                    .iter()
                    .map(|value| self.check_expression(value, None, context))
                    .collect::<Option<Vec<_>>>()?;
                let result = context.result.clone();
                if !crate::json::signature(op, &types, &result) {
                    self.type_error(range, "invalid internal JSON operation signature");
                    return None;
                }
                if matches!(
                    op,
                    crate::DataOperation::JsonDecode | crate::DataOperation::JsonDecodeWithLimits
                ) && let Some(path) = self.unsupported_json_type(&result)
                {
                    self.type_error(range, format!("unsupported JSON target type at {path}"));
                    return None;
                }
                self.data_operations.insert(range, op);
                context.facts.may_fail = true;
                return Some(result);
            }
            let input = self.check_expression(&values[0], None, context)?;
            let Some((parameters, result)) = crate::core::data_signature(op, &input) else {
                self.type_error(
                    values[0].range,
                    format!("{} does not accept `{input}`", op.name()),
                );
                return None;
            };
            for (value, expected) in values.iter().skip(1).zip(parameters.iter().skip(1)) {
                self.expect_expression(value, expected, context);
            }
            use crate::DataOperation::*;
            if op == MapEntries
                && let Type::Map(key, value) = &input
            {
                self.intern_record(crate::collections::entry_fields(key, value), range)?;
            }
            let payload = match (op, &input) {
                (ListGet, Type::List(t)) => Some(t.as_ref()),
                (ListSet | ListRemove, _) => Some(&input),
                (StringGet | StringSlice, _) => Some(&Type::String),
                (StringFind | BytesFindAny | BytesFindNotAny, _) => Some(&Type::Int),
                (BytesSlice, _) => Some(&Type::Bytes),
                (BytesGet, _) => Some(&Type::Scalar(AbiScalar::U8)),
                (ErrorCause, _) => Some(&Type::Error),
                (ErrorSourceFile, _) => Some(&Type::Scalar(AbiScalar::U64)),
                (ErrorSourceStart | ErrorSourceEnd | ErrorNativeStatus, _) => Some(&Type::Int),
                (MapGet, Type::Map(_, v)) => Some(v.as_ref()),
                _ => None,
            };
            if let Some(payload) = payload {
                self.ensure_option(payload, range, &context.function.name)?;
            }
            self.data_operations.insert(range, op);
            context.facts.may_fail = true;
            return Some(result);
        }
        if matches!(core, Some("utf8ByteLength" | "byteLength")) {
            let Arguments::Positional(values) = arguments else {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::TypeMismatch,
                    range,
                    "core length functions require one positional argument",
                    "pass one String or Bytes value",
                ));
                return None;
            };
            if values.len() != 1 {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::TypeMismatch,
                    range,
                    "core length functions require one argument",
                    "pass one String or Bytes value",
                ));
                return None;
            }
            let actual = self.check_expression(&values[0], None, context)?;
            let kind = if core == Some("utf8ByteLength") && actual == Type::String {
                crate::CoreLengthKind::Utf8
            } else if core == Some("byteLength")
                && matches!(actual, Type::FixedBytes(_) | Type::Bytes)
            {
                crate::CoreLengthKind::Bytes
            } else {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::TypeMismatch,
                    range,
                    "core length argument has an incompatible type",
                    "use utf8ByteLength for String and byteLength for Bytes(N)/ByteArray",
                ));
                return None;
            };
            self.core_lengths.insert(range, kind);
            return Some(Type::from_scalar(AbiScalar::U64));
        }
        if target.len() == 1
            && let Some(scalar) =
                AbiScalar::from_name(&target[0]).filter(|scalar| *scalar != AbiScalar::Bool)
        {
            let Arguments::Positional(values) = arguments else {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::TypeMismatch,
                    range,
                    "numeric conversion requires one positional argument",
                    "pass one numeric value",
                ));
                return None;
            };
            if values.len() != 1 {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::TypeMismatch,
                    range,
                    "numeric conversion requires one argument",
                    "pass one numeric value",
                ));
                return None;
            }
            let source = self.check_expression(&values[0], None, context)?;
            if !source.is_numeric() {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::TypeMismatch,
                    values[0].range,
                    "numeric conversion requires a numeric operand",
                    "use an integer or floating value",
                ));
                return None;
            }
            let result = Type::from_scalar(scalar);
            context.facts.may_fail |= source != result;
            return Some(result);
        }

        if let Some(signature) = self.source_signatures.get(&qualified).cloned() {
            if signature.is_supplant && context.direct_await_operand != Some(range) {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::InvalidAwait,
                    range,
                    "supplant call must be the direct operand of await",
                    "write await nativeFunction(arguments)",
                ));
                return None;
            }
            let native = NativeFunction {
                parameters: signature.parameters.iter().map(source_abi_type).collect(),
                result: if signature.is_async {
                    Type::Future(Box::new(source_abi_type(&signature.result)))
                } else {
                    source_abi_type(&signature.result)
                },
                call_class: NativeCallClass::NativeSafe,
                metadata_complete: true,
                cleanup_safe: !signature.is_async && !signature.is_supplant,
            };
            let result = self.check_native_call(&qualified, &native, arguments, range, context);
            if let Arguments::Positional(values) = arguments {
                for (argument, parameter) in values.iter().zip(&signature.parameters) {
                    if matches!(parameter, crate::SourceAbiType::Callback { .. }) {
                        let targets = self
                            .reference_targets
                            .get(&argument.range)
                            .cloned()
                            .unwrap_or_default();
                        for target in targets {
                            if let Some(native) = self.source_signatures.get(&target) {
                                context.facts.native_requirements.insert(target);
                                context.facts.has_cleanup_unsafe |=
                                    native.is_async || native.is_supplant;
                            } else {
                                context.facts.calls.insert(target.clone());
                                if context.in_finally && !context.in_task_cleanup {
                                    context.facts.cleanup_calls.insert(target, argument.range);
                                }
                            }
                        }
                    }
                }
            }
            self.native_calls.remove(&range);
            if signature.is_supplant {
                context.facts.requires_owner = true;
                self.supplant_calls.insert(
                    range,
                    SupplantCall {
                        targets: vec![qualified.clone()],
                        reference: None,
                        signature: self
                            .reference_signature(&qualified)
                            .expect("source signature"),
                    },
                );
            } else {
                self.source_native_calls.insert(range, signature);
            }
            // Ordinary source native calls execute on the caller thread. They do not
            // inherit the legacy SDK blanket Worker prohibition.
            context.facts.has_native = context.facts.native_requirements.iter().any(|name| {
                self.source_signatures
                    .get(name)
                    .is_none_or(|signature| signature.is_async || signature.is_supplant)
            });
            return result;
        }
        if let Some(native) = self.options.native_functions.get(&qualified).cloned() {
            return self.check_native_call(&qualified, &native, arguments, range, context);
        }
        if self.functions.contains_key(&qualified) {
            let needs_normalization = target.len() > 1;
            let target = vec![qualified];
            let result = self.check_function_call(&target, arguments, range, context, false);
            if needs_normalization && matches!(arguments, Arguments::Positional(_)) {
                self.method_calls.insert(
                    range,
                    ExpressionKind::Call {
                        target,
                        arguments: arguments.clone(),
                    },
                );
            }
            return result;
        }
        if target.len() == 1
            && let Some(Declaration::Error(error)) = self.declarations.get(&target[0]).cloned()
        {
            return self.check_error_constructor(&error, arguments, range, context);
        }
        if target.len() == 1
            && let Some(record) = self.record_declaration(&target[0]).cloned()
        {
            if record.fields.iter().any(|f| f.is_private)
                && !self.owner_access(&record.name, context)
            {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::UnknownName,
                    range,
                    format!(
                        "class `{}` has a private constructor because it contains private fields",
                        record.name
                    ),
                    "construct this class through a public factory method",
                ));
                return None;
            }
            context.facts.may_fail = true;
            return self
                .check_named_arguments(&record.fields, arguments, range, context)
                .then(|| Type::Named(record.name.clone()));
        }
        if target.len() == 2
            && let Some(Declaration::Enum(enum_decl)) = self.declarations.get(&target[0]).cloned()
            && let Some(variant) = enum_decl
                .variants
                .iter()
                .find(|value| value.name == target[1])
        {
            context.facts.may_fail = true;
            return self
                .check_named_arguments(&variant.fields, arguments, range, context)
                .then(|| Type::Named(enum_decl.name.clone()));
        }
        if target.first().is_some_and(|name| name == "rinzu") {
            context.facts.has_native = true;
            context.facts.native_requirements.insert(target.join("."));
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::UnsupportedSafeFfi,
                range,
                format!(
                    "safe binding `{}` is unavailable for this target",
                    target.join(".")
                ),
                "install or generate a binding with complete safety metadata",
            ));
        } else {
            self.unknown_name(&target.join("."), range);
        }
        None
    }

    fn check_runtime_call(
        &mut self,
        function: crate::RuntimeFunction,
        arguments: &Arguments,
        range: SourceRange,
        context: &mut FunctionContext<'_>,
    ) -> Option<Type> {
        use crate::{RuntimeFunction as R, SourceAbiType as A};
        let Arguments::Positional(values) = arguments else {
            self.type_error(
                range,
                "runtime library functions require positional arguments",
            );
            return None;
        };
        let [value] = values.as_slice() else {
            self.type_error(
                range,
                "runtime library function requires exactly one argument",
            );
            return None;
        };
        let expected = match function {
            R::Sleep => Some(Type::Int),
            R::ReadResourceSize => Some(Type::String),
            R::NativeCompleted | R::NativeTransferred => None,
        };
        let actual = self.check_expression(value, expected.as_ref(), context)?;
        let parameter = if let Some(expected) = expected {
            self.require_same(&expected, &actual, value.range);
            if function == R::Sleep {
                A::Scalar(AbiScalar::I64)
            } else {
                A::String
            }
        } else if let Type::Native {
            name,
            call_only: false,
        } = actual
        {
            A::Opaque(name)
        } else {
            self.type_error(
                value.range,
                "native completion requires an opaque protocol handle",
            );
            return None;
        };
        if context.owner_sync {
            self.type_error(
                range,
                "async library functions cannot be called from OwnerSync",
            );
        }
        if context.in_finally && !context.in_task_cleanup {
            self.invalid_finally(range, "this async library call is not CleanupSafe");
        }
        let result = match function {
            R::Sleep | R::NativeCompleted => A::Unit,
            R::ReadResourceSize | R::NativeTransferred => A::Scalar(AbiScalar::I64),
        };
        // Completion functions are generic over nominal native handle types.
        let name = if matches!(parameter, A::Opaque(_)) {
            format!("{}<{}>", function.name(), parameter.canonical_name())
        } else {
            function.name().to_owned()
        };
        let signature = crate::SourceNativeSignature {
            name: name.clone(),
            binding: crate::ast::NativeBinding::Runtime(function),
            is_supplant: false,
            is_async: true,
            parameters: vec![parameter],
            result,
        };
        let ty = Type::Future(Box::new(source_abi_type(&signature.result)));
        self.source_signatures
            .insert(name.clone(), signature.clone());
        self.source_native_calls.insert(range, signature);
        context.facts.has_native = true;
        context.facts.has_cleanup_unsafe = true;
        context.facts.may_fail = true;
        context.facts.native_requirements.insert(name);
        Some(ty)
    }

    fn check_native_call(
        &mut self,
        name: &str,
        native: &NativeFunction,
        arguments: &Arguments,
        range: SourceRange,
        context: &mut FunctionContext<'_>,
    ) -> Option<Type> {
        if !native.metadata_complete {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::UnsupportedSafeFfi,
                range,
                format!("safe binding `{name}` has incomplete metadata"),
                "complete call class, thread, blocking, callback, lifetime, and exception metadata",
            ));
            return None;
        }
        if native.call_class == NativeCallClass::OwnerThreadSync && !context.owner_sync {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::UnsupportedSafeFfi,
                range,
                format!("native call `{name}` requires an OwnerSync context"),
                "invoke it from the generated owner-thread callback context",
            ));
        }
        let Arguments::Positional(arguments) = arguments else {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::TypeMismatch,
                range,
                "native function calls use positional arguments",
                "pass positional arguments in binding declaration order",
            ));
            return None;
        };
        if native.parameters.len() != arguments.len() {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::TypeMismatch,
                range,
                format!(
                    "native function `{name}` expects {} arguments but received {}",
                    native.parameters.len(),
                    arguments.len()
                ),
                "pass exactly one argument for each generated parameter",
            ));
        }
        for (argument, parameter) in arguments.iter().zip(&native.parameters) {
            if let Type::StaticHandler(signature) = parameter {
                self.check_static_handler_reference(argument, signature);
            } else {
                self.expect_native_callback_argument(argument, parameter, context);
            }
        }
        if context.in_finally && !context.in_task_cleanup && !native.cleanup_safe {
            self.invalid_finally(range, "this native call is not CleanupSafe");
        }
        context.facts.has_native = true;
        context.facts.has_cleanup_unsafe |= !native.cleanup_safe;
        context.facts.may_fail = true;
        context.facts.native_requirements.insert(name.to_owned());
        self.native_calls.insert(range, native.call_class);
        Some(native.result.clone())
    }

    fn expect_native_callback_argument(
        &mut self,
        argument: &Expression,
        parameter: &Type,
        context: &mut FunctionContext<'_>,
    ) {
        if self.closure_conversion && matches!(parameter, Type::FunctionReference(_)) {
            if let Some(actual) = self.check_expression(argument, Some(parameter), context) {
                if let (Type::Named(name), Type::FunctionReference(expected)) = (&actual, parameter)
                    && name.starts_with("$rinzu_closure_")
                {
                    let count = self
                        .declarations
                        .get(&format!("{name}_env"))
                        .and_then(|d| {
                            if let Declaration::Enum(e) = d {
                                Some(e.variants.len())
                            } else {
                                None
                            }
                        })
                        .unwrap_or(0);
                    let mut parameters = vec![actual.clone()];
                    parameters.extend(expected.parameters.clone());
                    let targets = (0..count)
                        .map(|i| format!("{name}_callback_{i}"))
                        .collect::<Vec<_>>();
                    if targets.is_empty()
                        || targets.iter().any(|target| {
                            self.functions.get(target).is_none_or(|s| {
                                s.parameters != parameters
                                    || s.result != *expected.result
                                    || s.is_async
                            })
                        })
                    {
                        self.diagnostics.push(Diagnostic::error(
                            DiagnosticCode::TypeMismatch,
                            argument.range,
                            "captured native callback lost its typed adapter",
                            "use a Main callback returning Scalar or Unit",
                        ));
                    } else {
                        self.native_captures.insert(argument.range, actual);
                        self.reference_targets.insert(argument.range, targets);
                        context.facts.requires_owner = true;
                    }
                } else {
                    self.require_same(parameter, &actual, argument.range);
                }
            }
        } else {
            self.expect_expression(argument, parameter, context);
        }
    }

    fn check_static_handler_reference(
        &mut self,
        expression: &Expression,
        expected: &StaticHandlerType,
    ) {
        let ExpressionKind::Name(path) = &expression.kind else {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::InvalidHandlerReference,
                expression.range,
                "generated callback parameter requires a top-level function name",
                "pass a capture-free top-level function directly at this call site",
            ));
            return;
        };
        if path.len() != 1 {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::InvalidHandlerReference,
                expression.range,
                "callback handler must resolve to one imported or local top-level function",
                "pass the function name directly without storing it in another value",
            ));
            return;
        }
        let name = &path[0];
        if matches!(self.declarations.get(name), Some(Declaration::Function(function)) if function.fallback.is_some())
        {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::UnsupportedSafeFfi,
                expression.range,
                "callback fallback requires the source v2 callback ABI",
                "use a source native callback declaration",
            ));
            return;
        }
        let Some(actual) = self.functions.get(name) else {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::InvalidHandlerReference,
                expression.range,
                format!("`{name}` is not a top-level function"),
                "declare a capture-free top-level function with the required signature",
            ));
            return;
        };
        let signature_matches = actual.parameters == expected.parameters
            && actual.result == *expected.result
            && actual.is_async == expected.is_async;
        let owner_matches = !expected.owner_sync || self.options.owner_sync_handlers.contains(name);
        if !signature_matches || !owner_matches {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::InvalidHandlerReference,
                expression.range,
                format!("handler `{name}` does not match the generated callback contract"),
                "match the generated parameter/result/async signature and callback class",
            ));
            return;
        }
        self.expression_types.insert(
            expression.range,
            Type::StaticHandler(Box::new(expected.clone())),
        );
    }

    fn check_function_call(
        &mut self,
        target: &[String],
        arguments: &Arguments,
        range: SourceRange,
        context: &mut FunctionContext<'_>,
        from_spawn: bool,
    ) -> Option<Type> {
        if from_spawn
            && target
                .first()
                .is_some_and(|first| context.locals.contains_key(first))
        {
            self.diagnostics.push(local_spawn_error(range));
            return None;
        }
        let name = target.join(".");
        let Some(signature) = self.functions.get(&name).cloned() else {
            self.unknown_name(&name, range);
            return None;
        };
        if !self.method_visible(&name, range, context) {
            return None;
        }
        if from_spawn && target.len() > 1 {
            self.method_calls.insert(
                range,
                ExpressionKind::Spawn {
                    target: vec![name.clone()],
                    arguments: arguments.clone(),
                },
            );
        }
        let Arguments::Positional(arguments) = arguments else {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::TypeMismatch,
                range,
                "function calls use positional arguments",
                "remove argument labels or call a record/enum constructor",
            ));
            return None;
        };
        if signature.parameters.len() != arguments.len() {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::TypeMismatch,
                range,
                format!(
                    "function `{name}` expects {} arguments but received {}",
                    signature.parameters.len(),
                    arguments.len()
                ),
                "pass exactly one positional argument for each parameter",
            ));
        }
        for (argument, parameter) in arguments.iter().zip(&signature.parameters) {
            if let Some(actual) = self.check_expression(argument, Some(parameter), context) {
                self.require_same(parameter, &actual, argument.range);
                if from_spawn && !self.is_transferable(&actual, &mut HashSet::new()) {
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::InvalidSpawn,
                        argument.range,
                        format!(
                            "spawn argument type `{actual}` is not Transferable; field path: {}",
                            self.sealed_failure_path(&actual, "Transferable")
                        ),
                        "pass only immutable Transferable values to `spawn`",
                    ));
                }
            }
        }
        if from_spawn {
            if signature.is_async {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::InvalidSpawn,
                    range,
                    format!("async function `{name}` cannot be a spawn callee"),
                    "spawn a synchronous WorkerSafe function",
                ));
            } else {
                context.facts.spawn_calls.push((name.clone(), range));
            }
            Some(signature.result)
        } else {
            if context.owner_sync && signature.is_async {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::InvalidAwait,
                    range,
                    format!("OwnerSync handler cannot call async function `{name}`"),
                    "call only synchronous functions from OwnerSync",
                ));
            }
            if signature
                .parameters
                .iter()
                .any(|ty| self.contains_call_only_type(ty, &mut HashSet::new()))
            {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::CallOnlyEscape,
                    range,
                    "CallOnly value cannot be passed to a managed function",
                    "pass it only to an approved synchronous native call",
                ));
            }
            context.facts.calls.insert(name.clone());
            if context.in_finally && !context.in_task_cleanup {
                context.facts.cleanup_calls.insert(name.clone(), range);
            }
            context.facts.may_fail = true;
            if signature.is_async {
                Some(Type::Future(Box::new(signature.result)))
            } else {
                Some(signature.result)
            }
        }
    }

    fn check_named_arguments(
        &mut self,
        fields: &[crate::ast::FieldDecl],
        arguments: &Arguments,
        range: SourceRange,
        context: &mut FunctionContext<'_>,
    ) -> bool {
        let Arguments::Named(arguments) = arguments else {
            if fields.is_empty()
                && matches!(arguments, Arguments::Positional(values) if values.is_empty())
            {
                return true;
            }
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::TypeMismatch,
                range,
                "record and enum constructors use named arguments",
                "label every field as `name: value`",
            ));
            return false;
        };
        let mut seen = HashSet::new();
        for (name, expression, argument_range) in arguments {
            if !seen.insert(name) {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::DuplicateName,
                    *argument_range,
                    format!("constructor argument `{name}` appears more than once"),
                    "provide each field exactly once",
                ));
                continue;
            }
            let Some(field) = fields.iter().find(|field| &field.name == name) else {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::UnknownName,
                    *argument_range,
                    format!("constructor has no field `{name}`"),
                    "use a declared field name",
                ));
                continue;
            };
            if let Some(expected) = self.resolve_type(&field.ty) {
                self.expect_expression(expression, &expected, context);
            }
        }
        let missing: Vec<_> = fields
            .iter()
            .filter(|field| !seen.contains(&field.name))
            .map(|field| field.name.as_str())
            .collect();
        if !missing.is_empty() {
            self.diagnostics.push(Diagnostic::error(
                DiagnosticCode::TypeMismatch,
                range,
                format!("constructor is missing fields: {}", missing.join(", ")),
                "provide every field exactly once",
            ));
        }
        missing.is_empty() && seen.len() == fields.len()
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
        function.result = self.generated_type_ref(&result, function.result.range);
        crate::lambdas::rewrite(&mut function, &names);
        function.parameters = parameters;
        self.functions.insert(
            function.name.clone(),
            FunctionSignature {
                parameters: parameter_types,
                result: result.clone(),
                is_async: function.is_async,
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

    fn resolve_type(&mut self, type_ref: &TypeRef) -> Option<Type> {
        self.resolve_type_with_constants(type_ref, &self.constants.clone())
    }
    fn resolve_type_with_constants(
        &mut self,
        type_ref: &TypeRef,
        values: &std::collections::BTreeMap<String, IntegerConstant>,
    ) -> Option<Type> {
        let result = self.resolve_type_with_constants_inner(type_ref, values);
        if let Some(ty) = &result {
            self.type_references.insert(type_ref.range, ty.clone());
        }
        result
    }
    fn resolve_type_with_constants_inner(
        &mut self,
        type_ref: &TypeRef,
        values: &std::collections::BTreeMap<String, IntegerConstant>,
    ) -> Option<Type> {
        if let TypeRefKind::Named(path) = &type_ref.kind {
            let qualified = path.join(".");
            for member in self
                .module
                .native_blocks
                .iter()
                .flat_map(|block| &block.members)
            {
                match member {
                    crate::ast::NativeMember::Opaque { name, .. } if *name == qualified => {
                        return Some(Type::Native {
                            name: qualified,
                            call_only: false,
                        });
                    }
                    crate::ast::NativeMember::Record(record) if record.name == qualified => {
                        return Some(Type::Named(qualified));
                    }
                    _ => {}
                }
            }
            if let Some(external) = self.options.external_types.get(&qualified) {
                return Some(Type::Native {
                    name: qualified,
                    call_only: external.call_only,
                });
            }
        }
        match &type_ref.kind {
            TypeRefKind::Record(fields) => {
                self.check_unique_fields(fields, "anonymous record");
                if fields.is_empty() {
                    self.type_error(type_ref.range, "record types require at least one field");
                    return None;
                }
                let fields = fields
                    .iter()
                    .map(|f| {
                        Some((
                            f.name.clone(),
                            self.resolve_type_with_constants(&f.ty, values)?,
                        ))
                    })
                    .collect::<Option<Vec<_>>>()?;
                self.intern_record(fields, type_ref.range)
            }
            TypeRefKind::Generic { .. } => unreachable!("generic types expanded before checking"),
            TypeRefKind::ConcreteBytes(length) => Some(Type::FixedBytes(*length)),
            TypeRefKind::Named(path) if path.len() == 1 => {
                let name = &path[0];
                let ty = match name.as_str() {
                    "Unit" => Type::Unit,
                    "Never" => Type::Never,
                    "Bool" => Type::Bool,
                    "Int" => Type::Int,
                    "Float" => Type::Float,
                    _ if AbiScalar::from_name(name).is_some() => {
                        Type::from_scalar(AbiScalar::from_name(name).expect("scalar alias"))
                    }
                    "String" => Type::String,
                    "Error" => Type::Error,
                    "SourceLocation" => self.source_location_type(type_ref.range),
                    "ByteArray" => Type::Bytes,
                    "Bytes" => {
                        self.diagnostics.push(Diagnostic::error(
                            DiagnosticCode::TypeMismatch,
                            type_ref.range,
                            "Bytes requires a fixed length",
                            "write Bytes(N) with a compile-time nonnegative integer",
                        ));
                        return None;
                    }
                    _ if matches!(
                        self.declarations.get(name),
                        Some(Declaration::Record(_) | Declaration::Enum(_) | Declaration::Error(_))
                    ) =>
                    {
                        Type::Named(name.clone())
                    }
                    _ => {
                        self.diagnostics.push(Diagnostic::error(
                            DiagnosticCode::UnknownType,
                            type_ref.range,
                            format!("unknown type `{name}`"),
                            "use a built-in type or a record/enum declared in this module",
                        ));
                        return None;
                    }
                };
                Some(ty)
            }
            TypeRefKind::Named(path) => {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::UnknownType,
                    type_ref.range,
                    format!(
                        "external type `{}` is not available in this compilation unit",
                        path.join(".")
                    ),
                    "import a generated safe binding or use a local declared type",
                ));
                None
            }
            TypeRefKind::Bytes(length) => {
                match crate::constants::evaluate_integer_expression(length, None, values) {
                    Ok(value) if value.value >= 0 && value.value <= u64::MAX as i128 => {
                        Some(Type::FixedBytes(value.value as u64))
                    }
                    Ok(_) => {
                        self.diagnostics.push(Diagnostic::error(
                            DiagnosticCode::TypeMismatch,
                            length.range,
                            "Bytes length must be nonnegative and fit CSize",
                            "use a representable nonnegative constant",
                        ));
                        None
                    }
                    Err(error) => {
                        self.diagnostics.push(error);
                        None
                    }
                }
            }
            TypeRefKind::Function {
                supplant,
                parameters,
                result,
            } => {
                let parameters = parameters
                    .iter()
                    .map(|parameter| self.resolve_type_with_constants(parameter, values))
                    .collect::<Option<Vec<_>>>()?;
                let result = self.resolve_type_with_constants(result, values)?;
                Some(Type::FunctionReference(Box::new(FunctionReferenceType {
                    parameters,
                    result: Box::new(result),
                    supplant: *supplant,
                })))
            }
            TypeRefKind::List(element) => {
                let value = self.resolve_type_with_constants(element, values)?;
                Some(Type::List(Box::new(value)))
            }
            TypeRefKind::Map(key, value) => {
                let key = self.resolve_type_with_constants(key, values)?;
                let value = self.resolve_type_with_constants(value, values)?;
                if !self.is_key(&key, &mut HashSet::new()) {
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::TypeMismatch,
                        type_ref.range,
                        format!("map key type `{key}` does not implement sealed `Key`; field path: {}", self.sealed_failure_path(&key, "Key")),
                        "use integers, Bool, String, bytes, or language record/enum containing only Key fields",
                    ));
                }
                Some(Type::Map(Box::new(key), Box::new(value)))
            }
            TypeRefKind::Future(value) => {
                let value = self.resolve_type_with_constants(value, values)?;
                Some(Type::Future(Box::new(value)))
            }
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

    fn validate_generic_requirements(&mut self) {
        for requirement in self.generic_requirements.clone() {
            let Some(ty) = self.resolve_type(&requirement.ty) else {
                continue;
            };
            let satisfied = match requirement.marker.as_str() {
                "Transferable" => self.is_transferable(&ty, &mut HashSet::new()),
                "Equality" => self.has_equality(&ty, &mut HashSet::new()),
                "Key" => self.is_key(&ty, &mut HashSet::new()),
                _ => false,
            };
            if !satisfied {
                let path = self.sealed_failure_path(&ty, &requirement.marker);
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::TypeMismatch,
                    requirement.range,
                    format!(
                        "concrete type `{ty}` does not satisfy sealed `{}` in `{}`; field path: {path}",
                        requirement.marker, requirement.instance
                    ),
                    "use a concrete type whose fields satisfy the compiler-owned marker",
                ));
            }
        }
    }

    fn sealed_failure_path(&self, ty: &Type, marker: &str) -> String {
        fn walk(
            checker: &Checker<'_>,
            ty: &Type,
            marker: &str,
            path: String,
            visiting: &mut HashSet<String>,
        ) -> Option<String> {
            let satisfied = match marker {
                "Transferable" => checker.is_transferable(ty, &mut HashSet::new()),
                "Equality" => checker.has_equality(ty, &mut HashSet::new()),
                "Key" => checker.is_key(ty, &mut HashSet::new()),
                _ => false,
            };
            if satisfied {
                return None;
            }
            match ty {
                Type::List(element) if marker != "Key" => walk(
                    checker,
                    element,
                    marker,
                    format!("{path}[element]"),
                    visiting,
                ),
                Type::Map(key, value) if marker != "Key" => {
                    let key_marker = if marker == "Transferable" {
                        "Key"
                    } else {
                        marker
                    };
                    walk(checker, key, key_marker, format!("{path}[key]"), visiting).or_else(|| {
                        walk(checker, value, marker, format!("{path}[value]"), visiting)
                    })
                }
                Type::Named(name) => {
                    if !visiting.insert(name.clone()) {
                        return Some(format!("{path} -> recursive `{name}`"));
                    }
                    let mut failed = None;
                    if let Some(Declaration::Enum(value)) = checker.declarations.get(name).cloned()
                    {
                        for variant in &value.variants {
                            for field in &variant.fields {
                                if let Some(child) = checker.resolve_type_readonly(&field.ty) {
                                    failed = walk(
                                        checker,
                                        &child,
                                        marker,
                                        format!("{path}.{}.{}", variant.name, field.name),
                                        visiting,
                                    );
                                    if failed.is_some() {
                                        break;
                                    }
                                }
                            }
                            if failed.is_some() {
                                break;
                            }
                        }
                    } else if let Some(record) = checker.record_declaration(name)
                        && (marker != "Key"
                            || matches!(
                                checker.declarations.get(name),
                                Some(Declaration::Record(_))
                            ))
                    {
                        for field in &record.fields {
                            if let Some(child) = checker.resolve_type_readonly(&field.ty) {
                                failed = walk(
                                    checker,
                                    &child,
                                    marker,
                                    format!("{path}.{}", field.name),
                                    visiting,
                                );
                                if failed.is_some() {
                                    break;
                                }
                            }
                        }
                    }
                    visiting.remove(name);
                    failed.or_else(|| Some(format!("{path} -> `{ty}` lacks {marker}")))
                }
                _ => Some(format!("{path} -> `{ty}` lacks {marker}")),
            }
        }
        walk(self, ty, marker, ty.to_string(), &mut HashSet::new())
            .unwrap_or_else(|| ty.to_string())
    }

    fn has_equality(&self, ty: &Type, visiting: &mut HashSet<String>) -> bool {
        match ty {
            Type::Unit
            | Type::Bool
            | Type::Int
            | Type::Float
            | Type::Scalar(_)
            | Type::String
            | Type::Bytes
            | Type::FixedBytes(_) => true,
            Type::List(element) => self.has_equality(element, visiting),
            Type::Map(key, value) => {
                self.has_equality(key, visiting) && self.has_equality(value, visiting)
            }
            Type::Named(name) => {
                self.aggregate_property(name, visiting, |checker, field, visiting| {
                    checker.has_equality(field, visiting)
                })
            }
            Type::Never
            | Type::Future(_)
            | Type::Native { .. }
            | Type::StaticHandler(_)
            | Type::FunctionReference(_)
            | Type::Error => false,
        }
    }

    fn is_key(&self, ty: &Type, visiting: &mut HashSet<String>) -> bool {
        match ty {
            Type::Int | Type::String | Type::Bool | Type::Bytes | Type::FixedBytes(_) => true,
            Type::Scalar(scalar) => scalar.integer_bounds().is_some(),
            Type::Named(name)
                if matches!(
                    self.declarations.get(name),
                    Some(Declaration::Enum(_) | Declaration::Record(_))
                ) =>
            {
                self.aggregate_property(name, visiting, |checker, field, visiting| {
                    checker.is_key(field, visiting)
                })
            }
            _ => false,
        }
    }

    fn is_transferable(&self, ty: &Type, visiting: &mut HashSet<String>) -> bool {
        match ty {
            Type::Unit
            | Type::Bool
            | Type::Int
            | Type::Float
            | Type::Scalar(_)
            | Type::String
            | Type::Bytes
            | Type::FixedBytes(_) => true,
            Type::List(element) => self.is_transferable(element, visiting),
            Type::Map(key, value) => {
                self.is_key(key, &mut HashSet::new()) && self.is_transferable(value, visiting)
            }
            Type::Named(name) => {
                self.aggregate_property(name, visiting, |checker, field, visiting| {
                    checker.is_transferable(field, visiting)
                })
            }
            Type::Never
            | Type::Error
            | Type::Future(_)
            | Type::Native { .. }
            | Type::StaticHandler(_)
            | Type::FunctionReference(_) => false,
        }
    }

    fn contains_call_only_type(&self, ty: &Type, visiting: &mut HashSet<String>) -> bool {
        match ty {
            Type::Native { call_only, .. } => *call_only,
            Type::StaticHandler(_) => false,
            Type::List(element) | Type::Future(element) => {
                self.contains_call_only_type(element, visiting)
            }
            Type::Map(key, value) => {
                self.contains_call_only_type(key, visiting)
                    || self.contains_call_only_type(value, visiting)
            }
            Type::Named(name) => {
                if !visiting.insert(name.clone()) {
                    return false;
                }
                let result = match self.declarations.get(name).cloned() {
                    Some(Declaration::Error(error)) => {
                        error.payload.as_ref().is_some_and(|payload| {
                            self.resolve_type_readonly(&payload.ty)
                                .is_some_and(|ty| self.contains_call_only_type(&ty, visiting))
                        })
                    }
                    Some(Declaration::Record(record)) => record.fields.iter().any(|field| {
                        self.resolve_type_readonly(&field.ty)
                            .is_some_and(|field_type| {
                                self.contains_call_only_type(&field_type, visiting)
                            })
                    }),
                    Some(Declaration::Enum(enum_decl)) => enum_decl
                        .variants
                        .iter()
                        .flat_map(|variant| &variant.fields)
                        .any(|field| {
                            self.resolve_type_readonly(&field.ty)
                                .is_some_and(|field_type| {
                                    self.contains_call_only_type(&field_type, visiting)
                                })
                        }),
                    _ => false,
                };
                visiting.remove(name);
                result
            }
            _ => false,
        }
    }

    fn aggregate_property(
        &self,
        name: &str,
        visiting: &mut HashSet<String>,
        property: impl Copy + Fn(&Self, &Type, &mut HashSet<String>) -> bool,
    ) -> bool {
        if !visiting.insert(name.to_owned()) {
            return false;
        }
        let result = match self.declarations.get(name).cloned() {
            Some(Declaration::Record(record)) => {
                (!record.mutable || record.fields.iter().all(|field| field.is_const))
                    && record.fields.iter().all(|field| {
                        self.resolve_type_readonly(&field.ty)
                            .is_some_and(|ty| property(self, &ty, visiting))
                    })
            }
            Some(Declaration::Enum(enum_decl)) => enum_decl
                .variants
                .iter()
                .flat_map(|variant| &variant.fields)
                .all(|field| {
                    self.resolve_type_readonly(&field.ty)
                        .is_some_and(|ty| property(self, &ty, visiting))
                }),
            // Native records share mutable targets. Scalar layout fields do
            // not give those references content equality or transfer semantics.
            _ => false,
        };
        visiting.remove(name);
        result
    }

    fn resolve_type_readonly(&self, type_ref: &TypeRef) -> Option<Type> {
        if let TypeRefKind::Named(path) = &type_ref.kind {
            let qualified = path.join(".");
            if self.module.native_blocks.iter().flat_map(|block| &block.members).any(|member|
                matches!(member, crate::ast::NativeMember::Opaque { name, .. } if *name == qualified)) {
                return Some(Type::Native { name: qualified, call_only: false });
            }
            if let Some(external) = self.options.external_types.get(&qualified) {
                return Some(Type::Native {
                    name: qualified,
                    call_only: external.call_only,
                });
            }
        }
        match &type_ref.kind {
            TypeRefKind::Record(fields) => {
                let fields = fields
                    .iter()
                    .map(|f| Some((f.name.clone(), self.resolve_type_readonly(&f.ty)?)))
                    .collect::<Option<Vec<_>>>()?;
                Some(Type::Named(crate::records::identity(&fields)))
            }
            TypeRefKind::Named(path) if path.len() == 1 => Some(match path[0].as_str() {
                "Unit" => Type::Unit,
                "Never" => Type::Never,
                "Bool" => Type::Bool,
                "Int" => Type::Int,
                "Float" => Type::Float,
                name if AbiScalar::from_name(name).is_some() => {
                    Type::from_scalar(AbiScalar::from_name(name).expect("scalar alias"))
                }
                "String" => Type::String,
                "Error" => Type::Error,
                "SourceLocation" => Type::Named(errors::SOURCE_LOCATION.into()),
                "Bytes" | "ByteArray" => Type::Bytes,
                name => Type::Named(name.to_owned()),
            }),
            TypeRefKind::Generic { .. } | TypeRefKind::Named(_) => None,
            TypeRefKind::Function {
                supplant,
                parameters,
                result,
            } => Some(Type::FunctionReference(Box::new(FunctionReferenceType {
                supplant: *supplant,
                parameters: parameters
                    .iter()
                    .map(|p| self.resolve_type_readonly(p))
                    .collect::<Option<Vec<_>>>()?,
                result: Box::new(self.resolve_type_readonly(result)?),
            }))),
            TypeRefKind::ConcreteBytes(length) => Some(Type::FixedBytes(*length)),
            TypeRefKind::Bytes(length) => {
                crate::constants::evaluate_integer_expression(length, None, &self.constants)
                    .ok()
                    .filter(|value| value.value >= 0 && value.value <= u64::MAX as i128)
                    .map(|value| Type::FixedBytes(value.value as u64))
            }
            TypeRefKind::List(element) => self
                .resolve_type_readonly(element)
                .map(|value| Type::List(Box::new(value))),
            TypeRefKind::Map(key, value) => Some(Type::Map(
                Box::new(self.resolve_type_readonly(key)?),
                Box::new(self.resolve_type_readonly(value)?),
            )),
            TypeRefKind::Future(value) => self
                .resolve_type_readonly(value)
                .map(|value| Type::Future(Box::new(value))),
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

pub(crate) fn block_always_returns(block: &Block) -> bool {
    block
        .statements
        .last()
        .is_some_and(statement_always_returns)
}

fn statement_always_returns(statement: &Statement) -> bool {
    match &statement.kind {
        StatementKind::Block(block) => block_always_returns(block),
        StatementKind::Return(_) | StatementKind::Throw(_) => true,
        StatementKind::If {
            then_block,
            else_branch: Some(else_branch),
            ..
        } => block_always_returns(then_block) && statement_always_returns(else_branch),
        // Exhaustiveness is checked separately. A valid empty switch consumes
        // an uninhabited enum and therefore has no normal fallthrough path.
        StatementKind::Switch { cases, .. } => {
            cases.iter().all(|case| block_always_returns(&case.body))
        }
        StatementKind::TryErrors {
            body,
            handlers,
            cleanup,
            ..
        } => {
            cleanup.as_ref().is_some_and(block_always_returns)
                || (block_always_returns(body)
                    && handlers
                        .iter()
                        .all(|handler| block_always_returns(&handler.body)))
        }
        StatementKind::TryCatch { body, handler, .. } => {
            block_always_returns(body) && block_always_returns(handler)
        }
        StatementKind::TryFinally { body, cleanup }
        | StatementKind::TaskScope { body, cleanup } => {
            block_always_returns(body) || block_always_returns(cleanup)
        }
        _ => false,
    }
}

pub(crate) fn source_abi_type(abi: &crate::SourceAbiType) -> Type {
    use crate::SourceAbiType as A;
    match abi {
        A::Unit => Type::Unit,
        A::Scalar(scalar) => Type::from_scalar(*scalar),
        A::String => Type::String,
        A::Bytes(n) => Type::FixedBytes(*n),
        A::ByteArray | A::ByteArrayResult => Type::Bytes,
        A::Opaque(name) => Type::Native {
            name: name.clone(),
            call_only: false,
        },
        A::Record(name) | A::UserData(name) => Type::Named(name.clone()),
        A::Callback { parameters, result } => {
            Type::FunctionReference(Box::new(FunctionReferenceType {
                parameters: parameters.iter().map(source_abi_type).collect(),
                result: Box::new(source_abi_type(result)),
                supplant: false,
            }))
        }
    }
}
