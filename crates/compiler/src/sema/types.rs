//! Semantic types, type-reference resolution and compiler-owned markers.
use std::collections::HashSet;
use std::fmt;

use super::{Checker, errors};
use crate::ast::{Declaration, TypeRef, TypeRefKind};
use crate::{AbiScalar, Diagnostic, DiagnosticCode, IntegerConstant, SourceRange};

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
    /// None uses call-target inference. An empty bound is explicit nothrow.
    pub error_bound: Option<Vec<String>>,
}

impl FunctionReferenceType {
    pub(crate) fn same_signature(&self, other: &Self) -> bool {
        self.supplant == other.supplant
            && self.parameters == other.parameters
            && self.result == other.result
    }

    pub(crate) fn contract(&self, range: SourceRange) -> Option<Box<crate::ast::ErrorContract>> {
        self.error_bound.as_ref().map(|types| {
            Box::new(crate::ast::ErrorContract {
                types: types
                    .iter()
                    .map(|name| TypeRef {
                        kind: TypeRefKind::Named(vec![name.clone()]),
                        range,
                    })
                    .collect(),
                range,
            })
        })
    }

    pub(crate) fn runtime_shape(&self) -> Self {
        let mut signature = self.clone();
        signature.error_bound = None;
        signature
    }
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
                write!(
                    formatter,
                    "){}->{}",
                    crate::ast::error_bound_spelling(signature.error_bound.as_deref()),
                    signature.result
                )
            }
            Self::Error => formatter.write_str("Error"),
        }
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
                error_bound: None,
                parameters: parameters.iter().map(source_abi_type).collect(),
                result: Box::new(source_abi_type(result)),
                supplant: false,
            }))
        }
    }
}

impl Checker<'_> {
    pub(super) fn resolve_type(&mut self, type_ref: &TypeRef) -> Option<Type> {
        self.resolve_type_with_constants(type_ref, &self.constants.clone())
    }
    pub(super) fn resolve_type_with_constants(
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
                error_contract,
            } => {
                let parameters = parameters
                    .iter()
                    .map(|parameter| self.resolve_type_with_constants(parameter, values))
                    .collect::<Option<Vec<_>>>()?;
                let result = self.resolve_type_with_constants(result, values)?;
                let error_bound = error_contract
                    .as_ref()
                    .map(|contract| self.resolve_error_bound(contract, values));
                Some(Type::FunctionReference(Box::new(FunctionReferenceType {
                    parameters,
                    result: Box::new(result),
                    supplant: *supplant,
                    error_bound,
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

    pub(super) fn validate_generic_requirements(&mut self) {
        for requirement in self.generic_requirements.clone() {
            let Some(ty) = self.resolve_type(&requirement.ty) else {
                continue;
            };
            if !self.satisfies_marker(&ty, &requirement.marker) {
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

    fn satisfies_marker(&self, ty: &Type, marker: &str) -> bool {
        let mut visiting = HashSet::new();
        match marker {
            "Transferable" => self.is_transferable(ty, &mut visiting),
            "Equality" => self.has_equality(ty, &mut visiting),
            "Key" => self.is_key(ty, &mut visiting),
            _ => false,
        }
    }

    pub(super) fn sealed_failure_path(&self, ty: &Type, marker: &str) -> String {
        fn walk(
            checker: &Checker<'_>,
            ty: &Type,
            marker: &str,
            path: String,
            visiting: &mut HashSet<String>,
        ) -> Option<String> {
            if checker.satisfies_marker(ty, marker) {
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

    pub(super) fn has_equality(&self, ty: &Type, visiting: &mut HashSet<String>) -> bool {
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

    pub(super) fn is_key(&self, ty: &Type, visiting: &mut HashSet<String>) -> bool {
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

    pub(super) fn is_transferable(&self, ty: &Type, visiting: &mut HashSet<String>) -> bool {
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

    pub(super) fn contains_call_only_type(
        &self,
        ty: &Type,
        visiting: &mut HashSet<String>,
    ) -> bool {
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

    pub(super) fn resolve_type_readonly(&self, type_ref: &TypeRef) -> Option<Type> {
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
                error_contract,
            } => Some(Type::FunctionReference(Box::new(FunctionReferenceType {
                supplant: *supplant,
                parameters: parameters
                    .iter()
                    .map(|p| self.resolve_type_readonly(p))
                    .collect::<Option<Vec<_>>>()?,
                result: Box::new(self.resolve_type_readonly(result)?),
                error_bound: if let Some(contract) = error_contract {
                    let mut types: Vec<_> = contract
                        .types
                        .iter()
                        .map(|ty| self.resolve_type_readonly(ty).map(|ty| ty.to_string()))
                        .collect::<Option<_>>()?;
                    types.sort();
                    Some(types)
                } else {
                    None
                },
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
}
