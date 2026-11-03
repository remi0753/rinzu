use crate::{AbiScalar, FunctionReferenceType, Type};

pub(crate) const FUNCTIONS: [&str; 11] = [
    "utf8ByteLength",
    "byteLength",
    "readResourceSize",
    "nativeCompleted",
    "nativeTransferred",
    "futureReady",
    "cancelFuture",
    "timeoutFuture",
    "requestShutdown",
    "setProcessExitStatus",
    "panic",
];

pub(crate) fn function_name(path: &[String]) -> Option<&str> {
    let name = if path.len() == 1 {
        path[0].strip_prefix("rinzu.core::").unwrap_or(&path[0])
    } else if path.len() == 3 && path[0] == "rinzu" && path[1] == "core" {
        &path[2]
    } else {
        return None;
    };
    (FUNCTIONS.contains(&name) || crate::DataOperation::ALL.iter().any(|op| op.name() == name))
        .then_some(name)
}

pub(crate) fn data_signature(op: crate::DataOperation, input: &Type) -> Option<(Vec<Type>, Type)> {
    use crate::DataOperation::*;
    let option = |ty: &Type| Type::Named(format!("Option<{}>", ty.to_string().replace(", ", ",")));
    Some(match (op, input) {
        (Panic, Type::String) => (vec![Type::String], Type::Unit),
        (ListLength, Type::List(_)) => (vec![input.clone()], Type::Int),
        (ListGet, Type::List(element)) => (vec![input.clone(), Type::Int], option(element)),
        (ListAppend, Type::List(element)) => (vec![input.clone(), *element.clone()], input.clone()),
        (ListSet, Type::List(element)) => (
            vec![input.clone(), Type::Int, *element.clone()],
            option(input),
        ),
        (ListRemove, Type::List(_)) => (vec![input.clone(), Type::Int], option(input)),
        (MapLength, Type::Map(_, _)) => (vec![input.clone()], Type::Int),
        (MapGet, Type::Map(key, value)) => (vec![input.clone(), *key.clone()], option(value)),
        (MapInsert, Type::Map(key, value)) => (
            vec![input.clone(), *key.clone(), *value.clone()],
            input.clone(),
        ),
        (MapRemove, Type::Map(key, _)) => (vec![input.clone(), *key.clone()], input.clone()),
        (MapKeys, Type::Map(key, _)) => (vec![input.clone()], Type::List(key.clone())),
        (ByteArray, Type::FixedBytes(_)) => (vec![input.clone()], Type::Bytes),
        (ByteArray, Type::List(t)) if **t == Type::Scalar(AbiScalar::U8) => {
            (vec![input.clone()], Type::Bytes)
        }
        (BytesLength, Type::Bytes) => (vec![Type::Bytes], Type::Int),
        (BytesGet, Type::Bytes) => (
            vec![Type::Bytes, Type::Int],
            option(&Type::Scalar(AbiScalar::U8)),
        ),
        (BytesSlice, Type::Bytes) => (
            vec![Type::Bytes, Type::Int, Type::Int],
            option(&Type::Bytes),
        ),
        (BytesConcat, Type::Bytes) => (vec![Type::Bytes, Type::Bytes], Type::Bytes),
        (BytesConcatAll, Type::List(element)) if **element == Type::Bytes => {
            (vec![input.clone()], Type::Bytes)
        }
        (BytesCopy, Type::Bytes) => (vec![Type::Bytes], Type::Bytes),
        (BytesXor, Type::Bytes) => (vec![Type::Bytes, Type::Scalar(AbiScalar::U8)], Type::Bytes),
        (BytesFindAny | BytesFindNotAny, Type::Bytes) => (
            vec![Type::Bytes, Type::Bytes, Type::Int],
            option(&Type::Int),
        ),
        (BytesContainsAny | BytesAllIn, Type::Bytes) => {
            (vec![Type::Bytes, Type::Bytes], Type::Bool)
        }
        (BytesTrim, Type::Bytes) => (vec![Type::Bytes, Type::Bytes], Type::Bytes),
        (BytesLowerAscii, Type::Bytes) => (vec![Type::Bytes], Type::Bytes),
        (ErrorCreate, Type::Int) => (vec![Type::Int, Type::String], Type::Error),
        (ErrorCode, Type::Error) => (vec![Type::Error], Type::Int),
        (ErrorMessage | ErrorContext, Type::Error) => (vec![Type::Error], Type::String),
        (ErrorWithContext, Type::Error) => (vec![Type::Error, Type::String], Type::Error),
        (ErrorWithCause, Type::Error) => (vec![Type::Error, Type::Error], Type::Error),
        (ErrorCause, Type::Error) => (vec![Type::Error], option(&Type::Error)),
        (ErrorCleanup, Type::Error) => (vec![Type::Error], Type::List(Box::new(Type::Error))),
        (ErrorSourceFile, Type::Error) => {
            (vec![Type::Error], option(&Type::Scalar(AbiScalar::U64)))
        }
        (ErrorSourceStart | ErrorSourceEnd | ErrorNativeStatus, Type::Error) => {
            (vec![Type::Error], option(&Type::Int))
        }
        (NativeStatus, Type::Int) => (vec![Type::Int, Type::String], Type::Int),
        (StringLength, Type::String) => (vec![Type::String], Type::Int),
        (StringGet, Type::String) => (vec![Type::String, Type::Int], option(&Type::String)),
        (StringSlice, Type::String) => (
            vec![Type::String, Type::Int, Type::Int],
            option(&Type::String),
        ),
        (StringFind, Type::String) => (
            vec![Type::String, Type::String, Type::Int],
            option(&Type::Int),
        ),
        (StringSplit, Type::String) => (
            vec![Type::String, Type::String],
            Type::List(Box::new(Type::String)),
        ),
        (ParseInt, Type::String) => (vec![Type::String], Type::Int),
        (FormatInt, Type::Int) => (vec![Type::Int], Type::String),
        (Utf8Encode, Type::String) => (vec![Type::String], Type::Bytes),
        (Utf8Decode, Type::Bytes) => (vec![Type::Bytes], Type::String),
        (BitAnd | BitOr | BitXor | WrappingAdd | WrappingSub | WrappingMul, _)
            if input.is_integer() =>
        {
            (vec![input.clone(), input.clone()], input.clone())
        }
        (BitNot, _) if input.is_integer() => (vec![input.clone()], input.clone()),
        (ShiftLeft | ShiftRight, _) if input.is_integer() => {
            (vec![input.clone(), Type::Int], input.clone())
        }
        (CheckedAdd | CheckedSub | CheckedMul, _) if input.is_integer() => {
            (vec![input.clone(), input.clone()], input.clone())
        }
        (MathAbs | MathSqrt | FloatIsNan, _)
            if input
                .scalar()
                .is_some_and(|s| matches!(s, AbiScalar::F32 | AbiScalar::F64)) =>
        {
            (
                vec![input.clone()],
                if op == FloatIsNan {
                    Type::Bool
                } else {
                    input.clone()
                },
            )
        }
        _ => return None,
    })
}

pub(crate) fn type_ref(ty: &Type, range: crate::SourceRange) -> crate::ast::TypeRef {
    use crate::ast::{TypeRef, TypeRefKind};
    TypeRef {
        range,
        kind: match ty {
            Type::List(t) => TypeRefKind::List(Box::new(type_ref(t, range))),
            Type::Map(k, v) => {
                TypeRefKind::Map(Box::new(type_ref(k, range)), Box::new(type_ref(v, range)))
            }
            Type::Future(t) => TypeRefKind::Future(Box::new(type_ref(t, range))),
            Type::FixedBytes(n) => TypeRefKind::ConcreteBytes(*n),
            Type::FunctionReference(signature) => TypeRefKind::Function {
                supplant: signature.supplant,
                parameters: signature
                    .parameters
                    .iter()
                    .map(|p| type_ref(p, range))
                    .collect(),
                result: Box::new(type_ref(&signature.result, range)),
            },
            _ => TypeRefKind::Named(vec![ty.to_string()]),
        },
    }
}

pub(crate) fn reference_signature(name: &str) -> Option<FunctionReferenceType> {
    let parameter = if name == "rinzu.core::utf8ByteLength" {
        Type::String
    } else {
        let spelling = name.strip_prefix("rinzu.core::byteLength:")?;
        if spelling == "dynamic" {
            Type::Bytes
        } else {
            let length = spelling.parse::<u64>().ok()?;
            if spelling != length.to_string() {
                return None;
            }
            Type::FixedBytes(length)
        }
    };
    Some(FunctionReferenceType {
        parameters: vec![parameter],
        result: Box::new(Type::from_scalar(AbiScalar::U64)),
        supplant: false,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoreLengthKind {
    Utf8,
    Bytes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum TaskControlKind {
    Ready,
    Cancel,
    Timeout,
    Shutdown,
    ExitStatus,
}
impl TaskControlKind {
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "futureReady" => Self::Ready,
            "cancelFuture" => Self::Cancel,
            "timeoutFuture" => Self::Timeout,
            "requestShutdown" => Self::Shutdown,
            "setProcessExitStatus" => Self::ExitStatus,
            _ => return None,
        })
    }
}
