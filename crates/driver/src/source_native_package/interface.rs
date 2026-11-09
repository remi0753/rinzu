//! Source SDK descriptor generation from the checked interface and native contract.
use super::source_language_flags;
use crate::native_contract::NativeContract;
use rinzu_bindgen::source_package::{
    BackendRecord, CallbackRecord, CallbackSiteRecord, CodeRecord, FallbackRecord, FieldLayout,
    FunctionKind, FunctionRecord, PackageIdentity, SourceInterface, SourceTemplateRecord,
    SourceTypeRecord, TypeRecord, sha256,
};
use rinzu_compiler::ast::NativeBinding;
use rinzu_compiler::{NativeCliInputs, ResolvedSdkPackage, SourceAbiType, SourceSdkInterface};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

fn abi_type(ty: &SourceAbiType) -> String {
    ty.canonical_name()
}

fn source_type(ty: &rinzu_compiler::Type) -> String {
    use rinzu_compiler::Type;
    if let Some(scalar) = ty.scalar() {
        return scalar.name().to_owned();
    }
    match ty {
        Type::FunctionReference(reference) if reference.error_bound.is_some() => ty.to_string(),
        Type::FunctionReference(reference) => format!(
            "{}({})->{}",
            if reference.supplant { "supplant" } else { "fn" },
            reference
                .parameters
                .iter()
                .map(source_type)
                .collect::<Vec<_>>()
                .join(","),
            source_type(&reference.result)
        ),
        Type::Future(result) => format!("Future<{}>", source_type(result)),
        Type::List(element) => format!("List<{}>", source_type(element)),
        Type::Map(key, value) => format!("Map<{},{}>", source_type(key), source_type(value)),
        _ => ty.to_string(),
    }
}

fn native_callback_sites(
    source: &SourceSdkInterface,
    function: &str,
) -> Result<Vec<CallbackSiteRecord>, String> {
    let mut sites = Vec::new();
    let mut append = |signature: &rinzu_compiler::SourceNativeSignature,
                      callbacks: &[rinzu_compiler::CallbackArgumentPlan]|
     -> Result<(), String> {
        for callback in callbacks {
            let parameter = signature
                .parameters
                .get(callback.index)
                .filter(|ty| matches!(ty, SourceAbiType::Callback { .. }))
                .ok_or_else(|| {
                    format!(
                        "callback site has no checked parameter: {}:{}",
                        signature.name, callback.index
                    )
                })?;
            sites.push(CallbackSiteRecord {
                native_function: signature.name.clone(),
                parameter: callback.index,
                source_type: abi_type(parameter),
                capture: callback.capture.as_ref().map(ToString::to_string),
                targets: callback
                    .targets
                    .iter()
                    .map(|target| CodeRecord {
                        declaration: target.clone(),
                        version: 1,
                        fallback: source
                            .lowered_functions
                            .iter()
                            .find(|body| body.name == *target)
                            .and_then(|body| body.callback_fallback.as_ref())
                            .map(fallback_record),
                    })
                    .collect(),
            });
        }
        Ok(())
    };
    for operation in source
        .lowered_functions
        .iter()
        .filter(|body| body.name == function)
        .flat_map(|body| &body.blocks)
        .flat_map(|block| &block.operations)
    {
        match &operation.kind {
            rinzu_compiler::OperationKind::PrepareNativeCall {
                signature,
                callbacks,
                ..
            } => append(signature, callbacks)?,
            rinzu_compiler::OperationKind::ReserveSupplant {
                targets, callbacks, ..
            } if !callbacks.is_empty() => {
                for target in targets {
                    let signature = source
                        .lowered_native_signatures
                        .iter()
                        .find(|signature| signature.name == *target && signature.is_supplant)
                        .ok_or_else(|| {
                            format!(
                                "supplant callback site has no checked native declaration: {target}"
                            )
                        })?;
                    append(signature, callbacks)?;
                }
            }
            _ => {}
        }
    }
    Ok(sites)
}

pub(super) fn build(
    package: &ResolvedSdkPackage,
    source: &SourceSdkInterface,
    contract: &NativeContract,
    application_root: &Path,
    inputs: &NativeCliInputs,
) -> Result<SourceInterface, String> {
    let _timing = rinzu_compiler::timing::span("native.interface").detail(&package.manifest.name);
    let mut types = BTreeMap::new();
    fn collect_type(
        ty: &SourceAbiType,
        contract: &NativeContract,
        schemas: &BTreeMap<String, Vec<(String, rinzu_compiler::AbiScalar)>>,
        types: &mut BTreeMap<String, TypeRecord>,
    ) {
        let identity = abi_type(ty);
        if types.contains_key(&identity) {
            return;
        }
        let (c_type, size, alignment, bytes_length, fields) = match ty {
            SourceAbiType::Unit => ("void".to_owned(), 0, 1, None, Vec::new()),
            SourceAbiType::Scalar(scalar) => (
                scalar.c_type().to_owned(),
                scalar.size(),
                scalar.size(),
                None,
                Vec::new(),
            ),
            SourceAbiType::Record(name) => {
                let layout = &contract.records[name];
                let fields = schemas[name]
                    .iter()
                    .zip(&layout.offsets)
                    .map(|((name, scalar), offset)| FieldLayout {
                        name: name.clone(),
                        c_type: scalar.c_type().to_owned(),
                        offset: offset.to_string(),
                    })
                    .collect();
                (
                    layout.c_type.clone(),
                    layout.size,
                    layout.alignment,
                    None,
                    fields,
                )
            }
            SourceAbiType::String => ("const char *".to_owned(), 8, 8, None, Vec::new()),
            SourceAbiType::Bytes(length) => (
                "const uint8_t *".to_owned(),
                8,
                8,
                Some(length.to_string()),
                Vec::new(),
            ),
            SourceAbiType::ByteArray => ("const uint8_t *".to_owned(), 8, 8, None, Vec::new()),
            SourceAbiType::ByteArrayResult => (
                "const struct __rinzu_byte_array_result *".to_owned(),
                8,
                8,
                None,
                Vec::new(),
            ),
            SourceAbiType::Callback { parameters, result } => {
                for ty in parameters {
                    collect_type(ty, contract, schemas, types);
                }
                collect_type(result, contract, schemas, types);
                ("void *".to_owned(), 8, 8, None, Vec::new())
            }
            SourceAbiType::Opaque(_) | SourceAbiType::UserData(_) => {
                ("void *".to_owned(), 8, 8, None, Vec::new())
            }
        };
        types.insert(
            identity.clone(),
            TypeRecord {
                identity: identity.clone(),
                canonical: identity,
                c_type,
                size: size.to_string(),
                alignment: alignment.to_string(),
                bytes_length,
                fields,
            },
        );
    }
    for (name, schema) in &source.types {
        let ty = match schema.kind.as_str() {
            "native-record" => SourceAbiType::Record(name.clone()),
            "opaque" => SourceAbiType::Opaque(name.clone()),
            _ => continue,
        };
        collect_type(&ty, contract, &source.native_records, &mut types);
    }
    let mut functions = Vec::new();
    for signature in &source.signatures {
        for ty in signature
            .parameters
            .iter()
            .chain(std::iter::once(&signature.result))
        {
            collect_type(ty, contract, &source.native_records, &mut types);
        }
        let source_type = signature.canonical_type();
        let (symbol, service) = match &signature.binding {
            NativeBinding::Symbol(symbol) => (Some(symbol.clone()), None),
            NativeBinding::Runtime(function) => (None, Some(function.service_id().to_owned())),
        };
        let callbacks = signature
            .parameters
            .iter()
            .filter_map(|ty| {
                if matches!(ty, SourceAbiType::Callback { .. }) {
                    Some(CallbackRecord {
                        source_type: abi_type(ty),
                        foreign_to_main: false,
                        lifetime: "current-native-call".to_owned(),
                        gate: "admission-close-settle-v1".to_owned(),
                        origin: "actual-caller-domain".to_owned(),
                        parent_scope: "actual-caller-scope".to_owned(),
                        target: "aarch64-apple-darwin".to_owned(),
                        code_version: 1,
                        fallback: None,
                    })
                } else {
                    None
                }
            })
            .collect();
        functions.push(FunctionRecord {
            identity: signature.name.clone(),
            error_contract: None,
            source_type,
            kind: if signature.is_async {
                FunctionKind::Async
            } else if signature.is_supplant {
                FunctionKind::Supplant
            } else {
                FunctionKind::Native
            },
            effects: if signature.is_supplant {
                vec!["requires-owner".to_owned()]
            } else {
                Vec::new()
            },
            parameter_requirements: signature.parameters.iter().map(abi_type).collect(),
            symbol,
            service,
            c_prototype: contract
                .prototypes
                .get(&signature.name)
                .cloned()
                .unwrap_or_default(),
            conversion_plan: Vec::new(),
            callbacks,
            callback_sites: Vec::new(),
            code: None,
        });
    }
    for function in &source.functions {
        let facts = &function.facts;
        let effects = [
            ("may-fail", facts.may_fail),
            ("may-await", facts.may_await),
            ("may-spawn", facts.may_spawn),
            ("worker-safe", facts.worker_safe),
            ("requires-owner", facts.requires_owner),
            ("cleanup-safe", facts.cleanup_safe),
        ]
        .into_iter()
        .filter(|(_, present)| *present)
        .map(|(name, _)| name.to_owned())
        .collect();
        functions.push(FunctionRecord {
            identity: function.declaration.name.clone(),
            error_contract: Some(rinzu_bindgen::source_package::ErrorContractRecord {
                declared: function.declaration.error_contract.is_some(),
                types: function.error_types.clone(),
            }),
            source_type: format!(
                "{}({})->{}",
                if facts.is_async { "async" } else { "fn" },
                function
                    .parameter_types
                    .iter()
                    .map(source_type)
                    .collect::<Vec<_>>()
                    .join(","),
                source_type(&function.result_type)
            ),
            kind: if facts.is_async {
                FunctionKind::Async
            } else {
                FunctionKind::Sync
            },
            effects,
            parameter_requirements: function.parameter_types.iter().map(source_type).collect(),
            symbol: None,
            service: None,
            c_prototype: String::new(),
            conversion_plan: Vec::new(),
            callbacks: Vec::new(),
            callback_sites: native_callback_sites(source, &function.declaration.name)?,
            code: Some(CodeRecord {
                declaration: function.declaration.name.clone(),
                version: 1,
                fallback: function.callback_fallback.as_ref().map(fallback_record),
            }),
        });
    }
    let mut build_inputs =
        crate::native_fingerprint::normalize(&package.manifest.root, &contract.fingerprint)?;
    crate::native_fingerprint::source_inputs(&package.manifest, source, &mut build_inputs)?;
    static COMPILER_DIGEST: std::sync::OnceLock<Result<String, String>> =
        std::sync::OnceLock::new();
    let compiler_digest = COMPILER_DIGEST
        .get_or_init(|| {
            std::env::current_exe()
                .map_err(|error| error.to_string())
                .and_then(|path| {
                    std::fs::read(path)
                        .map(|bytes| sha256(&bytes))
                        .map_err(|error| error.to_string())
                })
        })
        .clone()?;
    build_inputs.insert("rinzu.compiler-binary".to_owned(), compiler_digest);
    build_inputs.extend(crate::toolchain_bundle::build_inputs()?);
    build_inputs.insert(
        "rinzu.native-data-contract".into(),
        rinzu_abi::NATIVE_DATA_CONTRACT_VERSION.to_string(),
    );
    if rinzu_compiler::standard_library::is_standard_manifest(&package.manifest)? {
        build_inputs.insert(
            "rinzu.standard-package".into(),
            package.manifest.name.clone(),
        );
        build_inputs.insert("rinzu.standard-package-version".into(), "0.1.0".into());
    }

    let abi_header =
        rinzu_compiler::standard_library::resource_root()?.join("include/rinzu/runtime.h");
    build_inputs.insert(
        "rinzu.abi-header".to_owned(),
        sha256(&std::fs::read(abi_header).map_err(|error| error.to_string())?),
    );
    let completion_header = rinzu_compiler::standard_library::resource_root()?
        .join("include/rinzu/native_completion.h");
    build_inputs.insert(
        "rinzu.completion-header".to_owned(),
        sha256(&std::fs::read(completion_header).map_err(|error| error.to_string())?),
    );

    for path in package
        .manifest
        .native
        .sources
        .iter()
        .chain(&package.manifest.native.headers)
        .chain(&package.manifest.native.objects)
    {
        build_inputs.insert(
            format!("sdk:{}", path.display()),
            sha256(
                &std::fs::read(package.manifest.root.join(path))
                    .map_err(|error| error.to_string())?,
            ),
        );
    }
    let source_options: BTreeMap<_, _> = package
        .manifest
        .native
        .sources
        .iter()
        .map(|path| {
            (
                path.to_string_lossy().into_owned(),
                source_language_flags(path),
            )
        })
        .collect();
    build_inputs.insert(
        "sdk.source-options".to_owned(),
        serde_json::to_string(&source_options).map_err(|error| error.to_string())?,
    );
    for path in &inputs.objects {
        build_inputs.insert(
            format!("cli:{}", path.display()),
            sha256(&std::fs::read(application_root.join(path)).map_err(|error| error.to_string())?),
        );
    }
    build_inputs.insert(
        "cli.search-paths".to_owned(),
        serde_json::to_string(&inputs.search_paths).map_err(|error| error.to_string())?,
    );
    build_inputs.insert(
        "cli.libraries".to_owned(),
        serde_json::to_string(&inputs.libraries).map_err(|error| error.to_string())?,
    );
    build_inputs.insert(
        "sdk.libraries".to_owned(),
        serde_json::to_string(&package.manifest.native.libraries)
            .map_err(|error| error.to_string())?,
    );
    build_inputs.insert(
        "sdk.frameworks".to_owned(),
        serde_json::to_string(&package.manifest.native.frameworks)
            .map_err(|error| error.to_string())?,
    );
    let exports: Vec<_> = source
        .modules
        .iter()
        .flat_map(|(module, exports)| exports.keys().map(move |name| format!("{module}.{name}")))
        .collect();
    let public_targets: BTreeSet<_> = source
        .modules
        .values()
        .flat_map(|exports| exports.values())
        .collect();
    let constants = source
        .constants
        .iter()
        .filter(|(name, _)| public_targets.contains(name))
        .map(|(name, value)| (name.clone(), value.value.to_string()))
        .collect();
    let provided_header_digest = if package.manifest.native.headers.is_empty() {
        None
    } else {
        let headers: BTreeMap<_, _> = package
            .manifest
            .native
            .headers
            .iter()
            .map(|path| {
                std::fs::read(package.manifest.root.join(path))
                    .map(|bytes| (path.to_string_lossy().into_owned(), sha256(&bytes)))
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<_, _>>()?;
        Some(sha256(
            &serde_json::to_vec(&headers).map_err(|error| error.to_string())?,
        ))
    };
    Ok(SourceInterface {
        package: PackageIdentity {
            package: package.manifest.name.clone(),
            modules: source.modules.keys().cloned().collect(),
            exports,
            language: format!("rinzu-{}", package.manifest.language),
            target: "aarch64-apple-darwin".to_owned(),
            data_layout: "e-m:o-i64:64-i128:128-n32:64-S128".to_owned(),
            toolchain: format!("rinzu-{}", env!("CARGO_PKG_VERSION")),
            native_compiler: contract.fingerprint["clang.version"].clone(),
            build_inputs,
        },
        types: types.into_values().collect(),
        functions,
        backend: BackendRecord {
            native_data_contract: rinzu_abi::NATIVE_DATA_CONTRACT_VERSION,
            callback_rule: 1,
            template_layout: rinzu_abi::TEMPLATE_LAYOUT_VERSION,
            service_registry: rinzu_abi::CLOSED_SERVICE_REGISTRY_VERSION,
            page_size: "16384".to_owned(),
            entry_stride: "32".to_owned(),
            control_stride: "32".to_owned(),
            bridge_abi: "arm64-x16-v1".to_owned(),
            owner_dispatcher: "macos-runloop-v1".to_owned(),
        },
        constants,
        constant_types: source
            .constants
            .iter()
            .filter(|(name, _)| public_targets.contains(name))
            .map(|(name, value)| (name.clone(), value.scalar.name().to_owned()))
            .collect(),
        public_bindings: source
            .modules
            .iter()
            .flat_map(|(module, exports)| {
                exports.iter().map(move |(name, target)| {
                    let identity = match source.type_aliases.get(target) {
                        Some(rinzu_compiler::Type::Named(name))
                            if !name.starts_with("$rinzu_record_")
                                && !name.starts_with("$rinzu_closure_") =>
                        {
                            name.clone()
                        }
                        Some(rinzu_compiler::Type::Native { name, .. }) => name.clone(),
                        _ => target.clone(),
                    };
                    (format!("{module}.{name}"), identity)
                })
            })
            .collect(),
        source_templates: source
            .templates
            .iter()
            .map(|(name, schema)| {
                (
                    name.clone(),
                    SourceTemplateRecord {
                        interface_constraints: schema.interface_constraints.clone(),
                        interfaces: schema.interfaces.clone(),
                        error_contract: schema.error_contract.clone(),
                        methods: schema.methods.clone(),
                        owner_type_parameters: schema.owner_type_parameters.clone(),
                        private_fields: schema.private_fields.clone(),
                        kind: schema.kind.clone(),
                        type_parameters: schema.type_parameters.clone(),
                        signature: schema.signature.clone(),
                        const_fields: schema.const_fields.clone(),
                        fields: schema.fields.clone(),
                        variants: schema.variants.clone(),
                    },
                )
            })
            .collect(),
        source_values: source
            .values
            .iter()
            .map(|(name, ty)| (name.clone(), source_type(ty)))
            .collect(),
        source_types: source
            .types
            .iter()
            .map(|(name, schema)| {
                (
                    name.clone(),
                    SourceTypeRecord {
                        private_fields: schema.private_fields.clone(),
                        kind: schema.kind.clone(),
                        const_fields: schema.const_fields.clone(),
                        fields: schema
                            .fields
                            .iter()
                            .map(|(name, ty)| (name.clone(), ty.to_string()))
                            .collect(),
                        variants: schema
                            .variants
                            .iter()
                            .map(|(name, fields)| {
                                (
                                    name.clone(),
                                    fields
                                        .iter()
                                        .map(|(name, ty)| (name.clone(), ty.to_string()))
                                        .collect(),
                                )
                            })
                            .collect(),
                    },
                )
            })
            .chain(
                source
                    .signatures
                    .iter()
                    .flat_map(|signature| {
                        signature
                            .parameters
                            .iter()
                            .chain(std::iter::once(&signature.result))
                    })
                    .filter_map(|ty| {
                        if let SourceAbiType::Opaque(name) = ty {
                            Some((
                                name.clone(),
                                SourceTypeRecord {
                                    private_fields: Vec::new(),
                                    kind: "opaque".into(),
                                    const_fields: Vec::new(),
                                    fields: Vec::new(),
                                    variants: Vec::new(),
                                },
                            ))
                        } else {
                            None
                        }
                    }),
            )
            .collect(),
        provided_header_digest,
    })
}

fn fallback_record(value: &rinzu_compiler::FallbackConstant) -> FallbackRecord {
    use rinzu_compiler::FallbackConstant as F;
    let (kind, scalar, value, bytes_length) = match value {
        F::Unit => ("unit", None, String::new(), None),
        F::Integer { scalar, value } => (
            "integer",
            Some(scalar.name().to_owned()),
            value.to_string(),
            None,
        ),
        F::Float { scalar, bits } => (
            "float-bits",
            Some(scalar.name().to_owned()),
            bits.to_string(),
            None,
        ),
        F::Bool(value) => ("boolean", None, value.to_string(), None),
        F::String(value) => ("string", None, value.clone(), None),
        F::Bytes { prefix, length } => (
            "bytes",
            None,
            prefix.iter().map(|byte| format!("{byte:02x}")).collect(),
            Some(length.to_string()),
        ),
    };
    FallbackRecord {
        kind: kind.to_owned(),
        scalar,
        value,
        bytes_length,
    }
}
