//! Source-package v2 wire format and generated C table (frontend-independent R1 boundary).
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageIdentity {
    pub package: String,
    pub modules: Vec<String>,
    pub exports: Vec<String>,
    pub language: String,
    pub target: String,
    pub data_layout: String,
    pub toolchain: String,
    pub native_compiler: String,
    /// Relative input identities and content digests, flags, libraries; no timestamps/addresses.
    pub build_inputs: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldLayout {
    pub name: String,
    pub c_type: String,
    pub offset: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypeRecord {
    pub identity: String,
    pub canonical: String,
    pub c_type: String,
    pub size: String,
    pub alignment: String,
    pub bytes_length: Option<String>,
    pub fields: Vec<FieldLayout>,
}

/// Multiple SDK contracts may mention the same canonical native record.
/// C tag spelling is local to each contract; every ABI property must agree.
pub fn matching_record_layout<'a>(
    records: impl Iterator<Item = &'a TypeRecord>,
    identity: &str,
) -> Result<&'a TypeRecord, String> {
    let mut records = records.filter(|record| record.identity == identity);
    let first = records
        .next()
        .ok_or_else(|| format!("missing validated native record layout: {identity}"))?;
    for record in records {
        if record.canonical != first.canonical
            || record.size != first.size
            || record.alignment != first.alignment
            || record.bytes_length != first.bytes_length
            || record.fields != first.fields
        {
            return Err(format!("conflicting native record layout: {identity}"));
        }
    }
    Ok(first)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallbackRecord {
    pub source_type: String,
    pub foreign_to_main: bool,
    pub lifetime: String,
    pub gate: String,
    pub origin: String,
    pub parent_scope: String,
    pub target: String,
    pub code_version: u32,
    pub fallback: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FunctionKind {
    Sync,
    Async,
    Native,
    Supplant,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FallbackRecord {
    pub kind: String,
    pub scalar: Option<String>,
    /// Decimal integer, float bits, Boolean or literal text; never a JSON numeric ABI value.
    pub value: String,
    pub bytes_length: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeRecord {
    pub declaration: String,
    pub version: u32,
    pub fallback: Option<FallbackRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallbackSiteRecord {
    pub native_function: String,
    pub parameter: usize,
    pub source_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<String>,
    pub targets: Vec<CodeRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionRecord {
    pub identity: String,
    pub source_type: String,
    pub kind: FunctionKind,
    pub effects: Vec<String>,
    #[serde(default)]
    pub error_contract: Option<ErrorContractRecord>,
    pub parameter_requirements: Vec<String>,
    pub symbol: Option<String>,
    pub service: Option<String>,
    /// Complete C declaration without a semicolon (generated contract header).
    pub c_prototype: String,
    pub conversion_plan: Vec<String>,
    pub callbacks: Vec<CallbackRecord>,
    #[serde(default)]
    pub callback_sites: Vec<CallbackSiteRecord>,
    #[serde(default)]
    pub code: Option<CodeRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorContractRecord {
    pub declared: bool,
    pub types: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendRecord {
    /// Native references, call-scoped context tokens, aliasing and atomic writeback.
    pub native_data_contract: u32,
    pub callback_rule: u32,
    pub template_layout: u32,
    pub service_registry: u32,
    pub page_size: String,
    pub entry_stride: String,
    pub control_stride: String,
    pub bridge_abi: String,
    pub owner_dispatcher: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceTypeRecord {
    pub kind: String,
    #[serde(default)]
    pub const_fields: Vec<u32>,
    #[serde(default)]
    pub private_fields: Vec<u32>,
    pub fields: Vec<(String, String)>,
    pub variants: Vec<(String, Vec<(String, String)>)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceTemplateRecord {
    pub kind: String,
    pub type_parameters: Vec<(String, Vec<String>)>,
    #[serde(default)]
    pub owner_type_parameters: Vec<(String, Vec<String>)>,
    #[serde(default)]
    pub interface_constraints: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub interfaces: Vec<String>,
    #[serde(default)]
    pub error_contract: Option<Vec<String>>,
    #[serde(default)]
    pub methods: Vec<(String, String)>,
    pub signature: String,
    pub const_fields: Vec<u32>,
    #[serde(default)]
    pub private_fields: Vec<u32>,
    pub fields: Vec<(String, String)>,
    pub variants: Vec<(String, Vec<(String, String)>)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceInterface {
    pub package: PackageIdentity,
    pub types: Vec<TypeRecord>,
    pub functions: Vec<FunctionRecord>,
    pub backend: BackendRecord,
    pub constants: BTreeMap<String, String>,
    /// Canonical integer type for each exact decimal constant, including aliases.
    pub constant_types: BTreeMap<String, String>,
    #[serde(default)]
    pub public_bindings: BTreeMap<String, String>,
    #[serde(default)]
    pub source_types: BTreeMap<String, SourceTypeRecord>,
    #[serde(default)]
    pub source_templates: BTreeMap<String, SourceTemplateRecord>,
    #[serde(default)]
    pub source_values: BTreeMap<String, String>,
    /// Digest of optional caller-supplied header; verified by the frontend in R2.
    pub provided_header_digest: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceDescriptor {
    pub schema: String,
    pub major: u16,
    pub minor: u16,
    pub interface: SourceInterface,
    pub import_keys: Vec<String>,
    pub semantic_digest: String,
    /// Digest of declared native inputs, separate from semantic identity.
    pub native_artifact_digest: String,
    pub generated_header_digest: String,
    pub glue_identity: String,
}

pub struct GeneratedSourcePackage {
    pub descriptor: SourceDescriptor,
    pub wire: Vec<u8>,
    pub header: String,
    pub c_table: String,
}

pub fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn json<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    serde_json::to_vec(value).map_err(|error| error.to_string())
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().enumerate().all(|(index, byte)| {
            byte == b'_' || byte.is_ascii_alphabetic() || (index > 0 && byte.is_ascii_digit())
        })
}

fn decimal(value: &str) -> bool {
    let magnitude = value.strip_prefix('-').unwrap_or(value);
    !magnitude.is_empty()
        && magnitude.bytes().all(|byte| byte.is_ascii_digit())
        && (magnitude == "0" || !magnitude.starts_with('0'))
        && value != "-0"
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn integer_bounds(scalar: &str) -> Option<(i128, i128)> {
    Some(match scalar {
        "CInt8" => (i8::MIN as i128, i8::MAX as i128),
        "CInt16" => (i16::MIN as i128, i16::MAX as i128),
        "CInt32" => (i32::MIN as i128, i32::MAX as i128),
        "Int" => (i64::MIN as i128, i64::MAX as i128),
        "CUInt8" => (0, u8::MAX as i128),
        "CUInt16" => (0, u16::MAX as i128),
        "CUInt32" => (0, u32::MAX as i128),
        "CUInt64" => (0, u64::MAX as i128),
        _ => return None,
    })
}

impl FallbackRecord {
    fn validate(&self) -> Result<(), String> {
        let valid = match self.kind.as_str() {
            "unit" => self.scalar.is_none() && self.value.is_empty() && self.bytes_length.is_none(),
            "boolean" => {
                self.scalar.is_none()
                    && matches!(self.value.as_str(), "true" | "false")
                    && self.bytes_length.is_none()
            }
            "string" => self.scalar.is_none() && self.bytes_length.is_none(),
            "bytes" => {
                self.scalar.is_none()
                    && self.value.len().is_multiple_of(2)
                    && self
                        .value
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                    && self.bytes_length.as_ref().is_some_and(|length| {
                        decimal(length)
                            && length
                                .parse::<u64>()
                                .is_ok_and(|length| self.value.len() as u64 / 2 <= length)
                    })
            }
            "float-bits" => {
                self.bytes_length.is_none()
                    && decimal(&self.value)
                    && self
                        .value
                        .parse::<u64>()
                        .is_ok_and(|bits| match self.scalar.as_deref() {
                            Some("CFloat") => bits <= u64::from(u32::MAX),
                            Some("Float") => true,
                            _ => false,
                        })
            }
            "integer" => {
                self.bytes_length.is_none()
                    && decimal(&self.value)
                    && self.value.parse::<i128>().is_ok_and(|value| {
                        self.scalar
                            .as_deref()
                            .and_then(integer_bounds)
                            .is_some_and(|(min, max)| min <= value && value <= max)
                    })
            }
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err("invalid closed callback fallback".to_owned())
        }
    }
}

impl SourceInterface {
    /// Derive a complete, positional plan from the checked physical signature.
    /// Generation rebuilds this plan; loading only accepts the exact derived plan.
    pub fn native_conversion_plan(&self, function: &FunctionRecord) -> Result<Vec<String>, String> {
        if function.code.is_some() {
            return Ok(Vec::new());
        }
        fn signature(value: &str) -> Result<(Vec<&str>, &str), String> {
            let start = value.find('(').ok_or("missing native signature")?;
            let mut depth = 1usize;
            let mut end = None;
            for (offset, character) in value[start + 1..].char_indices() {
                match character {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(start + 1 + offset);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let end = end.ok_or("unclosed native signature")?;
            let result = value[end + 1..]
                .strip_prefix("->")
                .ok_or("missing native result")?;
            let input = &value[start + 1..end];
            let mut parameters = Vec::new();
            let mut begin = 0;
            let mut depth = 0;
            for (offset, character) in input.char_indices() {
                match character {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    ',' if depth == 0 => {
                        parameters.push(&input[begin..offset]);
                        begin = offset + 1;
                    }
                    _ => {}
                }
            }
            if !input.is_empty() {
                parameters.push(&input[begin..]);
            }
            Ok((parameters, result))
        }
        fn append(
            interface: &SourceInterface,
            ty: &str,
            position: &str,
            input: bool,
            callback: bool,
            plan: &mut Vec<String>,
        ) -> Result<(), String> {
            let operation = if ty.starts_with("fn(") {
                if !input || callback {
                    return Err("unsupported native callback position".into());
                }
                plan.push(format!("{position}:callback-current-call-v1:{ty}"));
                let (parameters, result) = signature(ty)?;
                for (index, parameter) in parameters.iter().enumerate() {
                    append(
                        interface,
                        parameter,
                        &format!("{position}.callback.arg[{index}]"),
                        false,
                        true,
                        plan,
                    )?;
                }
                return append(
                    interface,
                    result,
                    &format!("{position}.callback.result"),
                    true,
                    true,
                    plan,
                );
            } else if let Some(identity) = ty
                .strip_prefix("UserData<")
                .and_then(|value| value.strip_suffix('>'))
            {
                if !interface
                    .source_types
                    .get(identity)
                    .is_some_and(|schema| schema.kind == "record")
                {
                    return Err("context token requires its ordinary record schema".into());
                }
                if callback && input {
                    return Err("context callback result is unsupported".into());
                }
                if input {
                    "context-token-input-v1"
                } else if callback {
                    "context-token-current-call-argument-v1"
                } else {
                    "context-token-current-call-result-v1"
                }
            } else if let Some(schema) = interface.source_types.get(ty) {
                match schema.kind.as_str() {
                    "native-record" => {
                        if callback && input {
                            return Err("native record callback result is unsupported".into());
                        }
                        if input {
                            "native-record-loan-writeback-v1"
                        } else {
                            "native-record-restore-or-copy-v1"
                        }
                    }
                    "opaque" => "opaque-reference-domain-v1",
                    _ => return Err(format!("unsupported direct native data type: {ty}")),
                }
            } else if matches!(
                ty,
                "Unit"
                    | "Int"
                    | "Bool"
                    | "Float"
                    | "CBool"
                    | "CInt8"
                    | "CInt16"
                    | "CInt32"
                    | "CUInt8"
                    | "CUInt16"
                    | "CUInt32"
                    | "CUInt64"
                    | "CFloat"
                    | "CDouble"
            ) {
                "scalar-v1"
            } else if ty == "String" {
                if input {
                    "string-borrow-v1"
                } else {
                    "string-copy-v1"
                }
            } else if ty.starts_with("Bytes(") && ty.ends_with(')') {
                if input {
                    "bytes-borrow-v1"
                } else {
                    "bytes-copy-v1"
                }
            } else if ty == "ByteArray" && input {
                "byte-array-borrow-v1"
            } else if ty == "NativeResult<ByteArray>" && !input && !callback {
                "byte-array-result-copy-v1"
            } else {
                return Err(format!("unsupported native conversion: {ty}"));
            };
            plan.push(format!("{position}:{operation}:{ty}"));
            Ok(())
        }
        let (parameters, result) = signature(&function.source_type)?;
        let mut plan = Vec::new();
        for (index, parameter) in parameters.iter().enumerate() {
            append(
                self,
                parameter,
                &format!("arg[{index}]"),
                true,
                false,
                &mut plan,
            )?;
        }
        append(self, result, "result", false, false, &mut plan)?;
        Ok(plan)
    }

    fn dependency_declaration(&self, name: &str) -> bool {
        self.package.build_inputs.iter().any(|(key, value)| {
            key.strip_prefix("dependency:")
                .is_some_and(|package| digest(value) && name.starts_with(&format!("{package}::")))
        })
    }

    /// Put metadata in canonical wire order before deriving a reusable identity.
    /// This does not replace interface validation.
    pub fn canonicalize(&mut self) {
        self.package.modules.sort();
        self.package.exports.sort();
        self.types.sort_by(|a, b| a.identity.cmp(&b.identity));
        self.functions.sort_by(|a, b| a.identity.cmp(&b.identity));
        for schema in self.source_types.values_mut() {
            schema.const_fields.sort_unstable();
            schema.private_fields.sort_unstable();
        }
        for schema in self.source_templates.values_mut() {
            schema.const_fields.sort_unstable();
            schema.private_fields.sort_unstable();
            for (_, markers) in schema
                .type_parameters
                .iter_mut()
                .chain(&mut schema.owner_type_parameters)
            {
                markers.sort();
                markers.dedup();
            }
        }
        for function in &mut self.functions {
            function.effects.sort();
            for site in &mut function.callback_sites {
                site.targets
                    .sort_by(|a, b| a.declaration.cmp(&b.declaration));
            }
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        let backend = &self.backend;
        if backend.native_data_contract != rinzu_abi::NATIVE_DATA_CONTRACT_VERSION {
            return Err("unsupported native data contract revision".into());
        }
        if backend.callback_rule != rinzu_abi::CALLBACK_RULE_VERSION
            || backend.template_layout != rinzu_abi::TEMPLATE_LAYOUT_VERSION
            || !(1..=rinzu_abi::CLOSED_SERVICE_REGISTRY_VERSION).contains(&backend.service_registry)
        {
            return Err("unsupported callback/template/service version".into());
        }
        if backend.owner_dispatcher != "macos-runloop-v1"
            || backend.bridge_abi != "arm64-x16-v1"
            || self.package.target != "aarch64-apple-darwin"
        {
            return Err("unsupported target/backend capability".into());
        }
        if backend.page_size != "16384"
            || backend.entry_stride != "32"
            || backend.control_stride != "32"
        {
            return Err("invalid template layout".into());
        }
        if self.package.package.is_empty()
            || self.package.data_layout.is_empty()
            || self.package.language.is_empty()
            || self.package.toolchain.is_empty()
            || self.package.native_compiler.is_empty()
        {
            return Err("incomplete package identity".into());
        }
        for names in [&self.package.modules, &self.package.exports] {
            if names.iter().collect::<BTreeSet<_>>().len() != names.len() {
                return Err("duplicate module/export identity".into());
            }
        }
        if self
            .provided_header_digest
            .as_ref()
            .is_some_and(|value| !digest(value))
        {
            return Err("invalid provided header digest".into());
        }
        if self.constants.values().any(|value| !decimal(value)) {
            return Err("constants must be canonical decimal strings".into());
        }
        if !self.constants.keys().eq(self.constant_types.keys())
            || self.constants.iter().any(|(name, value)| {
                let bounds = self
                    .constant_types
                    .get(name)
                    .and_then(|ty| integer_bounds(ty));
                !value.parse::<i128>().is_ok_and(|value| {
                    bounds.is_some_and(|(min, max)| min <= value && value <= max)
                })
            })
        {
            return Err(
                "constants require matching canonical integer types and exact in-range values"
                    .into(),
            );
        }
        for (identity, schema) in &self.source_types {
            let unique_fields = |fields: &[(String, String)]| {
                fields
                    .iter()
                    .all(|(name, ty)| identifier(name) && !ty.is_empty())
                    && fields
                        .iter()
                        .map(|(name, _)| name)
                        .collect::<BTreeSet<_>>()
                        .len()
                        == fields.len()
            };
            if identity.is_empty()
                || schema
                    .private_fields
                    .iter()
                    .any(|i| *i as usize >= schema.fields.len())
                || schema.private_fields.windows(2).any(|p| p[0] >= p[1])
                || !schema.private_fields.is_empty()
                    && !matches!(schema.kind.as_str(), "record" | "state")
                || schema
                    .const_fields
                    .iter()
                    .any(|index| *index as usize >= schema.fields.len())
                || schema
                    .const_fields
                    .windows(2)
                    .any(|pair| pair[0] >= pair[1])
                || !unique_fields(&schema.fields)
                || schema
                    .variants
                    .iter()
                    .any(|(name, fields)| !identifier(name) || !unique_fields(fields))
                || schema
                    .variants
                    .iter()
                    .map(|(name, _)| name)
                    .collect::<BTreeSet<_>>()
                    .len()
                    != schema.variants.len()
                || !match schema.kind.as_str() {
                    "error" => {
                        schema.variants.is_empty()
                            && (4..=5).contains(&schema.fields.len())
                            && schema.const_fields
                                == (0..schema.fields.len() as u32).collect::<Vec<_>>()
                            && schema.fields[..4]
                                .iter()
                                .map(|(name, _)| name.as_str())
                                .eq(["name", "message", "source", "cause"])
                            && schema.fields[0].1 == "String"
                            && schema.fields[1].1 == "String"
                            && schema.fields[2].1.starts_with("Option<")
                            && schema.fields[3].1 == "Option<Error>"
                            && schema
                                .fields
                                .get(4)
                                .is_none_or(|(name, _)| name == "payload")
                    }
                    "record" | "native-record" | "state" => schema.variants.is_empty(),
                    "enum" => schema.fields.is_empty(),
                    "opaque" => schema.fields.is_empty() && schema.variants.is_empty(),
                    _ => false,
                }
            {
                return Err("invalid canonical source type schema".into());
            }
        }
        for (identity, schema) in &self.source_templates {
            let fields_valid = |fields: &[(String, String)]| {
                fields
                    .iter()
                    .all(|(name, ty)| identifier(name) && !ty.is_empty())
                    && fields
                        .iter()
                        .map(|(name, _)| name)
                        .collect::<BTreeSet<_>>()
                        .len()
                        == fields.len()
            };
            if identity.is_empty()
                || schema.type_parameters.is_empty()
                    && schema.owner_type_parameters.is_empty()
                    && schema.kind != "interface"
                    && schema.interfaces.is_empty()
                    && schema.error_contract.is_none()
                || !schema.owner_type_parameters.is_empty() && schema.kind != "function"
                || schema
                    .type_parameters
                    .iter()
                    .chain(&schema.owner_type_parameters)
                    .any(|(name, markers)| {
                        !identifier(name)
                            || markers
                                .iter()
                                .any(|m| !matches!(m.as_str(), "Transferable" | "Equality" | "Key"))
                    })
                || schema
                    .type_parameters
                    .iter()
                    .chain(&schema.owner_type_parameters)
                    .map(|(name, _)| name)
                    .collect::<BTreeSet<_>>()
                    .len()
                    != schema.type_parameters.len() + schema.owner_type_parameters.len()
                || !fields_valid(&schema.fields)
                || !fields_valid(&schema.methods)
                || !schema.methods.is_empty() && schema.kind != "interface"
                || !schema.interfaces.is_empty()
                    && !matches!(schema.kind.as_str(), "class" | "enum")
                || schema.error_contract.is_some() && schema.kind != "function"
                || schema.interfaces.iter().any(String::is_empty)
                || schema
                    .error_contract
                    .as_ref()
                    .is_some_and(|types| types.iter().any(String::is_empty))
                || schema.interface_constraints.iter().any(|(p, bounds)| {
                    bounds.is_empty()
                        || bounds.iter().any(String::is_empty)
                        || !schema
                            .type_parameters
                            .iter()
                            .chain(&schema.owner_type_parameters)
                            .any(|(name, _)| name == p)
                })
                || schema
                    .private_fields
                    .iter()
                    .any(|i| *i as usize >= schema.fields.len())
                || schema.private_fields.windows(2).any(|p| p[0] >= p[1])
                || !schema.private_fields.is_empty() && schema.kind != "class"
                || schema
                    .variants
                    .iter()
                    .any(|(name, fields)| !identifier(name) || !fields_valid(fields))
                || schema
                    .variants
                    .iter()
                    .map(|(name, _)| name)
                    .collect::<BTreeSet<_>>()
                    .len()
                    != schema.variants.len()
                || schema
                    .const_fields
                    .iter()
                    .any(|i| *i as usize >= schema.fields.len())
                || schema.const_fields.windows(2).any(|p| p[0] >= p[1])
                || !match schema.kind.as_str() {
                    "function" => {
                        (schema.signature.starts_with("fn(")
                            || schema.signature.starts_with("async("))
                            && schema.fields.is_empty()
                            && schema.variants.is_empty()
                    }
                    "class" => schema.signature.is_empty() && schema.variants.is_empty(),
                    "enum" => schema.signature.is_empty() && schema.fields.is_empty(),
                    "interface" => {
                        schema.signature.is_empty()
                            && schema.fields.is_empty()
                            && schema.variants.is_empty()
                            && schema.const_fields.is_empty()
                    }
                    "error" => {
                        schema.variants.is_empty()
                            && (4..=5).contains(&schema.fields.len())
                            && schema.const_fields
                                == (0..schema.fields.len() as u32).collect::<Vec<_>>()
                            && schema.fields[..4]
                                .iter()
                                .map(|(name, _)| name.as_str())
                                .eq(["name", "message", "source", "cause"])
                            && schema.fields[0].1 == "String"
                            && schema.fields[1].1 == "String"
                            && schema.fields[2].1 == "Option<$rinzu_SourceLocation>"
                            && schema.fields[3].1 == "Option<Error>"
                            && match schema.fields.get(4) {
                                None => schema.signature == "error()",
                                Some((name, ty)) => {
                                    name == "payload"
                                        && (schema.signature == format!("error(payload:{ty})")
                                            || ty
                                                .strip_prefix("Option<")
                                                .and_then(|inner| inner.strip_suffix('>'))
                                                .is_some_and(|inner| {
                                                    schema.signature
                                                        == format!("error(payload?:{inner})")
                                                }))
                                }
                            }
                    }
                    _ => false,
                }
            {
                return Err("invalid public source template schema".into());
            }
        }
        let mut types = BTreeSet::new();
        if self
            .source_values
            .iter()
            .any(|(name, ty)| name.is_empty() || ty.is_empty())
        {
            return Err("public value requires a canonical type".into());
        }
        for (alias, identity) in &self.public_bindings {
            if alias.is_empty()
                || identity.is_empty()
                || !(self.source_types.contains_key(identity)
                    || self.source_templates.contains_key(identity)
                    || self.constants.contains_key(identity)
                    || self.source_values.contains_key(identity)
                    || self.types.iter().any(|t| &t.identity == identity)
                    || self.functions.iter().any(|f| &f.identity == identity)
                    || self.dependency_declaration(identity))
            {
                return Err("public binding requires a checked declaration identity".into());
            }
        }
        for ty in &self.types {
            if !types.insert(&ty.identity) {
                return Err("duplicate type identity".into());
            }
            let size = ty.size.parse::<u64>().map_err(|_| "invalid type size")?;
            let alignment = ty
                .alignment
                .parse::<u64>()
                .map_err(|_| "invalid type alignment")?;
            if !decimal(&ty.size)
                || !decimal(&ty.alignment)
                || !alignment.is_power_of_two()
                || size % alignment != 0
            {
                return Err("invalid type layout".into());
            }
            if ty
                .bytes_length
                .as_ref()
                .is_some_and(|n| !decimal(n) || n.starts_with('-'))
            {
                return Err("invalid Bytes length".into());
            }
            if !ty.fields.is_empty() && !identifier(&ty.c_type) {
                return Err("invalid record C identifier".into());
            }
            let mut fields = BTreeSet::new();
            let mut previous = None;
            for field in &ty.fields {
                let offset = field
                    .offset
                    .parse::<u64>()
                    .map_err(|_| "invalid field offset")?;
                if !identifier(&field.name)
                    || !matches!(
                        field.c_type.as_str(),
                        "int8_t"
                            | "int16_t"
                            | "int32_t"
                            | "int64_t"
                            | "uint8_t"
                            | "uint16_t"
                            | "uint32_t"
                            | "uint64_t"
                            | "float"
                            | "double"
                            | "_Bool"
                    )
                    || !decimal(&field.offset)
                    || offset >= size
                    || previous.is_some_and(|old| old >= offset)
                    || !fields.insert(&field.name)
                {
                    return Err("invalid field layout".into());
                }
                previous = Some(offset);
            }
        }
        let mut functions = BTreeSet::new();
        let mut symbols = BTreeSet::new();
        for function in &self.functions {
            if function.conversion_plan != self.native_conversion_plan(function)? {
                return Err(
                    "native positional conversion plan disagrees with checked signature".into(),
                );
            }
            if !functions.insert(&function.identity) {
                return Err("duplicate function identity".into());
            }
            if function.source_type.contains("function reference")
                || function
                    .parameter_requirements
                    .iter()
                    .any(|ty| ty.contains("function reference"))
            {
                return Err("function reference requires an exact public signature".into());
            }
            let managed = function.code.is_some();
            if managed {
                let code = function.code.as_ref().unwrap();
                if function.symbol.is_some()
                    || function.service.is_some()
                    || !function.c_prototype.is_empty()
                    || !matches!(function.kind, FunctionKind::Sync | FunctionKind::Async)
                    || code.declaration != function.identity
                    || code.version != 1
                {
                    return Err("invalid managed function code metadata".into());
                }
                if let Some(fallback) = &code.fallback {
                    if function.kind != FunctionKind::Sync {
                        return Err("async code cannot have a synchronous fallback".into());
                    }
                    fallback.validate()?;
                }
            } else if function.symbol.is_some() == function.service.is_some() {
                return Err("function requires exactly one symbol or service".into());
            }
            if let Some(symbol) = &function.symbol {
                if !identifier(symbol) || !symbols.insert(symbol) {
                    return Err("invalid/duplicate symbol".into());
                }
                if !function.c_prototype.contains(&format!("{symbol}("))
                    || function
                        .c_prototype
                        .contains([';', '#', '\n', '\r', '{', '}'])
                {
                    return Err("invalid C prototype".into());
                }
            }
            if let Some(service) = &function.service {
                let expected = match service.as_str() {
                    "reactor.file-read-size" => "async(String)->Int",
                    "reactor.timer" => "async(Int)->Unit",
                    "reactor.native-completion" | "reactor.native-transfer" => {
                        let transfer = service == "reactor.native-transfer";
                        let minimum_registry = if transfer { 3 } else { 2 };
                        if self.backend.service_registry < minimum_registry {
                            return Err(format!(
                                "native receipt requires closed service registry v{minimum_registry}"
                            ));
                        }
                        let identity =
                            function
                                .source_type
                                .strip_prefix("async(")
                                .and_then(|name| {
                                    name.strip_suffix(if transfer { ")->Int" } else { ")->Unit" })
                                });
                        if !identity.is_some_and(|identity| {
                            self.source_types
                                .get(identity)
                                .is_some_and(|schema| schema.kind == "opaque")
                        }) {
                            return Err(format!(
                                "native completion requires an opaque protocol handle: {}",
                                function.source_type
                            ));
                        }
                        function.source_type.as_str()
                    }
                    _ => return Err("unsupported closed service".into()),
                };
                if function.kind != FunctionKind::Async || function.source_type != expected {
                    return Err("unsupported closed service".into());
                }
            }
            if function.effects.iter().collect::<BTreeSet<_>>().len() != function.effects.len() {
                return Err("duplicate effect".into());
            }
            for callback in &function.callbacks {
                if callback.code_version != 1 || callback.target != self.package.target {
                    return Err("unsupported callback code/target".into());
                }
                if callback.lifetime != "current-native-call"
                    || callback.gate != "admission-close-settle-v1"
                    || callback.origin != "actual-caller-domain"
                    || callback.parent_scope != "actual-caller-scope"
                    || callback.foreign_to_main
                    || callback.fallback.is_some()
                {
                    return Err("unsupported persistent callback contract; use a current-native-call wrapper".into());
                }
            }
            if !managed && !function.callback_sites.is_empty() {
                return Err("native declaration cannot contain managed callback sites".into());
            }
            for site in &function.callback_sites {
                if site.native_function.is_empty() || !site.source_type.starts_with("fn(") {
                    return Err("invalid native callback site".into());
                }
                if let Some(capture) = &site.capture {
                    let valid = self.source_types.get(capture).is_some_and(|schema| {
                        schema.kind == "state"
                            && schema.fields.len() == 1
                            && schema.fields[0].0 == "environment"
                            && self
                                .source_types
                                .get(&schema.fields[0].1)
                                .is_some_and(|environment| environment.kind == "enum")
                    });
                    if !valid {
                        return Err(
                            "callback capture requires its checked closure and environment schemas"
                                .into(),
                        );
                    }
                }
                if let Some(native) = self
                    .functions
                    .iter()
                    .find(|callee| callee.identity == site.native_function)
                {
                    if !matches!(native.kind, FunctionKind::Native | FunctionKind::Supplant)
                        || native.parameter_requirements.get(site.parameter)
                            != Some(&site.source_type)
                    {
                        return Err("callback site disagrees with native parameter".into());
                    }
                } else if !self.dependency_declaration(&site.native_function) {
                    return Err(
                        "callback site has no native declaration or validated dependency".into(),
                    );
                }
                let mut targets = BTreeSet::new();
                for target in &site.targets {
                    if target.declaration.is_empty()
                        || target.version != 1
                        || !targets.insert(&target.declaration)
                    {
                        return Err("invalid/duplicate callback target".into());
                    }
                    if let Some(fallback) = &target.fallback {
                        fallback.validate()?;
                    }
                    if let Some(length) = target.declaration.strip_prefix("rinzu.core::byteLength:")
                    {
                        if site.capture.is_some()
                            || !length.parse::<u64>().is_ok_and(|n| n.to_string() == length)
                            || site.source_type != format!("fn(Bytes({length}))->CUInt64")
                            || target.fallback.is_some()
                        {
                            return Err(
                                "callback length target disagrees with canonical Bytes signature"
                                    .into(),
                            );
                        }
                    } else if target.declaration == "rinzu.core::utf8ByteLength" {
                        if site.capture.is_some()
                            || site.source_type != "fn(String)->CUInt64"
                            || target.fallback.is_some()
                        {
                            return Err(
                                "callback UTF-8 length target disagrees with signature".into()
                            );
                        }
                    } else if target.declaration.starts_with("rinzu.core::") {
                        return Err("unsupported core callback target".into());
                    } else if let Some(callee) = self
                        .functions
                        .iter()
                        .find(|callee| callee.identity == target.declaration)
                    {
                        let expected = match callee.kind {
                            FunctionKind::Sync => {
                                let logical = self.managed_callback_type(&site.source_type)?;
                                if let Some(capture) = &site.capture {
                                    if callee.parameter_requirements.first() != Some(capture) {
                                        return Err(
                                            "callback target lacks its hidden capture parameter"
                                                .into(),
                                        );
                                    }
                                    let suffix = logical.strip_prefix("fn(").unwrap();
                                    format!(
                                        "fn({capture}{}{suffix}",
                                        if suffix.starts_with(')') { "" } else { "," }
                                    )
                                } else {
                                    logical
                                }
                            }
                            FunctionKind::Native if site.capture.is_none() => {
                                site.source_type.replacen("fn(", "native(", 1)
                            }
                            _ => return Err("callback target must be synchronous".into()),
                        };
                        if callee.source_type != expected
                            || callee.code.as_ref().is_some_and(|code| code != target)
                            || (callee.kind == FunctionKind::Native && target.fallback.is_some())
                        {
                            return Err(
                                "callback target disagrees with code signature/version/fallback"
                                    .into(),
                            );
                        }
                    } else if site.capture.is_some()
                        || !self.dependency_declaration(&target.declaration)
                    {
                        return Err(
                            "callback target has no code declaration or validated dependency"
                                .into(),
                        );
                    }
                }
            }
        }
        Ok(())
    }

    /// Userdata is an explicit physical pointer mapping of an ordinary nominal
    /// parameter. Its managed function type keeps that record/enum identity.
    fn managed_callback_type(&self, physical: &str) -> Result<String, String> {
        let (parameters, result) = physical
            .strip_prefix("fn(")
            .and_then(|ty| ty.rsplit_once(")->"))
            .ok_or("invalid callback signature")?;
        let mut logical = Vec::new();
        if !parameters.is_empty() {
            for parameter in parameters.split(',') {
                let ty = if let Some(identity) = parameter
                    .strip_prefix("UserData<")
                    .and_then(|ty| ty.strip_suffix('>'))
                {
                    let mapped = self.types.iter().any(|ty| {
                        ty.identity == parameter
                            && ty.canonical == parameter
                            && ty.c_type == "void *"
                            && ty.size == "8"
                            && ty.alignment == "8"
                            && ty.fields.is_empty()
                            && ty.bytes_length.is_none()
                    });
                    if !mapped
                        || !self.source_types.get(identity).is_some_and(|schema| {
                            matches!(schema.kind.as_str(), "record" | "enum" | "state")
                        })
                    {
                        return Err("callback userdata has no exact nominal mapping".into());
                    }
                    identity
                } else {
                    parameter
                };
                logical.push(ty);
            }
        }
        Ok(format!("fn({})->{result}", logical.join(",")))
    }
}

fn header(interface: &SourceInterface) -> String {
    let mut result = String::from(
        "/* Generated source-package v2 contract. */\n#include <stdint.h>\n#include <stddef.h>\n",
    );
    for ty in &interface.types {
        if !ty.fields.is_empty() {
            result.push_str(&format!("typedef struct {} {{\n", ty.c_type));
            for field in &ty.fields {
                result.push_str(&format!("  {} {};\n", field.c_type, field.name));
            }
            result.push_str(&format!("}} {};\n", ty.c_type));
            result.push_str(&format!("_Static_assert(sizeof({0}) == {1} && _Alignof({0}) == {2}, \"source native record layout\");\n", ty.c_type, ty.size, ty.alignment));
            for field in &ty.fields {
                result.push_str(&format!(
                    "_Static_assert(offsetof({}, {}) == {}, \"source native field offset\");\n",
                    ty.c_type, field.name, field.offset
                ));
            }
        }
    }
    for function in &interface.functions {
        if function.symbol.is_some() {
            result.push_str(&format!("{};\n", function.c_prototype));
        }
    }
    result
}

fn keys(interface: &SourceInterface) -> Result<Vec<String>, String> {
    interface
        .functions
        .iter()
        .map(|function| {
            // Full source and external ABI, referenced layouts, callbacks and backend versions.
            json(&(
                function,
                &interface.types,
                &interface.backend,
                &interface.package.target,
                &interface.package.data_layout,
                &interface.provided_header_digest,
                &interface.constants,
                &interface.constant_types,
                &interface.source_types,
                &interface.source_templates,
                &interface.source_values,
            ))
            .map(|bytes| sha256(&bytes))
        })
        .collect()
}

pub fn generate_source_package(
    mut interface: SourceInterface,
    native_artifact: &[u8],
) -> Result<GeneratedSourcePackage, String> {
    let plans = interface
        .functions
        .iter()
        .map(|function| interface.native_conversion_plan(function))
        .collect::<Result<Vec<_>, _>>()?;
    for (function, plan) in interface.functions.iter_mut().zip(plans) {
        function.conversion_plan = plan;
    }
    interface.canonicalize();
    interface.validate()?;
    let header = header(&interface);
    let descriptor = SourceDescriptor {
        schema: "rinzu.source-package".into(),
        major: rinzu_abi::SOURCE_PACKAGE_MAJOR,
        minor: rinzu_abi::SOURCE_PACKAGE_MINOR,
        import_keys: keys(&interface)?,
        semantic_digest: sha256(&json(&interface)?),
        native_artifact_digest: sha256(native_artifact),
        generated_header_digest: sha256(header.as_bytes()),
        glue_identity: "rinzu-source-table-v2.1".into(),
        interface,
    };
    let wire = json(&descriptor)?;
    let mut c_table = String::from(
        "/* Generated: do not edit. */\n#include \"rinzu/runtime.h\"\n#include \"source-package.h\"\n",
    );
    c_table.push_str(&format!(
        "static const uint8_t descriptor_bytes[] = {{{}}};\n",
        c_bytes(&wire)
    ));
    let native = descriptor
        .interface
        .functions
        .iter()
        .zip(&descriptor.import_keys)
        .filter_map(|(function, key)| function.symbol.as_ref().map(|symbol| (symbol, key)))
        .collect::<Vec<_>>();
    if !native.is_empty() {
        c_table.push_str("static const rinzu_source_import_v2_t imports[] = {\n");
        for (symbol, key) in &native {
            let key_bytes = decode_key(key)?;
            c_table.push_str(&format!(
                "  {{{{{}}}, \"{symbol}\", (const void *)&{symbol}}},\n",
                c_bytes(&key_bytes)
            ));
        }
        c_table.push_str("};\n");
    }
    c_table.push_str(&format!("static const rinzu_source_package_descriptor_v2_t package = {{sizeof(package), RINZU_SOURCE_PACKAGE_MAJOR, RINZU_SOURCE_PACKAGE_MINOR, descriptor_bytes, sizeof(descriptor_bytes), {}, {}}};\n", native.len(), if native.is_empty() { "NULL" } else { "imports" }));
    c_table.push_str("RINZU_API const rinzu_source_package_descriptor_v2_t *rinzu_source_package_v2(void) { return &package; }\n");
    Ok(GeneratedSourcePackage {
        descriptor,
        wire,
        header,
        c_table,
    })
}

fn c_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| byte.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

pub fn decode_key(key: &str) -> Result<[u8; 32], String> {
    if !digest(key) {
        return Err("invalid SHA-256 key".into());
    }
    let mut result = [0; 32];
    for (index, byte) in result.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&key[index * 2..index * 2 + 2], 16)
            .map_err(|error| error.to_string())?;
    }
    Ok(result)
}

/// Reuse a successful validation only for an identical wire digest. Callers get
/// independent values; file freshness and generated artifact seals remain the
/// caller's responsibility. Failed validation never populates this cache.
pub fn validate_source_descriptor(wire: &[u8]) -> Result<SourceDescriptor, String> {
    use std::sync::{Arc, Mutex};
    type Entry = (usize, Arc<SourceDescriptor>);
    static CACHE: Mutex<std::collections::BTreeMap<String, Entry>> =
        Mutex::new(std::collections::BTreeMap::new());
    const LIMIT: usize = 8 * 1024 * 1024;
    let key = (wire.len() <= LIMIT / 2).then(|| sha256(wire));
    if let Some(key) = &key
        && let Some(descriptor) = CACHE
            .lock()
            .ok()
            .and_then(|cache| cache.get(key).map(|(_, value)| value.clone()))
    {
        return Ok((*descriptor).clone());
    }
    let descriptor = validate_source_descriptor_uncached(wire)?;
    if let Some(key) = key
        && let Ok(mut cache) = CACHE.lock()
        && !cache.contains_key(&key)
    {
        // Bound retained wire-size weight and entry count for editor processes.
        let weight = wire.len() + key.len();
        let mut retained = cache.values().map(|(weight, _)| weight).sum::<usize>();
        while retained + weight > LIMIT || cache.len() >= 32 {
            let Some((_, (removed, _))) = cache.pop_first() else {
                break;
            };
            retained -= removed;
        }
        cache.insert(key, (weight, Arc::new(descriptor.clone())));
    }
    Ok(descriptor)
}

fn validate_source_descriptor_uncached(wire: &[u8]) -> Result<SourceDescriptor, String> {
    let descriptor: SourceDescriptor = serde_json::from_slice(wire).map_err(|error| {
        format!("invalid source-package descriptor; regenerate with the current toolchain: {error}")
    })?;
    if descriptor.schema != "rinzu.source-package"
        || descriptor.major != rinzu_abi::SOURCE_PACKAGE_MAJOR
        || descriptor.minor != rinzu_abi::SOURCE_PACKAGE_MINOR
    {
        return Err(
            "unsupported source-package schema/version; regenerate with the current toolchain"
                .into(),
        );
    }
    descriptor.interface.validate()?;
    crate::consumption::validate(&descriptor.interface)?;
    if let Some(plan) = crate::native_binding::NativeBindingPlan::decode(
        &descriptor.interface.package.build_inputs,
    )? {
        plan.validate_functions(&descriptor.interface.functions)?;
    }
    crate::compilation::validate(&descriptor.interface.package.build_inputs)?;
    let mut canonical = descriptor.interface.clone();
    canonical.canonicalize();
    if canonical != descriptor.interface || json(&descriptor)? != wire {
        return Err("noncanonical source descriptor".into());
    }
    if descriptor.import_keys != keys(&descriptor.interface)?
        || descriptor.semantic_digest != sha256(&json(&descriptor.interface)?)
        || descriptor.generated_header_digest != sha256(header(&descriptor.interface).as_bytes())
        || !digest(&descriptor.native_artifact_digest)
        || descriptor.glue_identity != "rinzu-source-table-v2.1"
    {
        return Err("source descriptor key/digest/header/glue mismatch".into());
    }
    Ok(descriptor)
}
