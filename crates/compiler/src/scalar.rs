//! Closed scalar ABI for the initial macOS arm64 target.
use crate::{Diagnostic, DiagnosticCode, SourceRange};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AbiScalar {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
    Bool,
}

impl AbiScalar {
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "CInt8" => Self::I8,
            "CInt16" => Self::I16,
            "CInt" | "CInt32" => Self::I32,
            "Int" | "CInt64" => Self::I64,
            "CUInt8" => Self::U8,
            "CUInt16" => Self::U16,
            "CUInt" | "CUInt32" => Self::U32,
            "CSize" | "CUInt64" => Self::U64,
            "CFloat" => Self::F32,
            "Float" | "CDouble" => Self::F64,
            "Bool" | "CBool" => Self::Bool,
            _ => return None,
        })
    }
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::I8 => "CInt8",
            Self::I16 => "CInt16",
            Self::I32 => "CInt32",
            Self::I64 => "Int",
            Self::U8 => "CUInt8",
            Self::U16 => "CUInt16",
            Self::U32 => "CUInt32",
            Self::U64 => "CUInt64",
            Self::F32 => "CFloat",
            Self::F64 => "Float",
            Self::Bool => "Bool",
        }
    }
    #[must_use]
    pub const fn c_type(self) -> &'static str {
        match self {
            Self::I8 => "int8_t",
            Self::I16 => "int16_t",
            Self::I32 => "int32_t",
            Self::I64 => "int64_t",
            Self::U8 => "uint8_t",
            Self::U16 => "uint16_t",
            Self::U32 => "uint32_t",
            Self::U64 => "uint64_t",
            Self::F32 => "float",
            Self::F64 => "double",
            Self::Bool => "_Bool",
        }
    }
    #[must_use]
    pub const fn size(self) -> u64 {
        match self {
            Self::I8 | Self::U8 | Self::Bool => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::I64 | Self::U64 | Self::F64 => 8,
        }
    }
    #[must_use]
    pub const fn integer_bounds(self) -> Option<(i128, i128)> {
        Some(match self {
            Self::I8 => (i8::MIN as i128, i8::MAX as i128),
            Self::I16 => (i16::MIN as i128, i16::MAX as i128),
            Self::I32 => (i32::MIN as i128, i32::MAX as i128),
            Self::I64 => (i64::MIN as i128, i64::MAX as i128),
            Self::U8 => (0, u8::MAX as i128),
            Self::U16 => (0, u16::MAX as i128),
            Self::U32 => (0, u32::MAX as i128),
            Self::U64 => (0, u64::MAX as i128),
            _ => return None,
        })
    }
    pub fn check_integer(self, value: i128, range: SourceRange) -> Result<i128, Diagnostic> {
        let Some((min, max)) = self.integer_bounds() else {
            return Err(Diagnostic::error(
                DiagnosticCode::TypeMismatch,
                range,
                "integer constant requires an integer type",
                "use an integer ABI scalar",
            ));
        };
        if value < min || value > max {
            Err(Diagnostic::error(
                DiagnosticCode::IntegerOutOfRange,
                range,
                format!(
                    "integer value {value} is outside {} range [{min}, {max}]",
                    self.name()
                ),
                "use a representable value",
            ))
        } else {
            Ok(value)
        }
    }
}
