use rinzu_compiler::AbiScalar;
use rinzu_runtime::scalar::ScalarKind;

pub(crate) const fn kind(scalar: AbiScalar) -> ScalarKind {
    match scalar {
        AbiScalar::I8 => ScalarKind::I8,
        AbiScalar::I16 => ScalarKind::I16,
        AbiScalar::I32 => ScalarKind::I32,
        AbiScalar::I64 => ScalarKind::I64,
        AbiScalar::U8 => ScalarKind::U8,
        AbiScalar::U16 => ScalarKind::U16,
        AbiScalar::U32 => ScalarKind::U32,
        AbiScalar::U64 => ScalarKind::U64,
        AbiScalar::F32 => ScalarKind::F32,
        AbiScalar::F64 => ScalarKind::F64,
        AbiScalar::Bool => ScalarKind::Bool,
    }
}
