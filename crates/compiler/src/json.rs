//! Private signatures behind the source-level standard JSON module.
use crate::{DataOperation, Type, records};

pub(crate) fn private_operation(name: &str) -> Option<&'static str> {
    let suffix = name.strip_prefix("json")?;
    DataOperation::ALL
        .into_iter()
        .filter(|op| op.is_json())
        .find_map(|op| (op.name().strip_prefix("$json::") == Some(suffix)).then_some(op.name()))
}

fn is_named(ty: &Type, expected: &str) -> bool {
    matches!(ty, Type::Named(name) if name == expected || name.strip_prefix("rinzu.json::") == Some(expected))
}

fn option(result: &Type, payload: &Type) -> bool {
    matches!(result, Type::Named(name) if *name == format!("Option<{}>", payload.to_string().replace(", ", ",")))
}

fn limits(ty: &Type, bytes: &str) -> bool {
    matches!(ty, Type::Named(name) if *name == records::identity(&[
        ("maxDepth".into(), Type::Int),
        ("maxNodes".into(), Type::Int),
        (bytes.into(), Type::Int),
    ]))
}

pub(crate) fn signature(op: DataOperation, arguments: &[Type], result: &Type) -> bool {
    use DataOperation::*;
    let value = |ty| is_named(ty, "Value");
    match (op, arguments) {
        (
            JsonParse | JsonDecode | JsonParseWithLimits | JsonDecodeWithLimits,
            [Type::String | Type::Bytes, rest @ ..],
        ) => {
            (matches!(op, JsonDecode | JsonDecodeWithLimits) || value(result))
                && match rest {
                    [] => matches!(op, JsonParse | JsonDecode),
                    [bounds] => {
                        matches!(op, JsonParseWithLimits | JsonDecodeWithLimits)
                            && limits(bounds, "maxInputBytes")
                    }
                    _ => false,
                }
        }
        (JsonKind, [input]) => value(input) && is_named(result, "Kind"),
        (JsonField, [input, Type::String]) | (JsonAt, [input, Type::Int]) => {
            value(input) && option(result, input)
        }
        (JsonArrayLength | JsonObjectLength | JsonInt, [input]) => {
            value(input) && *result == Type::Int
        }
        (JsonEntryAt, [input, Type::Int]) if value(input) => option(
            result,
            &Type::Named(format!("{{key:String,value:{input}}}")),
        ),
        (JsonBool, [input]) => value(input) && *result == Type::Bool,
        (JsonString | JsonNumberText, [input]) => value(input) && *result == Type::String,
        (JsonFloat, [input]) => value(input) && *result == Type::Float,
        (JsonIntField, [input, Type::String]) => value(input) && option(result, &Type::Int),
        (JsonFloatField, [input, Type::String]) => value(input) && option(result, &Type::Float),
        (JsonNull, []) => value(result),
        (JsonConstruct, [Type::Bool | Type::String | Type::Int])
        | (JsonFloatValue, [Type::Float])
        | (JsonNumber, [Type::String]) => value(result),
        (JsonConstruct, [Type::List(element)]) => value(element) && value(result),
        (JsonConstruct, [Type::Map(key, element)]) => {
            **key == Type::String && value(element) && value(result)
        }
        (JsonEncode, [input]) => value(input) && matches!(result, Type::Bytes | Type::String),
        (JsonEncodeWithLimits, [input, bounds]) => {
            value(input)
                && limits(bounds, "maxOutputBytes")
                && matches!(result, Type::Bytes | Type::String)
        }
        _ => false,
    }
}

pub(crate) fn errors(op: DataOperation) -> &'static [&'static str] {
    use DataOperation::*;
    match op {
        JsonParse => &["DecodeFailed", "LimitExceeded"],
        JsonParseWithLimits => &["DecodeFailed", "LimitExceeded", "InvalidLimits"],
        JsonDecode => &["DecodeFailed", "SchemaMismatch", "LimitExceeded"],
        JsonDecodeWithLimits => &[
            "DecodeFailed",
            "SchemaMismatch",
            "LimitExceeded",
            "InvalidLimits",
        ],
        JsonField | JsonAt | JsonArrayLength | JsonObjectLength | JsonEntryAt | JsonBool
        | JsonString | JsonNumberText => &["TypeMismatch"],
        JsonInt | JsonFloat | JsonIntField | JsonFloatField => &["TypeMismatch", "NumberError"],
        JsonNumber | JsonFloatValue => &["NumberError"],
        JsonEncode => &["LimitExceeded"],
        JsonEncodeWithLimits => &["LimitExceeded", "InvalidLimits"],
        _ => &[],
    }
}
