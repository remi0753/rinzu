//! Decode layouts indexed by the same type IDs as the Owner heap.
use rinzu_compiler::{DataOperation, IrModule, OperationKind, Type};
use rinzu_runtime::json::{self, Field, Schema};
use rinzu_runtime::scalar::ScalarKind;
use rinzu_runtime::value::{ValueSchema, ValueType};
use std::fmt::Write;

pub(crate) fn build(module: &IrModule, types: &[Type], values: &ValueSchema) -> Option<Schema> {
    if !module
        .functions
        .iter()
        .flat_map(|f| &f.blocks)
        .flat_map(|b| &b.operations)
        .any(|op| matches!(op.kind, OperationKind::DataOperation { kind, .. } if kind.is_json()))
    {
        return None;
    }
    let errors = [
        "DecodeFailed",
        "InvalidLimits",
        "LimitExceeded",
        "TypeMismatch",
        "NumberError",
        "SchemaMismatch",
    ]
    .map(|name| {
        let namespaced = format!("rinzu.json::{name}");
        types
            .iter()
            .position(|ty| matches!(ty, Type::Named(identity) if *identity == namespaced))
            .or_else(|| {
                types
                    .iter()
                    .position(|ty| matches!(ty, Type::Named(identity) if identity == name))
            })
            .map_or(json::NONE, word)
    });
    let mut plan = Schema {
        errors,
        ..Default::default()
    };
    // Dynamic JSON needs the error catalog but no decode table.
    if !module
        .functions
        .iter()
        .flat_map(|f| &f.blocks)
        .flat_map(|b| &b.operations)
        .any(|op| {
            matches!(
                op.kind,
                OperationKind::DataOperation {
                    kind: DataOperation::JsonDecode | DataOperation::JsonDecodeWithLimits,
                    ..
                }
            )
        })
    {
        return Some(plan);
    }
    for index in 0..types.len() {
        let mut entry = json::Type::default();
        entry.kind = match values.ty(index).expect("closed JSON type id") {
            ValueType::Scalar(ScalarKind::Bool) => json::BOOL_TY,
            ValueType::Scalar(ScalarKind::I64) => json::INT_TY,
            ValueType::Scalar(ScalarKind::F64) => json::FLOAT_TY,
            ValueType::String => json::STRING_TY,
            ValueType::List(element) => {
                entry.child = word(*element);
                json::LIST_TY
            }
            ValueType::Map { key, value } if types[*key] == Type::String => {
                entry.child = word(*value);
                json::MAP_TY
            }
            ValueType::Record { identity, fields } if identity.starts_with("$rinzu_record_") => {
                entry.first = word(plan.fields.len());
                entry.count = word(fields.len());
                for ((name, _), &ty) in module.source_types[identity].fields.iter().zip(fields) {
                    plan.fields.push(Field {
                        name: word(plan.names.len()),
                        length: word(name.len()),
                        ty: word(ty),
                    });
                    plan.names.extend_from_slice(name.as_bytes());
                }
                json::RECORD_TY
            }
            ValueType::Enum { identity, variants } if identity.starts_with("Option<") => {
                match variants.as_slice() {
                    [none, some] if none.is_empty() && some.len() == 1 => {
                        entry.child = word(some[0]);
                        json::OPTION_TY
                    }
                    _ => 0,
                }
            }
            _ => 0,
        };
        plan.types.push(entry);
    }
    Some(plan)
}

fn word(value: usize) -> u32 {
    u32::try_from(value).expect("JSON schema table exceeds the compiler's index range")
}

pub(crate) fn emit_c(out: &mut String, schema: &Schema) {
    out.push_str("#define RINZU_AOT_HAS_JSON 1\n#include \"rinzu/json.h\"\n");
    out.push_str("static const uint32_t rinzu_program_json_errors[] = {");
    for id in schema.errors {
        write!(out, "UINT32_C({id}),").unwrap();
    }
    out.push_str("};\nstatic const RinzuJsonType rinzu_program_json_types[] = {\n");
    for entry in &schema.types {
        writeln!(
            out,
            "{{{},{},{},{}}},",
            entry.kind, entry.child, entry.first, entry.count
        )
        .unwrap();
    }
    if schema.types.is_empty() {
        out.push_str("{0},\n");
    }
    out.push_str("};\nstatic const RinzuJsonField rinzu_program_json_fields[] = {\n");
    for field in &schema.fields {
        writeln!(out, "{{{},{},{}}},", field.name, field.length, field.ty).unwrap();
    }
    if schema.fields.is_empty() {
        out.push_str("{0},\n");
    }
    out.push_str("};\nstatic const unsigned char rinzu_program_json_names[] = {");
    for byte in &schema.names {
        write!(out, "{byte},").unwrap();
    }
    out.push_str("0};\n");
}
