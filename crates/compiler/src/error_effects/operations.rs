//! Normal Error sources in primitive calls. Allocation/invariant faults and
//! cancellation are not catchable Error effects. Normal absence uses Option.
use super::*;

pub(super) fn scalar_constructor(
    checked: &CheckedModule,
    expression: &Expression,
    target: &[String],
) -> Option<crate::AbiScalar> {
    if !matches!(
        expression.kind,
        ExpressionKind::Call { .. } | ExpressionKind::GenericCall { spawn: false, .. }
    ) || target.len() != 1
        || checked.indirect_calls.contains_key(&expression.range)
    {
        return None;
    }
    crate::AbiScalar::from_name(&target[0]).filter(|scalar| *scalar != crate::AbiScalar::Bool)
}

impl flow::Analyzer<'_> {
    pub(super) fn primitive_errors(
        &self,
        expression: &Expression,
        target: &[String],
        arguments: &Arguments,
        name: &str,
    ) -> Language {
        if let Some(operation) = self.checked.data_operations.get(&expression.range) {
            if self
                .checked
                .in_bounds_collection_updates
                .contains(&expression.range)
            {
                return Language::new();
            }
            return data_errors(self.checked, *operation);
        }
        if self.checked.stdout_calls.contains_key(&expression.range) {
            return language::one(Tag::Any);
        }
        if scalar_constructor(self.checked, expression, target).is_some() {
            return if matches!(arguments,Arguments::Positional(values) if values.first().and_then(|value|self.checked.expression_type(value.range)) == self.checked.expression_type(expression.range))
            {
                Language::new()
            } else {
                language::one(Tag::Any)
            };
        }
        if let Some(signature) = self
            .checked
            .source_native_calls
            .get(&expression.range)
            .or_else(|| {
                self.checked
                    .source_native_signatures
                    .iter()
                    .find(|signature| signature.name == name)
            })
        {
            return native_errors(signature);
        }
        if self.checked.native_calls.contains_key(&expression.range) {
            // Generated binding metadata does not declare a normal Error
            // contract. Do not use may_fail (which also includes traps) as a
            // proof that an external implementation cannot raise an Error.
            return language::unknown(self.bound);
        }
        if self.checked.supplant_calls.contains_key(&expression.range) {
            return language::one(Tag::Any);
        }
        // Lengths, readiness, cancel/timeout/shutdown and exit-status validation
        // do not throw a normal Error. In particular an invalid exit status
        // returns false.
        Language::new()
    }
}

fn native_errors(signature: &crate::SourceNativeSignature) -> Language {
    use crate::SourceAbiType as A;
    // Service input validation happens at the call position. A service's later
    // failure is separately retained by the Future value's unknown effect.
    // Supplant reservation can raise SupplantBusy before physical entry.
    let error = signature.is_async
        || signature.is_supplant
        || signature
            .parameters
            .iter()
            .any(|ty| matches!(ty, A::Record(_)))
        || match &signature.result {
            A::String
            | A::Bytes(_)
            | A::ByteArrayResult
            | A::Opaque(_)
            | A::Record(_)
            | A::UserData(_) => true,
            A::Unit | A::Scalar(_) => false,
            // Invalid ABI shapes must not become an infallibility proof even
            // if a caller constructs incomplete internal metadata.
            A::ByteArray | A::Callback { .. } => true,
        };
    if error {
        language::one(Tag::Any)
    } else {
        Language::new()
    }
}

fn data_errors(checked: &CheckedModule, operation: crate::DataOperation) -> Language {
    use crate::DataOperation::*;
    // Exhaustive on purpose: adding an intrinsic requires reviewing its Error
    // contract here. Invalid collection indices return Option; overflow in ordinary
    // arithmetic remains a trap.
    match operation {
        ErrorWithCause | NativeStatus | ParseInt | Utf8Decode | ShiftLeft | ShiftRight
        | CheckedAdd | CheckedSub | CheckedMul => language::one(Tag::Any),
        ListAppend | ListConcatAll | MapInsert => {
            language::one(Tag::Named(operation.length_overflow().unwrap().into()))
        }
        ListLength | ListGet | ListSet | ListRemove | MapLength | MapGet | MapRemove | MapKeys
        | MapEntries | ListBufferReserve | ListBufferAppend | MapBufferIndex | MapBufferInsert
        | CollectionFinish | ByteArray | BytesLength | BytesGet | BytesSlice | BytesConcat
        | BytesConcatAll | BytesCopy | BytesXor | BytesFindAny | BytesFindNotAny
        | BytesLowerAscii | BytesContainsAny | BytesAllIn | BytesTrim | ErrorCreate | ErrorCode
        | ErrorMessage | ErrorContext | ErrorWithContext | ErrorCause | ErrorCleanup
        | ErrorSourceFile | ErrorSourceStart | ErrorSourceEnd | ErrorNativeStatus
        | StringLength | StringGet | StringSlice | StringFind | StringSplit | FormatInt
        | Utf8Encode | BitAnd | BitOr | BitXor | BitNot | WrappingAdd | WrappingSub
        | WrappingMul | MathAbs | MathSqrt | FloatIsNan | Panic | ProcessArguments => {
            Language::new()
        }
        JsonParse | JsonParseWithLimits | JsonDecode | JsonDecodeWithLimits | JsonKind
        | JsonField | JsonAt | JsonArrayLength | JsonObjectLength | JsonEntryAt | JsonBool
        | JsonString | JsonNumberText | JsonInt | JsonFloat | JsonIntField | JsonFloatField
        | JsonNull | JsonConstruct | JsonNumber | JsonFloatValue | JsonEncode
        | JsonEncodeWithLimits => {
            let mut result = Language::new();
            for name in crate::json::errors(operation) {
                let qualified = format!("rinzu.json::{name}");
                let identity = if checked.source_types.contains_key(&qualified) {
                    qualified
                } else {
                    (*name).into()
                };
                result.extend(language::one(Tag::Named(identity)));
            }
            result
        }
    }
}
