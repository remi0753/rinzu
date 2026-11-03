//! Closed scalar value algebra. Canonical bits preserve integer width and floating precision.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u32)]
pub enum ScalarKind {
    I8 = 0,
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

impl ScalarKind {
    pub const fn from_tag(tag: u32) -> Option<Self> {
        Some(match tag {
            0 => Self::I8,
            1 => Self::I16,
            2 => Self::I32,
            3 => Self::I64,
            4 => Self::U8,
            5 => Self::U16,
            6 => Self::U32,
            7 => Self::U64,
            8 => Self::F32,
            9 => Self::F64,
            10 => Self::Bool,
            _ => return None,
        })
    }
    pub const fn width(self) -> u32 {
        match self {
            Self::I8 | Self::U8 | Self::Bool => 8,
            Self::I16 | Self::U16 => 16,
            Self::I32 | Self::U32 | Self::F32 => 32,
            Self::I64 | Self::U64 | Self::F64 => 64,
        }
    }
    pub const fn signed(self) -> bool {
        matches!(self, Self::I8 | Self::I16 | Self::I32 | Self::I64)
    }
    pub const fn integer(self) -> bool {
        matches!(
            self,
            Self::I8
                | Self::I16
                | Self::I32
                | Self::I64
                | Self::U8
                | Self::U16
                | Self::U32
                | Self::U64
        )
    }
    pub const fn bounds(self) -> Option<(i128, i128)> {
        if !self.integer() {
            return None;
        }
        let width = self.width();
        Some(if self.signed() {
            (-(1_i128 << (width - 1)), (1_i128 << (width - 1)) - 1)
        } else {
            (0, (1_i128 << width) - 1)
        })
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScalarError {
    InvalidEncoding,
    TypeMismatch,
    Arithmetic,
    Conversion,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Arithmetic {
    Negate = 0,
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Comparison {
    Equal = 0,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

#[derive(Clone, Copy, Debug)]
pub struct ScalarValue {
    kind: ScalarKind,
    bits: u64,
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ScalarKey {
    kind: ScalarKind,
    bits: u64,
}

impl ScalarKey {
    #[must_use]
    pub fn stable_hash(self) -> u64 {
        let mut hash = crate::key::KeyHasher::default();
        // ScalarKey can only be obtained from an already validated integer.
        hash.integer(ScalarValue {
            kind: self.kind,
            bits: self.bits,
        })
        .expect("ScalarKey is an integer");
        hash.finish()
    }
}

impl ScalarValue {
    pub fn from_bits(kind: ScalarKind, bits: u64) -> Result<Self, ScalarError> {
        let valid = if let Some((minimum, maximum)) = kind.bounds() {
            let integer = if kind.signed() {
                i128::from(bits as i64)
            } else {
                i128::from(bits)
            };
            minimum <= integer && integer <= maximum
        } else {
            match kind {
                ScalarKind::F32 => bits <= u64::from(u32::MAX),
                ScalarKind::F64 => true,
                ScalarKind::Bool => bits <= 1,
                _ => unreachable!(),
            }
        };
        if valid {
            Ok(Self { kind, bits })
        } else {
            Err(ScalarError::InvalidEncoding)
        }
    }
    /// Read a physical scalar's low-width bits, canonicalizing signed narrow values.
    pub fn from_physical_bits(kind: ScalarKind, bits: u64) -> Result<Self, ScalarError> {
        let width = kind.width();
        let bits = if width == 64 {
            bits
        } else if kind.signed() {
            (((bits << (64 - width)) as i64) >> (64 - width)) as u64
        } else {
            bits & ((1_u64 << width) - 1)
        };
        Self::from_bits(kind, bits)
    }
    pub fn from_integer(kind: ScalarKind, value: i128) -> Result<Self, ScalarError> {
        let (minimum, maximum) = kind.bounds().ok_or(ScalarError::Conversion)?;
        if value < minimum || value > maximum {
            return Err(ScalarError::Conversion);
        }
        Ok(Self {
            kind,
            bits: value as u64,
        })
    }
    pub const fn from_f32(value: f32) -> Self {
        Self {
            kind: ScalarKind::F32,
            bits: value.to_bits() as u64,
        }
    }
    pub const fn from_f64(value: f64) -> Self {
        Self {
            kind: ScalarKind::F64,
            bits: value.to_bits(),
        }
    }
    pub const fn from_bool(value: bool) -> Self {
        Self {
            kind: ScalarKind::Bool,
            bits: value as u64,
        }
    }
    pub const fn kind(self) -> ScalarKind {
        self.kind
    }
    pub const fn bits(self) -> u64 {
        self.bits
    }
    pub fn integer(self) -> Option<i128> {
        self.kind.integer().then(|| {
            if self.kind.signed() {
                i128::from(self.bits as i64)
            } else {
                i128::from(self.bits)
            }
        })
    }
    pub fn float(self) -> Option<f64> {
        match self.kind {
            ScalarKind::F32 => Some(f64::from(f32::from_bits(self.bits as u32))),
            ScalarKind::F64 => Some(f64::from_bits(self.bits)),
            _ => None,
        }
    }
    pub fn key(self) -> Option<ScalarKey> {
        self.kind.integer().then_some(ScalarKey {
            kind: self.kind,
            bits: self.bits,
        })
    }
    pub fn compare(self, operator: Comparison, right: Self) -> Result<bool, ScalarError> {
        if self.kind != right.kind {
            return Err(ScalarError::TypeMismatch);
        }
        macro_rules! compare {
            ($left:expr, $right:expr) => {
                match operator {
                    Comparison::Equal => $left == $right,
                    Comparison::NotEqual => $left != $right,
                    Comparison::Less => $left < $right,
                    Comparison::LessEqual => $left <= $right,
                    Comparison::Greater => $left > $right,
                    Comparison::GreaterEqual => $left >= $right,
                }
            };
        }
        Ok(
            if let (Some(left), Some(right)) = (self.integer(), right.integer()) {
                compare!(left, right)
            } else if let (Some(left), Some(right)) = (self.float(), right.float()) {
                compare!(left, right)
            } else if matches!(operator, Comparison::Equal | Comparison::NotEqual) {
                compare!(self.bits, right.bits)
            } else {
                return Err(ScalarError::TypeMismatch);
            },
        )
    }
    pub fn arithmetic(self, operator: Arithmetic, right: Self) -> Result<Self, ScalarError> {
        if self.kind != right.kind {
            return Err(ScalarError::TypeMismatch);
        }
        if let (Some(left), Some(right)) = (self.integer(), right.integer()) {
            if operator == Arithmetic::Negate && !self.kind.signed() {
                return Err(ScalarError::TypeMismatch);
            }
            if matches!(operator, Arithmetic::Divide | Arithmetic::Remainder)
                && (right == 0
                    || (self.kind.signed() && left == self.kind.bounds().unwrap().0 && right == -1))
            {
                return Err(ScalarError::Arithmetic);
            }
            let value = match operator {
                Arithmetic::Negate => left.checked_neg(),
                Arithmetic::Add => left.checked_add(right),
                Arithmetic::Subtract => left.checked_sub(right),
                Arithmetic::Multiply => left.checked_mul(right),
                Arithmetic::Divide => left.checked_div(right),
                Arithmetic::Remainder => left.checked_rem(right),
            }
            .ok_or(ScalarError::Arithmetic)?;
            return Self::from_integer(self.kind, value).map_err(|_| ScalarError::Arithmetic);
        }
        macro_rules! arithmetic {
            ($left:expr, $right:expr) => {
                match operator {
                    Arithmetic::Negate => -$left,
                    Arithmetic::Add => $left + $right,
                    Arithmetic::Subtract => $left - $right,
                    Arithmetic::Multiply => $left * $right,
                    Arithmetic::Divide => $left / $right,
                    Arithmetic::Remainder => $left % $right,
                }
            };
        }
        if operator == Arithmetic::Remainder {
            return Err(ScalarError::TypeMismatch);
        }
        match self.kind {
            ScalarKind::F32 => Ok(Self::from_f32(arithmetic!(
                f32::from_bits(self.bits as u32),
                f32::from_bits(right.bits as u32)
            ))),
            ScalarKind::F64 => Ok(Self::from_f64(arithmetic!(
                f64::from_bits(self.bits),
                f64::from_bits(right.bits)
            ))),
            _ => Err(ScalarError::TypeMismatch),
        }
    }
    pub fn convert(self, target: ScalarKind) -> Result<Self, ScalarError> {
        if self.kind == ScalarKind::Bool || target == ScalarKind::Bool {
            return Err(ScalarError::Conversion);
        }
        if self.kind == target {
            return Ok(self);
        }
        if target.integer() {
            let value = if let Some(value) = self.integer() {
                value
            } else {
                let value = self.float().ok_or(ScalarError::Conversion)?;
                let (minimum, _) = target.bounds().unwrap();
                let upper = if target.signed() {
                    2_f64.powi(target.width() as i32 - 1)
                } else {
                    2_f64.powi(target.width() as i32)
                };
                if !value.is_finite()
                    || value.fract() != 0.0
                    || value < minimum as f64
                    || value >= upper
                {
                    return Err(ScalarError::Conversion);
                }
                value as i128
            };
            return Self::from_integer(target, value);
        }
        match target {
            ScalarKind::F32 => {
                let value = if let Some(integer) = self.integer() {
                    integer as f32
                } else {
                    let source = self.float().ok_or(ScalarError::Conversion)?;
                    let converted = source as f32;
                    if source.is_finite() && converted.is_infinite() {
                        return Err(ScalarError::Conversion);
                    }
                    converted
                };
                Ok(Self::from_f32(value))
            }
            ScalarKind::F64 => Ok(Self::from_f64(if let Some(integer) = self.integer() {
                integer as f64
            } else {
                self.float().ok_or(ScalarError::Conversion)?
            })),
            _ => Err(ScalarError::Conversion),
        }
    }
}
impl PartialEq for ScalarValue {
    fn eq(&self, right: &Self) -> bool {
        self.compare(Comparison::Equal, *right).unwrap_or(false)
    }
}
