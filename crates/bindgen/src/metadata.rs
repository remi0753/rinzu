use std::fmt;

use serde::{Deserialize, Serialize};

pub const BINDING_METADATA_SCHEMA: &str = "rinzu.binding-metadata";
pub const BINDING_METADATA_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BindingMetadata {
    pub schema: String,
    pub version: u32,
    pub library: String,
    pub header: String,
    pub functions: Vec<FunctionMetadata>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FunctionMetadata {
    pub name: String,
    pub symbol: String,
    pub parameters: Vec<ParameterMetadata>,
    pub result_c_type: String,
    pub thread: ThreadRequirement,
    pub blocking: BlockingBehavior,
    pub synchronous_reentry: SynchronousReentry,
    pub exception_boundary: bool,
    pub callbacks: Vec<CallbackMetadata>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ParameterMetadata {
    pub name: String,
    pub c_type: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CallbackMetadata {
    pub parameter: String,
    pub context_parameter: String,
    pub result_c_type: String,
    pub class: CallbackClass,
    pub arguments: Vec<CallbackArgumentMetadata>,
    pub delivery: DeliveryPolicy,
    pub nested_delivery: NestedDelivery,
    pub unregister: UnregisterPolicy,
    pub shutdown: ShutdownPolicy,
    pub failure_return: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CallbackArgumentMetadata {
    pub name: String,
    pub c_type: String,
    pub lifetime: ArgumentLifetime,
    pub retain_release_thread_safe: Option<bool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThreadRequirement {
    ProcessMain,
    Any,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BlockingBehavior {
    NonBlocking,
    OffloadBlocking,
    OwnerLoopEntry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SynchronousReentry {
    Never,
    MayCallback,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CallbackClass {
    OwnerSync,
    OwnerAsync,
    ForeignOneWay,
    ForeignSync,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArgumentLifetime {
    ValueCopy,
    Retained,
    OwnedTransfer,
    CallOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeliveryPolicy {
    Inline,
    OneShot,
    SerialBounded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NestedDelivery {
    Guaranteed,
    Deferred,
    Forbidden,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "kebab-case")]
pub enum UnregisterPolicy {
    NotRequired,
    Function { symbol: String, asynchronous: bool },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "kebab-case")]
pub enum ShutdownPolicy {
    Unregister,
    NativeOnlyRelease { symbol: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedMetadata(BindingMetadata);

impl ValidatedMetadata {
    #[must_use]
    pub fn get(&self) -> &BindingMetadata {
        &self.0
    }

    #[must_use]
    pub fn into_inner(self) -> BindingMetadata {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BindingMetadataError {
    pub code: &'static str,
    pub path: String,
    pub message: String,
}

impl BindingMetadataError {
    fn new(code: &'static str, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code,
            path: path.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for BindingMetadataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} at {}: {}",
            self.code, self.path, self.message
        )
    }
}

impl std::error::Error for BindingMetadataError {}

/// Parses and validates metadata. Any parse or semantic failure means the
/// declaration is RawOnly; callers must not generate a safe wrapper.
pub fn validate_metadata(source: &str) -> Result<ValidatedMetadata, Vec<BindingMetadataError>> {
    let metadata: BindingMetadata = serde_json::from_str(source).map_err(|error| {
        vec![BindingMetadataError::new(
            "RZB0001",
            "$",
            format!("metadata is incomplete or contains an unknown value: {error}"),
        )]
    })?;
    let errors = validate(&metadata);
    if errors.is_empty() {
        Ok(ValidatedMetadata(metadata))
    } else {
        Err(errors)
    }
}

fn validate(metadata: &BindingMetadata) -> Vec<BindingMetadataError> {
    let mut errors = Vec::new();
    if metadata.schema != BINDING_METADATA_SCHEMA {
        errors.push(BindingMetadataError::new(
            "RZB0002",
            "$.schema",
            format!("expected `{BINDING_METADATA_SCHEMA}`"),
        ));
    }
    if metadata.version != BINDING_METADATA_VERSION {
        errors.push(BindingMetadataError::new(
            "RZB0003",
            "$.version",
            format!("unsupported binding metadata version {}", metadata.version),
        ));
    }
    require_identifier(&metadata.library, "$.library", &mut errors);
    require_nonempty(&metadata.header, "$.header", &mut errors);
    if metadata.functions.is_empty() {
        errors.push(BindingMetadataError::new(
            "RZB0004",
            "$.functions",
            "at least one C function declaration is required",
        ));
    }
    let mut function_names = std::collections::HashSet::new();
    let mut symbols = std::collections::HashSet::new();
    for (function_index, function) in metadata.functions.iter().enumerate() {
        let path = format!("$.functions[{function_index}]");
        require_identifier(&function.name, &format!("{path}.name"), &mut errors);
        require_identifier(&function.symbol, &format!("{path}.symbol"), &mut errors);
        require_nonempty(
            &function.result_c_type,
            &format!("{path}.resultCType"),
            &mut errors,
        );
        if !function_names.insert(&function.name) {
            errors.push(BindingMetadataError::new(
                "RZB0005",
                format!("{path}.name"),
                "function name must be unique",
            ));
        }
        if !symbols.insert(&function.symbol) {
            errors.push(BindingMetadataError::new(
                "RZB0006",
                format!("{path}.symbol"),
                "C symbol must be unique",
            ));
        }
        let mut parameters = std::collections::HashSet::new();
        for (parameter_index, parameter) in function.parameters.iter().enumerate() {
            let parameter_path = format!("{path}.parameters[{parameter_index}]");
            require_identifier(
                &parameter.name,
                &format!("{parameter_path}.name"),
                &mut errors,
            );
            require_nonempty(
                &parameter.c_type,
                &format!("{parameter_path}.cType"),
                &mut errors,
            );
            if !parameters.insert(parameter.name.as_str()) {
                errors.push(BindingMetadataError::new(
                    "RZB0007",
                    format!("{parameter_path}.name"),
                    "parameter name must be unique within a function",
                ));
            }
        }
        if !function.callbacks.is_empty() && !function.exception_boundary {
            errors.push(BindingMetadataError::new(
                "RZB0008",
                format!("{path}.exceptionBoundary"),
                "a safe callback binding must contain native/managed exceptions",
            ));
        }
        let mut callback_parameters = std::collections::HashSet::new();
        for (callback_index, callback) in function.callbacks.iter().enumerate() {
            let callback_path = format!("{path}.callbacks[{callback_index}]");
            if !parameters.contains(callback.parameter.as_str()) {
                errors.push(BindingMetadataError::new(
                    "RZB0009",
                    format!("{callback_path}.parameter"),
                    "callback must name a declared C parameter",
                ));
            }
            if !parameters.contains(callback.context_parameter.as_str()) {
                errors.push(BindingMetadataError::new(
                    "RZB0025",
                    format!("{callback_path}.contextParameter"),
                    "callback context must name a declared C parameter",
                ));
            }
            if callback.context_parameter == callback.parameter {
                errors.push(BindingMetadataError::new(
                    "RZB0026",
                    format!("{callback_path}.contextParameter"),
                    "callback and context must use different parameters",
                ));
            }
            if !callback_parameters.insert(callback.parameter.as_str()) {
                errors.push(BindingMetadataError::new(
                    "RZB0010",
                    format!("{callback_path}.parameter"),
                    "a callback parameter may have only one callback contract",
                ));
            }
            validate_callback(function, callback, &callback_path, &mut errors);
        }
    }
    errors
}

fn validate_callback(
    function: &FunctionMetadata,
    callback: &CallbackMetadata,
    path: &str,
    errors: &mut Vec<BindingMetadataError>,
) {
    require_nonempty(
        &callback.result_c_type,
        &format!("{path}.resultCType"),
        errors,
    );
    if callback.class == CallbackClass::ForeignSync {
        errors.push(BindingMetadataError::new(
            "RZB0011",
            format!("{path}.class"),
            "ForeignSync is unsupported by v1 safe bindings",
        ));
    }
    let expected_delivery = match callback.class {
        CallbackClass::OwnerSync => DeliveryPolicy::Inline,
        CallbackClass::OwnerAsync => DeliveryPolicy::SerialBounded,
        CallbackClass::ForeignOneWay => DeliveryPolicy::OneShot,
        CallbackClass::ForeignSync => return,
    };
    if callback.delivery != expected_delivery {
        errors.push(BindingMetadataError::new(
            "RZB0012",
            format!("{path}.delivery"),
            format!(
                "{:?} requires {:?} delivery",
                callback.class, expected_delivery
            ),
        ));
    }
    if callback.class == CallbackClass::OwnerSync && callback.failure_return.is_none() {
        errors.push(BindingMetadataError::new(
            "RZB0013",
            format!("{path}.failureReturn"),
            "OwnerSync requires an explicit failure return mapping",
        ));
    }
    if callback.class != CallbackClass::OwnerSync && callback.failure_return.is_some() {
        errors.push(BindingMetadataError::new(
            "RZB0014",
            format!("{path}.failureReturn"),
            "only OwnerSync returns a managed failure synchronously",
        ));
    }
    if callback.class == CallbackClass::OwnerSync
        && function.thread != ThreadRequirement::ProcessMain
    {
        errors.push(BindingMetadataError::new(
            "RZB0015",
            format!("{path}.class"),
            "OwnerSync requires the process-main owner thread",
        ));
    }
    if callback.class == CallbackClass::OwnerSync
        && function.synchronous_reentry != SynchronousReentry::MayCallback
    {
        errors.push(BindingMetadataError::new(
            "RZB0016",
            format!("{path}.class"),
            "OwnerSync requires synchronousReentry `may-callback`",
        ));
    }
    if callback.delivery == DeliveryPolicy::SerialBounded
        && matches!(callback.unregister, UnregisterPolicy::NotRequired)
    {
        errors.push(BindingMetadataError::new(
            "RZB0017",
            format!("{path}.unregister"),
            "bounded repeated delivery requires a native unregister hook",
        ));
    }
    if matches!(callback.shutdown, ShutdownPolicy::Unregister)
        && matches!(callback.unregister, UnregisterPolicy::NotRequired)
    {
        errors.push(BindingMetadataError::new(
            "RZB0018",
            format!("{path}.shutdown"),
            "shutdown `unregister` requires an unregister function",
        ));
    }
    let mut names = std::collections::HashSet::new();
    for (argument_index, argument) in callback.arguments.iter().enumerate() {
        let argument_path = format!("{path}.arguments[{argument_index}]");
        require_identifier(&argument.name, &format!("{argument_path}.name"), errors);
        require_nonempty(&argument.c_type, &format!("{argument_path}.cType"), errors);
        if !names.insert(argument.name.as_str()) {
            errors.push(BindingMetadataError::new(
                "RZB0019",
                format!("{argument_path}.name"),
                "callback argument name must be unique",
            ));
        }
        if argument.lifetime == ArgumentLifetime::CallOnly
            && callback.class != CallbackClass::OwnerSync
        {
            errors.push(BindingMetadataError::new(
                "RZB0020",
                format!("{argument_path}.lifetime"),
                "CallOnly is valid only for OwnerSync callbacks",
            ));
        }
        if callback.class == CallbackClass::ForeignOneWay
            && argument.lifetime == ArgumentLifetime::Retained
            && argument.retain_release_thread_safe != Some(true)
        {
            errors.push(BindingMetadataError::new(
                "RZB0021",
                format!("{argument_path}.retainReleaseThreadSafe"),
                "ForeignOneWay retained arguments require an explicit thread-safety guarantee",
            ));
        }
        if argument.lifetime != ArgumentLifetime::Retained
            && argument.retain_release_thread_safe.is_some()
        {
            errors.push(BindingMetadataError::new(
                "RZB0022",
                format!("{argument_path}.retainReleaseThreadSafe"),
                "retain/release thread safety applies only to Retained arguments",
            ));
        }
    }
}

fn require_nonempty(value: &str, path: &str, errors: &mut Vec<BindingMetadataError>) {
    if value.trim().is_empty() {
        errors.push(BindingMetadataError::new(
            "RZB0023",
            path,
            "value must not be empty",
        ));
    }
}

fn require_identifier(value: &str, path: &str, errors: &mut Vec<BindingMetadataError>) {
    let mut characters = value.chars();
    let valid = characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric());
    if !valid {
        errors.push(BindingMetadataError::new(
            "RZB0024",
            path,
            "expected a non-empty C-style identifier",
        ));
    }
}
