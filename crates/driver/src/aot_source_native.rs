use crate::source_native_package::BuiltSourcePackage;
use rinzu_bindgen::source_package::{decode_key, sha256, validate_source_descriptor};
use rinzu_codegen::BaselineAotObject;
use rinzu_compiler::{AbiScalar, FallbackConstant, SourceAbiType, Type};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

fn bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(",")
}
fn literal(text: &str) -> String {
    serde_json::to_string(text).unwrap()
}

fn opaque_index(aot: &BaselineAotObject, identity: &str) -> Result<usize, String> {
    aot.data_type(&rinzu_compiler::Type::Native {
        name: identity.to_owned(),
        call_only: false,
    })
    .filter(|ty| ty.managed)
    .map(|ty| ty.index)
    .ok_or_else(|| format!("opaque nominal type is absent from the Owner catalog: {identity}"))
}

fn managed_index(aot: &BaselineAotObject, ty: &rinzu_compiler::Type) -> Result<usize, String> {
    aot.data_type(ty)
        .filter(|entry| entry.managed)
        .map(|entry| entry.index)
        .ok_or_else(|| format!("native managed result is absent from the Owner catalog: {ty}"))
}
fn record_index(aot: &BaselineAotObject, identity: &str) -> Result<u64, String> {
    aot.native_record_schemas
        .keys()
        .position(|name| name == identity)
        .map(|index| index as u64 + 1)
        .ok_or_else(|| format!("missing native record schema: {identity}"))
}

pub fn record_layouts(
    aot: &BaselineAotObject,
    packages: &[BuiltSourcePackage],
) -> Result<String, String> {
    let descriptors = packages
        .iter()
        .map(|package| validate_source_descriptor(&package.artifact.expected_wire))
        .collect::<Result<Vec<_>, _>>()?;
    let mut source = String::from("#define RINZU_AOT_PREPARED_RECORD_RECEIVERS 1\n");
    let mut layouts = Vec::new();
    let mut prepared_receivers = Vec::new();
    let mut prepared_cases = Vec::new();
    for (index, (identity, fields)) in aot.native_record_schemas.iter().enumerate() {
        let actual = rinzu_bindgen::source_package::matching_record_layout(
            descriptors
                .iter()
                .flat_map(|descriptor| &descriptor.interface.types),
            identity,
        )?;
        if actual.canonical != *identity
            || actual.bytes_length.is_some()
            || actual.fields.len() != fields.len()
        {
            return Err(format!(
                "invalid native record identity/field count: {identity}"
            ));
        }
        let mut native_fields = Vec::new();
        let mut entries = Vec::new();
        let mut offsets = Vec::new();
        for (actual, (name, scalar)) in actual.fields.iter().zip(fields) {
            if actual.name != *name || actual.c_type != scalar.c_type() {
                return Err(format!("native record field mismatch: {identity}.{name}"));
            }
            let offset = actual
                .offset
                .parse::<usize>()
                .map_err(|error| error.to_string())?;
            let kind = rinzu_runtime::scalar::ScalarKind::from_tag(*scalar as u32)
                .ok_or("invalid native scalar")?;
            native_fields.push(rinzu_runtime::data::NativeRecordField { kind, offset });
            entries.push(format!("{{{},{offset}}}", *scalar as u32));
            offsets.push(offset);
        }
        let ty = managed_index(aot, &rinzu_compiler::Type::Named(identity.clone()))?;
        let layout = rinzu_runtime::data::NativeRecordLayout::new(
            ty,
            actual
                .size
                .parse()
                .map_err(|error: std::num::ParseIntError| error.to_string())?,
            actual
                .alignment
                .parse()
                .map_err(|error: std::num::ParseIntError| error.to_string())?,
            native_fields,
        )
        .map_err(|error| format!("invalid target record layout: {error:?}"))?;
        source.push_str(&format!("static const rinzu_native_record_field_t rinzu_native_record_fields_{index}[] = {{{}}};\n", entries.join(",")));
        layouts.push(format!(
            "{{{ty},{},{},{},rinzu_native_record_fields_{index}}}",
            layout.size(),
            layout.alignment(),
            fields.len()
        ));
        // Only the generated result receiver may reuse prepare's layout proof.
        // These freshly generated reference0 slots have canonical wide scalars.
        // Construct fully initialized fresh metadata with one final owned payload.
        // Mixed/narrow/Bool records
        // keep the generic receiver, including all conversion checks.
        if fields.len() > 1
            && fields
                .iter()
                .all(|(_, scalar)| matches!(scalar, AbiScalar::I64 | AbiScalar::U64))
        {
            let record_index = index + 1;
            let count = fields.len();
            let mut receiver = format!(
                "static inline int rinzu_native_receive_record_{record_index}_prepared(rinzu_managed_data_t *data, const unsigned char *input, uint64_t *output) {{\n\
                 if (!output) rinzu_failure_fatal();\n\
                 if (!input) return RINZU_NATIVE_NULL_RECORD;\n\
                 _Static_assert({count} <= SIZE_MAX / sizeof(rinzu_managed_slot_t), \"slot bound\");\n\
                 rinzu_managed_slot_t *slots = rinzu_failure_allocate({count} * sizeof(*slots));\n\
                 uint64_t bits;\n"
            );
            for (field, offset) in offsets.iter().enumerate() {
                receiver.push_str(&format!(
                    "memcpy(&bits, input + {offset}, 8);\nslots[{field}] = (rinzu_managed_slot_t){{bits, 0}};\n"
                ));
            }
            receiver.push_str(&format!(
                "rinzu_managed_value_t *value = rinzu_managed_new_owned(data, RINZU_MANAGED_COMPOSITE);\n\
                 value->type = {ty};\nvalue->has_variant = 0;\nvalue->variant = 0;\n\
                 value->count = {count};\nvalue->slots = slots;\n\
                 rinzu_managed_account_payload(data, value, {count} * sizeof(*slots));\n\
                 *output = value->identity;\nreturn RINZU_NATIVE_RECORD_OK;\n}}\n"
            ));
            prepared_receivers.push(receiver);
            prepared_cases.push(format!(
                "case {record_index}:\n\
                 rinzu_managed_owner(data);\n\
                 if (!root || root->data != data || data->schema != &rinzu_program_schema) break;\n\
                 return rinzu_native_receive_record_{record_index}_prepared(data, input, output);\n"
            ));
        }
    }
    source.push_str(&format!("static const rinzu_native_record_layout_t rinzu_native_record_layouts[] = {{{}}};\nstatic const size_t rinzu_native_record_layout_count = {};\n",
        if layouts.is_empty(){"{0,0,0,0,NULL}".into()}else{layouts.join(",")},layouts.len()));
    source.push_str(
        "/* Private prepared-result entry: prepare validated this immutable layout\n\
         and schema before publishing the call. Keep fresh Owner and require its\n\
         attached Domain plus this program schema; otherwise use generic checks.\n\
         Callback arguments and storage snapshots always use the generic API. */\n\
         static const rinzu_native_record_layout_t *rinzu_native_record_layout(uint64_t index);\n",
    );
    for receiver in prepared_receivers {
        source.push_str(&receiver);
    }
    source.push_str(
        "static __attribute__((unused)) int rinzu_native_receive_record_prepared(rinzu_managed_data_t *data, const rinzu_managed_root_set_t *root, uint64_t index, const unsigned char *input, uint64_t *output) {\n\
         const rinzu_native_record_layout_t *layout = rinzu_native_record_layout(index);\n",
    );
    if prepared_cases.is_empty() {
        source.push_str("(void)root;\n");
    } else {
        source.push_str("switch (index) {\n");
        for case in prepared_cases {
            source.push_str(&case);
        }
        source.push_str("default: break;\n}\n");
    }
    source.push_str("return rinzu_managed_receive_record(data, layout, input, output);\n}\n");
    Ok(source)
}

fn callback_type(ty: &SourceAbiType) -> Result<Type, String> {
    Ok(match ty {
        SourceAbiType::Unit => Type::Unit,
        SourceAbiType::Scalar(scalar) => Type::from_scalar(*scalar),
        SourceAbiType::String => Type::String,
        SourceAbiType::Bytes(length) => Type::FixedBytes(*length),
        SourceAbiType::Opaque(name) => Type::Native {
            name: name.clone(),
            call_only: false,
        },
        SourceAbiType::Record(name) | SourceAbiType::UserData(name) => Type::Named(name.clone()),
        _ => return Err("invalid managed callback ABI type".into()),
    })
}

fn fallback_bits(fallback: Option<&FallbackConstant>) -> Result<Option<u64>, String> {
    use rinzu_runtime::scalar::{ScalarKind, ScalarValue};
    let kind =
        |scalar: AbiScalar| ScalarKind::from_tag(scalar as u32).ok_or("invalid callback scalar");
    Ok(match fallback {
        None => None,
        Some(FallbackConstant::Unit) => Some(0),
        Some(FallbackConstant::Bool(value)) => Some(u64::from(*value)),
        Some(FallbackConstant::Integer { scalar, value }) => Some(
            ScalarValue::from_integer(kind(*scalar)?, *value)
                .map_err(|error| format!("invalid callback fallback: {error:?}"))?
                .bits(),
        ),
        Some(FallbackConstant::Float { scalar, bits }) => Some(
            ScalarValue::from_bits(kind(*scalar)?, *bits)
                .map_err(|error| format!("invalid callback fallback: {error:?}"))?
                .bits(),
        ),
        _ => return Err("callback buffer fallback requires the R3 process arena".into()),
    })
}

fn input(aot: &BaselineAotObject, ty: &SourceAbiType) -> Result<(u32, u32, u64), String> {
    Ok(match ty {
        SourceAbiType::Scalar(scalar) => (0, *scalar as u32, 0),
        SourceAbiType::String => (1, 0, 0),
        SourceAbiType::Bytes(length) => (2, 0, *length),
        SourceAbiType::ByteArray => (8, 0, 0),
        SourceAbiType::Opaque(identity) => (4, 0, opaque_index(aot, identity)? as u64),
        SourceAbiType::Record(identity) => (5, 0, record_index(aot, identity)?),
        SourceAbiType::UserData(identity) => (
            7,
            0,
            managed_index(aot, &Type::Named(identity.clone()))? as u64,
        ),
        _ => return Err("invalid native input type".into()),
    })
}

pub fn plans(aot: &BaselineAotObject, packages: &[BuiltSourcePackage]) -> Result<String, String> {
    let descriptors = packages
        .iter()
        .map(|package| validate_source_descriptor(&package.artifact.expected_wire))
        .collect::<Result<Vec<_>, _>>()?;
    let consumers =
        rinzu_codegen::source_package::return_consumers(&descriptors, &aot.native_signatures)
            .map_err(|error| error.to_string())?;
    let mut source = String::new();
    let mut signatures = aot
        .native_signatures
        .iter()
        .filter(|signature| !signature.is_async)
        .collect::<Vec<_>>();
    signatures.sort_by(|a, b| a.name.cmp(&b.name));
    for (index, _) in signatures
        .iter()
        .enumerate()
        .filter(|(_, signature)| signature.is_supplant)
    {
        source.push_str(&format!(
            "extern uint64_t rinzu_source_native_{index}(uint64_t,const uint64_t *);\n"
        ));
    }
    for (plan_index, plan) in aot.native_call_plans.iter().enumerate() {
        for site in &plan.callbacks {
            let targets = site
                .targets
                .iter()
                .map(|target| {
                    aot.reference_targets
                        .iter()
                        .position(|name| name == target)
                        .map(|index| (index + 1).to_string())
                        .ok_or_else(|| "unknown callback plan target".to_owned())
                })
                .collect::<Result<Vec<_>, _>>()?;
            source.push_str(&format!(
                "static const uint64_t rinzu_native_targets_{plan_index}_{}[] = {{{}}};\n",
                site.index,
                if targets.is_empty() {
                    "0".into()
                } else {
                    targets.join(",")
                }
            ));
        }
        let mut parameters = Vec::new();
        for (index, ty) in plan.signature.parameters.iter().enumerate() {
            let (kind, scalar, length, targets, count, callback) = match ty {
                SourceAbiType::Callback { parameters, result } => {
                    let (result_tag, result_length) = match **result {
                        SourceAbiType::Unit => (0, 0),
                        SourceAbiType::Scalar(scalar) => (scalar as u32 + 1, 0),
                        SourceAbiType::String => (13, 0),
                        SourceAbiType::Bytes(length) => (14, length),
                        _ => {
                            return Err("invalid native callback result type".into());
                        }
                    };
                    let site = plan
                        .callbacks
                        .iter()
                        .find(|site| site.index == index)
                        .ok_or("missing callback site")?;
                    let mut managed_parameters = parameters
                        .iter()
                        .map(callback_type)
                        .collect::<Result<Vec<_>, _>>()?;
                    if let Some(capture) = &site.capture {
                        managed_parameters.insert(0, capture.clone());
                    }
                    for target in &site.targets {
                        if let Some(function) = aot
                            .functions
                            .iter()
                            .find(|function| function.name == *target)
                        {
                            if function.is_async
                                || function.parameter_types != managed_parameters
                                || function.result_type != callback_type(result)?
                            {
                                return Err(
                                    "managed callback target disagrees with its physical signature"
                                        .into(),
                                );
                            }
                        } else if let Some(signature) = aot
                            .native_signatures
                            .iter()
                            .find(|signature| signature.name == *target)
                        {
                            if signature.is_async
                                || signature.is_supplant
                                || signature.parameters != *parameters
                                || signature.result != **result
                            {
                                return Err("raw native callback signature mismatch".into());
                            }
                        } else {
                            let expected = if target == "rinzu.core::utf8ByteLength" {
                                Some(SourceAbiType::String)
                            } else {
                                target
                                    .strip_prefix("rinzu.core::byteLength:")
                                    .and_then(|n| n.parse().ok())
                                    .map(SourceAbiType::Bytes)
                            };
                            if expected.is_none()
                                || parameters != &[expected.unwrap()]
                                || **result != SourceAbiType::Scalar(AbiScalar::U64)
                            {
                                return Err("unknown or incompatible core callback target".into());
                            }
                        }
                    }
                    let inputs = parameters
                        .iter()
                        .map(|ty| {
                            callback_type(ty)?;
                            let (kind, scalar, length) = input(aot, ty)?;
                            Ok(format!(
                                "{{{kind},{scalar},UINT64_C({length}),NULL,0,NULL}}"
                            ))
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    if !inputs.is_empty() {
                        source.push_str(&format!("static const RinzuNativeParameter rinzu_callback_inputs_{plan_index}_{index}[] = {{{}}};\n",inputs.join(",")));
                    }
                    let capture_type = site
                        .capture
                        .as_ref()
                        .map(|ty| managed_index(aot, ty))
                        .transpose()?
                        .unwrap_or(0);
                    source.push_str(&format!("static const RinzuNativeCallbackPlan rinzu_callback_plan_{plan_index}_{index} = {{{},{},{result_tag},{},UINT64_C({result_length}),{},{},{capture_type}}};\n",if inputs.is_empty(){"NULL".into()}else{format!("rinzu_callback_inputs_{plan_index}_{index}")},inputs.len(),u32::from(parameters.iter().any(|ty|matches!(ty,SourceAbiType::UserData(_)))),u32::from(ty.foreign_callback_eligible()),u32::from(site.capture.is_some())));
                    (
                        3,
                        0,
                        0,
                        format!("rinzu_native_targets_{plan_index}_{index}"),
                        site.targets.len(),
                        format!("&rinzu_callback_plan_{plan_index}_{index}"),
                    )
                }
                _ => {
                    let (kind, scalar, length) = input(aot, ty)?;
                    (kind, scalar, length, "NULL".into(), 0, "NULL".into())
                }
            };
            parameters.push(format!(
                "{{{kind},{scalar},UINT64_C({length}),{targets},{count},{callback}}}"
            ));
        }
        if !parameters.is_empty() {
            source.push_str(&format!("static const RinzuNativeParameter rinzu_native_parameters_{plan_index}[] = {{{}}};\n", parameters.join(",")));
        }
    }
    let plans = aot
        .native_call_plans
        .iter()
        .enumerate()
        .map(|(index, plan)| {
            let (result, result_type, result_length) = match &plan.signature.result {
                SourceAbiType::Unit => (0, 0, 0),
                SourceAbiType::Scalar(scalar) => (*scalar as u32 + 1, 0, 0),
                SourceAbiType::Opaque(identity) => (12, opaque_index(aot, identity)?, 0),
                SourceAbiType::String => {
                    (13, managed_index(aot, &rinzu_compiler::Type::String)?, 0)
                }
                SourceAbiType::Bytes(length) => (
                    14,
                    managed_index(aot, &rinzu_compiler::Type::FixedBytes(*length))?,
                    *length,
                ),
                SourceAbiType::ByteArrayResult => (16, managed_index(aot, &Type::Bytes)?, 0),
                SourceAbiType::UserData(identity) => {
                    (17, managed_index(aot, &Type::Named(identity.clone()))?, 0)
                }
                SourceAbiType::Record(identity) => (
                    15,
                    managed_index(aot, &rinzu_compiler::Type::Named(identity.clone()))?,
                    record_index(aot, identity)?,
                ),
                _ => {
                    return Err(
                        "native result requires the remaining R3 receiver backend".to_owned()
                    );
                }
            };
            Ok(format!(
                "{{{},{},{result},{result_type},UINT64_C({result_length}),{},{},{}}}",
                if plan.signature.parameters.is_empty() {
                    "NULL".into()
                } else {
                    format!("rinzu_native_parameters_{index}")
                },
                plan.signature.parameters.len(),
                if plan.signature.is_supplant {
                    format!(
                        "rinzu_source_native_{}",
                        signatures
                            .iter()
                            .position(|signature| signature.name == plan.signature.name)
                            .ok_or("missing supplant ABI stub")?
                    )
                } else {
                    "NULL".to_owned()
                },
                literal(match &plan.signature.binding {
                    rinzu_compiler::ast::NativeBinding::Symbol(symbol) => symbol,
                    _ => &plan.signature.name,
                }),
                u32::from(consumers.contains(&plan.signature.name))
            ))
        })
        .collect::<Result<Vec<_>, _>>()?;
    source.push_str(&format!("static const RinzuNativePlan rinzu_native_plans[] = {{{}}};\nstatic const size_t rinzu_native_plan_count = {};\nstatic const void *rinzu_native_addresses[{}];\n", if plans.is_empty() { "{NULL,0,0,0,0,NULL,NULL,0}".into() } else { plans.join(",") }, plans.len(), plans.len().max(1)));
    for (index, target) in aot.reference_targets.iter().enumerate() {
        if let Some(function) = aot
            .functions
            .iter()
            .find(|function| function.name == *target)
        {
            let arguments = (0..function.parameter_count)
                .map(|i| format!(", (int64_t)words[{i}]"))
                .collect::<String>();
            let parameters = (0..function.parameter_count)
                .map(|_| ", int64_t")
                .collect::<String>();
            source.push_str(&format!("extern int64_t {}(void *{});\nstatic uint64_t rinzu_callback_body_{index}(void *context,const uint64_t *words) {{(void)words;return (uint64_t){}(context{arguments});}}\n",function.symbol,parameters,function.symbol));
        }
    }
    let targets =
        aot.reference_targets
            .iter()
            .enumerate()
            .map(|(index, target)| {
                if target == "rinzu.core::utf8ByteLength" {
                    Ok("{2,0,NULL,0,0,{0},NULL}".to_owned())
                } else if let Some(n) = target
                    .strip_prefix("rinzu.core::byteLength:")
                    .and_then(|n| n.parse::<u64>().ok())
                {
                    Ok(format!("{{1,UINT64_C({n}),NULL,0,0,{{0}},NULL}}"))
                } else if let Some(function) = aot
                    .functions
                    .iter()
                    .find(|function| function.name == *target)
                {
                    let (fallback, buffer) = match function.callback_fallback.as_ref() {
                        Some(FallbackConstant::String(text)) => {
                            (Some(0), Some((text.as_bytes(), text.len() as u64, true)))
                        }
                        Some(FallbackConstant::Bytes { prefix, length }) => {
                            (Some(0), Some((prefix.as_slice(), *length, false)))
                        }
                        scalar => (fallback_bits(scalar)?, None),
                    };
                    let buffer = if let Some((prefix, length, string)) = buffer {
                        source.push_str(&format!(
                        "static const unsigned char rinzu_callback_fallback_{index}[] = {{{}}};\n",
                        if prefix.is_empty() { "0".into() } else { bytes(prefix) }
                    ));
                        format!(
                            "{{rinzu_callback_fallback_{index},{},UINT64_C({length}),{}}}",
                            prefix.len(),
                            u32::from(string)
                        )
                    } else {
                        "{0}".into()
                    };
                    Ok(format!(
                        "{{3,0,rinzu_callback_body_{index},{},UINT64_C({}),{buffer},{}}}",
                        u32::from(fallback.is_some()),
                        fallback.unwrap_or(0),
                        literal(&function.name)
                    ))
                } else if aot
                    .native_signatures
                    .iter()
                    .any(|signature| signature.name == *target)
                {
                    Ok("{4,0,NULL,0,0,{0},NULL}".to_owned())
                } else {
                    Ok("{0,0,NULL,0,0,{0},NULL}".to_owned())
                }
            })
            .collect::<Result<Vec<_>, String>>()?;
    source.push_str(&format!("static const RinzuNativeTarget rinzu_native_targets[] = {{{}}};\nstatic const size_t rinzu_native_target_count = {};\nstatic const void *rinzu_native_target_addresses[{}];\n", if targets.is_empty() { "{0,0,NULL,0,0,{0},NULL}".into() } else { targets.join(",") }, targets.len(),targets.len().max(1)));
    Ok(source)
}

pub fn startup(
    aot: &BaselineAotObject,
    packages: &[BuiltSourcePackage],
    bundle: &Path,
    layout: crate::app_layout::Layout,
) -> Result<String, String> {
    let bundle = bundle.to_str().ok_or("source bundle path is not UTF-8")?;
    let mut source = String::from(include_str!("aot_source_package.c"));
    let mut setup = format!(
        "static void rinzu_aot_source_native_startup(void) {{\n{}",
        layout.startup(&literal(bundle))
    );
    let mut bindings = BTreeMap::new();
    let mut identities = BTreeSet::new();
    let mut keys = BTreeSet::new();
    for (index, package) in packages.iter().enumerate() {
        let descriptor = validate_source_descriptor(&package.artifact.expected_wire)?;
        if !identities.insert(descriptor.interface.package.package.clone()) {
            return Err("duplicate source package identity".into());
        }
        let subdirectory = format!(
            "sdk-{}",
            &sha256(descriptor.interface.package.package.as_bytes())[..24]
        );
        let generated = package
            .descriptor
            .parent()
            .ok_or("source artifact has no directory")?;
        for name in [
            "source-package-v2.json",
            "source-package.h",
            "source-table.c",
            "source-wrappers.c",
            "native-contract.h",
        ] {
            let digest = decode_key(&sha256(
                &std::fs::read(generated.join(name)).map_err(|e| e.to_string())?,
            ))?;
            setup.push_str(&format!("{{ char *path = rinzu_source_path(directory, {}); const unsigned char digest[32] = {{{}}}; rinzu_source_file(path, digest); free(path); }}\n", literal(&layout.file(&format!("{subdirectory}/{name}"))?), bytes(&digest)));
        }
        if let Some(library) = &package.artifact.library {
            let binding_plan = rinzu_bindgen::native_binding::NativeBindingPlan::decode(
                &descriptor.interface.package.build_inputs,
            )?;
            let binding_arguments = if let Some(plan) = binding_plan {
                plan.validate_functions(&descriptor.interface.functions)?;
                let images = plan
                    .images
                    .iter()
                    .map(|image| -> Result<String, String> {
                        let path = if image.bundled {
                            layout.file(&format!("{subdirectory}/{}", image.path))?
                        } else {
                            image.path.clone()
                        };
                        let digest = image
                            .sha256
                            .as_deref()
                            .map(decode_key)
                            .transpose()?
                            .unwrap_or([0; 32]);
                        Ok(format!(
                            "{{{},{},{},{},{{{}}}}}",
                            literal(&path),
                            usize::from(image.bundled),
                            literal(&image.uuid),
                            usize::from(image.sha256.is_some()),
                            bytes(&digest)
                        ))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let slots = plan
                    .slots
                    .iter()
                    .map(|slot| -> Result<String, String> {
                        let owner = if plan.owner_bundled(slot) {
                            layout.file(&format!("{subdirectory}/{}", slot.owner_path))?
                        } else {
                            slot.owner_path.clone()
                        };
                        let digest = slot
                            .owner_sha256
                            .as_deref()
                            .map(decode_key)
                            .transpose()?
                            .unwrap_or([0; 32]);
                        Ok(format!(
                            "{{{},{},{},{},{},{},{},{{{}}}}}",
                            slot.image,
                            literal(&slot.symbol),
                            slot.resolver.as_deref().map_or("NULL".into(), literal),
                            literal(&owner),
                            usize::from(plan.owner_bundled(slot)),
                            literal(&slot.owner_uuid),
                            usize::from(slot.owner_sha256.is_some()),
                            bytes(&digest)
                        ))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                source.push_str(&format!("static const RinzuNativeImage rinzu_source_images_{index}[] = {{{}}};\nstatic const RinzuNativeSlot rinzu_source_slots_{index}[] = {{{}}};\n", images.join(","), slots.join(",")));
                let digest = descriptor
                    .interface
                    .package
                    .build_inputs
                    .get(rinzu_bindgen::native_binding::DIGEST_KEY)
                    .ok_or("native binding plan digest is absent")?;
                format!(
                    "directory, rinzu_source_images_{index}, {}, rinzu_source_slots_{index}, {}, {}",
                    images.len(),
                    slots.len(),
                    literal(digest)
                )
            } else {
                "directory, NULL, 0, NULL, 0, NULL".into()
            };
            let filename = library
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("facade filename is not UTF-8")?;
            let digest = decode_key(
                package
                    .artifact
                    .facade_digest
                    .as_deref()
                    .ok_or("missing facade digest")?,
            )?;
            source.push_str(&format!(
                "static const uint8_t rinzu_source_wire_{index}[] = {{{}}};\n",
                bytes(&package.artifact.expected_wire)
            ));
            let mut imports = Vec::new();
            for (function, key) in descriptor
                .interface
                .functions
                .iter()
                .zip(&descriptor.import_keys)
            {
                if let Some(symbol) = &function.symbol {
                    if !keys.insert(key.clone()) || bindings.contains_key(&function.identity) {
                        return Err("duplicate source native import identity".into());
                    }
                    bindings.insert(
                        function.identity.clone(),
                        (index, imports.len(), function.clone()),
                    );
                    imports.push(format!(
                        "{{{{{}}},{}}}",
                        bytes(&decode_key(key)?),
                        literal(symbol)
                    ));
                }
            }
            source.push_str(&format!(
                "static const RinzuExpectedImport rinzu_source_imports_{index}[] = {{{}}};\n",
                imports.join(",")
            ));
            setup.push_str(&format!("const void *staged_{index}[{}] = {{0}};\n{{ char *path = rinzu_source_path(directory, {}); const unsigned char digest[32] = {{{}}}; rinzu_source_file(path, digest); rinzu_source_table(path, rinzu_source_wire_{index}, sizeof rinzu_source_wire_{index}, rinzu_source_imports_{index}, {}, staged_{index}, {binding_arguments}); free(path); }}\n", imports.len().max(1), literal(&layout.file(&format!("{subdirectory}/{filename}"))?), bytes(&digest), imports.len()));
        }
    }
    for (index, plan) in aot.native_call_plans.iter().enumerate() {
        let (package, import, function) = bindings
            .get(&plan.signature.name)
            .ok_or_else(|| format!("missing AOT source binding {}", plan.signature.name))?;
        if function.kind
            != if plan.signature.is_supplant {
                rinzu_bindgen::source_package::FunctionKind::Supplant
            } else {
                rinzu_bindgen::source_package::FunctionKind::Native
            }
            || function.source_type != plan.signature.canonical_type()
            || function.parameter_requirements
                != plan
                    .signature
                    .parameters
                    .iter()
                    .map(SourceAbiType::canonical_name)
                    .collect::<Vec<_>>()
        {
            return Err("AOT native signature disagrees with validated artifact".into());
        }
        setup.push_str(&format!(
            "rinzu_native_addresses[{index}] = staged_{package}[{import}];\n"
        ));
    }
    for (index, target) in aot.reference_targets.iter().enumerate() {
        if let Some(signature) = aot
            .native_signatures
            .iter()
            .find(|signature| signature.name == *target)
        {
            let (package, import, function) = bindings
                .get(target)
                .ok_or("missing raw native callback binding")?;
            if function.source_type != signature.canonical_type() {
                return Err("raw native callback disagrees with validated artifact".into());
            }
            setup.push_str(&format!(
                "rinzu_native_target_addresses[{index}] = staged_{package}[{import}];\n"
            ));
        }
    }
    setup.push_str("free(directory);\n}\n");
    source.push_str(&setup);
    Ok(source)
}
