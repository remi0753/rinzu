//! Source declaration validation shared by semantic checking and artifact planning.
use std::collections::HashMap;

use crate::ast::{Module, NativeBinding, NativeFunctionDecl, NativeMember, TypeRef, TypeRefKind};
use crate::{AbiScalar, Diagnostic, DiagnosticCode, SourceRange};

/// Validate names and contracts that do not depend on expression/effect checking.
/// Physical prototypes and target layout are checked by the target build stage.
pub fn validate_native_declarations(module: &Module) -> Result<(), Vec<Diagnostic>> {
    let mut errors = Vec::new();
    let mut names: HashMap<String, SourceRange> = module
        .declarations
        .iter()
        .map(|declaration| (declaration.name().to_owned(), declaration.range()))
        .collect();
    for member in module.native_blocks.iter().flat_map(|block| &block.members) {
        let (name, range) = match member {
            NativeMember::Opaque { name, range } => (name, *range),
            NativeMember::Record(record) => (&record.name, record.range),
            NativeMember::Function(function) => (&function.name, function.range),
        };
        if let Some(previous) = names.insert(name.clone(), range) {
            errors.push(duplicate(name, range, previous));
        }
        match member {
            NativeMember::Record(record) => {
                if record.fields.is_empty() {
                    errors.push(invalid(
                        record.range,
                        "empty native records are unsupported",
                        "declare at least one ABI scalar field",
                    ));
                }
                let mut fields = HashMap::new();
                for field in &record.fields {
                    if let Some(previous) = fields.insert(&field.name, field.range) {
                        errors.push(duplicate(&field.name, field.range, previous));
                    }
                    if scalar(&field.ty).is_none() {
                        errors.push(invalid(
                            field.ty.range,
                            "native record fields require ABI scalars",
                            "use a scalar or built-in scalar alias",
                        ));
                    }
                }
            }
            NativeMember::Function(function) => validate_binding(function, &mut errors),
            _ => {}
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn scalar(ty: &TypeRef) -> Option<AbiScalar> {
    match &ty.kind {
        TypeRefKind::Named(path) if path.len() == 1 => AbiScalar::from_name(&path[0]),
        _ => None,
    }
}
fn unit(ty: &TypeRef) -> bool {
    matches!(&ty.kind, TypeRefKind::Named(path) if path == &["Unit"])
}
fn validate_binding(function: &NativeFunctionDecl, errors: &mut Vec<Diagnostic>) {
    let mut parameters = HashMap::new();
    for parameter in &function.parameters {
        if let Some(previous) = parameters.insert(&parameter.name, parameter.range) {
            errors.push(duplicate(&parameter.name, parameter.range, previous));
        }
    }
    match &function.binding {
        NativeBinding::Symbol(symbol) => {
            let mut bytes = symbol.bytes();
            let first = bytes.next();
            if !first.is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
                || !bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                errors.push(invalid(
                    function.binding_range,
                    "native symbol must be a nonempty ASCII C identifier",
                    "provide an unmangled C symbol",
                ));
            }
            if function.is_async {
                errors.push(invalid(
                    function.range,
                    "native declarations cannot be async",
                    "use a standard-library async function or a synchronous symbol declaration",
                ));
            }
        }
        NativeBinding::Runtime(_) => errors.push(invalid(
            function.binding_range,
            "runtime library bindings cannot be declared in native blocks",
            "call a standard-library function",
        )),
    }
}
fn invalid(range: SourceRange, message: &str, help: &str) -> Diagnostic {
    Diagnostic::error(DiagnosticCode::UnsupportedSafeFfi, range, message, help)
}
fn duplicate(name: &str, range: SourceRange, previous: SourceRange) -> Diagnostic {
    Diagnostic::error(
        DiagnosticCode::DuplicateName,
        range,
        format!("duplicate native interface name `{name}`"),
        "rename or remove one declaration",
    )
    .with_secondary(previous, "first declaration is here")
}

/// A closed source ABI shape; each pointer corresponds to one physical C argument.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceAbiType {
    Unit,
    Scalar(AbiScalar),
    String,
    Bytes(u64),
    /// Ordinary read-only pointer input; the SDK declares an explicit length.
    ByteArray,
    /// Ordinary result only: pointer to a stable data/length descriptor.
    /// Distinct from the one-pointer ByteArray input ABI.
    ByteArrayResult,
    Opaque(String),
    Record(String),
    UserData(String),
    Callback {
        parameters: Vec<Self>,
        result: Box<Self>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceNativeSignature {
    pub name: String,
    pub binding: NativeBinding,
    pub is_supplant: bool,
    pub is_async: bool,
    pub parameters: Vec<SourceAbiType>,
    pub result: SourceAbiType,
}

impl SourceAbiType {
    /// Foreign delivery is derived from the closed callback signature. This
    /// does not establish Main/loop progress or authorize an invocation.
    #[must_use]
    pub fn foreign_callback_eligible(&self) -> bool {
        let Self::Callback { parameters, result } = self else {
            return false;
        };
        parameters
            .iter()
            .all(|parameter| matches!(parameter, Self::Scalar(_) | Self::String | Self::Bytes(_)))
            && matches!(
                **result,
                Self::Unit | Self::Scalar(_) | Self::String | Self::Bytes(_)
            )
    }
    #[must_use]
    pub fn canonical_name(&self) -> String {
        match self {
            Self::Unit => "Unit".into(),
            Self::Scalar(scalar) => scalar.name().into(),
            Self::String => "String".into(),
            Self::Bytes(length) => format!("Bytes({length})"),
            Self::ByteArray => "ByteArray".into(),
            Self::ByteArrayResult => "NativeResult<ByteArray>".into(),
            Self::Opaque(name) | Self::Record(name) => name.clone(),
            Self::UserData(name) => format!("UserData<{name}>"),
            Self::Callback { parameters, result } => format!(
                "fn({})->{}",
                parameters
                    .iter()
                    .map(Self::canonical_name)
                    .collect::<Vec<_>>()
                    .join(","),
                result.canonical_name()
            ),
        }
    }
}

impl SourceNativeSignature {
    #[must_use]
    pub fn canonical_type(&self) -> String {
        let prefix = if self.is_async {
            "async"
        } else if self.is_supplant {
            "supplant"
        } else {
            "native"
        };
        format!(
            "{prefix}({})->{}",
            self.parameters
                .iter()
                .map(SourceAbiType::canonical_name)
                .collect::<Vec<_>>()
                .join(","),
            self.result.canonical_name()
        )
    }
}

/// Resolve native signatures without guessing pointer contracts or target layout.
pub fn source_native_signatures(
    module: &Module,
) -> Result<Vec<SourceNativeSignature>, Vec<Diagnostic>> {
    let module = crate::records::expand_aliases(module)?;
    let (module, _) = crate::generics::expand(&module, true)?;
    crate::sema::resolve_source_native_signatures(&module)
}

pub(crate) fn source_native_signatures_with_records(
    module: &Module,
    mut record: impl FnMut(
        &TypeRef,
        &std::collections::BTreeMap<String, crate::IntegerConstant>,
    ) -> Option<String>,
) -> Result<Vec<SourceNativeSignature>, Vec<Diagnostic>> {
    validate_native_declarations(module)?;
    let constants = crate::evaluate_module_constants(module)?;
    let mut types = HashMap::new();
    for member in module.native_blocks.iter().flat_map(|b| &b.members) {
        match member {
            NativeMember::Opaque { name, .. } => {
                types.insert(name.clone(), SourceAbiType::Opaque(name.clone()));
            }
            NativeMember::Record(record) => {
                types.insert(
                    record.name.clone(),
                    SourceAbiType::Record(record.name.clone()),
                );
            }
            _ => {}
        }
    }
    let mut errors = Vec::new();
    let mut signatures = Vec::new();
    for member in module.native_blocks.iter().flat_map(|b| &b.members) {
        let NativeMember::Function(function) = member else {
            continue;
        };
        let mut signature_constants = constants.clone();
        for parameter in &function.parameters {
            signature_constants.remove(&parameter.name);
        }
        let mut resolver = AbiResolver {
            types: types.clone(),
            constants: signature_constants,
            record: &mut record,
        };
        let resolved = (|| {
            let parameters = function
                .parameters
                .iter()
                .map(|p| {
                    resolver.resolve(
                        &p.ty,
                        AbiPosition::Parameter {
                            storage: !function.is_supplant,
                        },
                        true,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            let result = resolver.resolve(&function.result, AbiPosition::Result, false)?;
            if function.is_supplant && result == SourceAbiType::ByteArrayResult {
                return Err(invalid(
                    function.result.range,
                    "ByteArray results require an ordinary synchronous native function",
                    "return a fixed Bytes value from a supplant function",
                ));
            }
            Ok::<_, Diagnostic>(SourceNativeSignature {
                name: function.name.clone(),
                binding: function.binding.clone(),
                is_supplant: function.is_supplant,
                is_async: function.is_async,
                parameters,
                result,
            })
        })();
        match resolved {
            Ok(signature) => signatures.push(signature),
            Err(error) => errors.push(error),
        }
    }
    if errors.is_empty() {
        Ok(signatures)
    } else {
        Err(errors)
    }
}
#[derive(Clone, Copy)]
enum AbiPosition {
    Parameter { storage: bool },
    Result,
    CallbackParameter,
    CallbackResult,
}
type RecordResolver<'a> = dyn FnMut(&TypeRef, &std::collections::BTreeMap<String, crate::IntegerConstant>) -> Option<String>
    + 'a;
struct AbiResolver<'a> {
    types: HashMap<String, SourceAbiType>,
    constants: std::collections::BTreeMap<String, crate::IntegerConstant>,
    record: &'a mut RecordResolver<'a>,
}
impl AbiResolver<'_> {
    fn resolve(
        &mut self,
        ty: &TypeRef,
        position: AbiPosition,
        callback: bool,
    ) -> Result<SourceAbiType, Diagnostic> {
        let result = match &ty.kind {
            TypeRefKind::Named(path) => {
                let name = path.join(".");
                if let Some(scalar) = scalar(ty) {
                    SourceAbiType::Scalar(scalar)
                } else if unit(ty)
                    && matches!(position, AbiPosition::Result | AbiPosition::CallbackResult)
                {
                    SourceAbiType::Unit
                } else if name == "String" {
                    SourceAbiType::String
                } else if name == "ByteArray"
                    && matches!(position, AbiPosition::Parameter { storage: true })
                {
                    SourceAbiType::ByteArray
                } else if name == "ByteArray" && matches!(position, AbiPosition::Result) {
                    SourceAbiType::ByteArrayResult
                } else if let Some(value) = self.types.get(&name) {
                    value.clone()
                } else if let Some(identity) = (self.record)(ty, &self.constants) {
                    SourceAbiType::UserData(identity)
                } else {
                    return Err(invalid(
                        ty.range,
                        "type has no source native ABI mapping",
                        "use a scalar, String, fixed Bytes, native type or ordinary structural record; wrap a class or enum in a record",
                    ));
                }
            }
            TypeRefKind::Record(_) => {
                let identity = (self.record)(ty, &self.constants).ok_or_else(|| {
                    invalid(
                        ty.range,
                        "invalid ordinary record at the native boundary",
                        "use a finite structural record with valid Rinzu field types",
                    )
                })?;
                SourceAbiType::UserData(identity)
            }
            TypeRefKind::Bytes(length) => {
                let value =
                    crate::constants::evaluate_integer_expression(length, None, &self.constants)?;
                let length = u64::try_from(value.value).map_err(|_| {
                    invalid(
                        ty.range,
                        "native Bytes length must fit nonnegative CSize",
                        "use a representable constant",
                    )
                })?;
                SourceAbiType::Bytes(length)
            }
            TypeRefKind::Function {
                supplant: false,
                parameters,
                result,
            } if callback => SourceAbiType::Callback {
                parameters: parameters
                    .iter()
                    .map(|p| self.resolve(p, AbiPosition::CallbackParameter, false))
                    .collect::<Result<_, _>>()?,
                result: Box::new(self.resolve(result, AbiPosition::CallbackResult, false)?),
            },
            _ => {
                return Err(invalid(
                    ty.range,
                    "type or effect is unsupported at this native ABI position",
                    "callbacks must be synchronous and use supported parameter and result types",
                ));
            }
        };
        let invalid_context_result = matches!(
            (&result, position),
            (SourceAbiType::UserData(_), AbiPosition::Result)
        ) && (self.record)(ty, &self.constants).as_ref()
            != match &result {
                SourceAbiType::UserData(name) => Some(name),
                _ => None,
            };
        if invalid_context_result {
            return Err(invalid(
                ty.range,
                "only ordinary structural records have a context result ABI",
                "wrap a class or enum in an ordinary record",
            ));
        }
        if matches!(
            (&result, position),
            (SourceAbiType::Record(_), AbiPosition::CallbackResult)
                | (SourceAbiType::UserData(_), AbiPosition::CallbackResult)
        ) {
            return Err(invalid(
                ty.range,
                "record callback results are unsupported",
                "return a supported scalar, String, Bytes or opaque reference",
            ));
        }
        Ok(result)
    }
}
