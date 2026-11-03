//! Callback fallback constants are attached to declarations, never inferred from a function type.
use crate::ast::{BinaryOperator as B, Expression, ExpressionKind as E, Literal, UnaryOperator};
use crate::{AbiScalar, Diagnostic, DiagnosticCode, SourceRange, Type};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FallbackConstant {
    Unit,
    Integer {
        scalar: AbiScalar,
        value: i128,
    },
    Float {
        scalar: AbiScalar,
        bits: u64,
    },
    Bool(bool),
    String(String),
    /// Trailing zeros are represented by length without allocating a target-sized buffer.
    Bytes {
        prefix: Vec<u8>,
        length: u64,
    },
}

impl FallbackConstant {
    pub(crate) fn valid_for(&self, ty: &Type) -> bool {
        match self {
            Self::Unit => *ty == Type::Unit,
            Self::Bool(_) => *ty == Type::Bool,
            Self::String(_) => *ty == Type::String,
            Self::Bytes { prefix, length } => {
                *ty == Type::FixedBytes(*length) && prefix.len() as u64 <= *length
            }
            Self::Integer { scalar, value } => {
                ty.scalar() == Some(*scalar)
                    && scalar
                        .integer_bounds()
                        .is_some_and(|(min, max)| *value >= min && *value <= max)
            }
            Self::Float { scalar, bits } => {
                ty.scalar() == Some(*scalar)
                    && match scalar {
                        AbiScalar::F32 => *bits <= u64::from(u32::MAX),
                        AbiScalar::F64 => true,
                        _ => false,
                    }
            }
        }
    }
}

pub(crate) fn evaluate_fallback(
    expression: &Expression,
    result: &Type,
) -> Result<FallbackConstant, Diagnostic> {
    closed(expression)?;
    match result {
        Type::Unit if matches!(expression.kind, E::Unit) => Ok(FallbackConstant::Unit),
        Type::Bool => boolean(expression).map(FallbackConstant::Bool),
        Type::String => match &expression.kind {
            E::Literal(Literal::String(value)) => Ok(FallbackConstant::String(value.clone())),
            _ => Err(invalid(
                expression.range,
                "String fallback requires a literal",
            )),
        },
        Type::FixedBytes(length) => match &expression.kind {
            E::Literal(Literal::Bytes(prefix)) if prefix.len() as u64 <= *length => {
                Ok(FallbackConstant::Bytes {
                    prefix: prefix.clone(),
                    length: *length,
                })
            }
            _ => Err(invalid(
                expression.range,
                "Bytes fallback must be a literal fitting its result length",
            )),
        },
        _ if result.is_integer() => {
            let scalar = result.scalar().expect("integer scalar");
            let value = crate::constants::evaluate_integer_expression(
                expression,
                Some(scalar),
                &Default::default(),
            )?;
            Ok(FallbackConstant::Integer {
                scalar,
                value: value.value,
            })
        }
        _ if matches!(result.scalar(), Some(AbiScalar::F32 | AbiScalar::F64)) => {
            let scalar = result.scalar().expect("floating scalar");
            let value = floating(expression, scalar)?;
            let bits = if scalar == AbiScalar::F32 {
                u64::from((value as f32).to_bits())
            } else {
                value.to_bits()
            };
            Ok(FallbackConstant::Float { scalar, bits })
        }
        _ => Err(invalid(
            expression.range,
            "fallback result requires scalar, Unit, String or fixed Bytes ABI",
        )),
    }
}
fn closed(expression: &Expression) -> Result<(), Diagnostic> {
    match &expression.kind {
        E::Literal(_) | E::Unit => Ok(()),
        E::Unary { operand, .. } => closed(operand),
        E::Binary { left, right, .. } => {
            closed(left)?;
            closed(right)
        }
        _ => Err(invalid(
            expression.range,
            "fallback may only contain literals and supported built-in operations",
        )),
    }
}
fn floating(expression: &Expression, scalar: AbiScalar) -> Result<f64, Diagnostic> {
    let spelling = match &expression.kind {
        E::Literal(Literal::Int(value)) => Some(value.to_string()),
        E::Literal(Literal::Unsigned(value)) => Some(value.to_string()),
        E::Literal(Literal::DecimalInteger(spelling) | Literal::Float { spelling, .. }) => {
            Some(spelling.clone())
        }
        _ => None,
    };
    if let Some(spelling) = spelling {
        return if scalar == AbiScalar::F32 {
            spelling.parse::<f32>().map(f64::from)
        } else {
            spelling.parse::<f64>()
        }
        .map_err(|_| invalid(expression.range, "invalid floating fallback literal"));
    }
    match &expression.kind {
        E::Unary {
            operator: UnaryOperator::Negate,
            operand,
        } => Ok(-floating(operand, scalar)?),
        E::Binary {
            operator,
            left,
            right,
        } => {
            let left = floating(left, scalar)?;
            let right = floating(right, scalar)?;
            let value = if scalar == AbiScalar::F32 {
                let (left, right) = (left as f32, right as f32);
                f64::from(match operator {
                    B::Add => left + right,
                    B::Subtract => left - right,
                    B::Multiply => left * right,
                    B::Divide => left / right,
                    _ => {
                        return Err(invalid(
                            expression.range,
                            "unsupported floating fallback operation",
                        ));
                    }
                })
            } else {
                match operator {
                    B::Add => left + right,
                    B::Subtract => left - right,
                    B::Multiply => left * right,
                    B::Divide => left / right,
                    _ => {
                        return Err(invalid(
                            expression.range,
                            "unsupported floating fallback operation",
                        ));
                    }
                }
            };
            Ok(value)
        }
        _ => Err(invalid(
            expression.range,
            "fallback is not a floating constant",
        )),
    }
}
fn has_float(expression: &Expression) -> bool {
    match &expression.kind {
        E::Literal(Literal::Float { .. }) => true,
        E::Unary { operand, .. } => has_float(operand),
        E::Binary { left, right, .. } => has_float(left) || has_float(right),
        _ => false,
    }
}
fn boolean(expression: &Expression) -> Result<bool, Diagnostic> {
    match &expression.kind {
        E::Literal(Literal::Bool(value)) => Ok(*value),
        E::Unary {
            operator: UnaryOperator::Not,
            operand,
        } => Ok(!boolean(operand)?),
        E::Binary {
            operator: B::And,
            left,
            right,
        } => Ok(boolean(left)? && boolean(right)?),
        E::Binary {
            operator: B::Or,
            left,
            right,
        } => Ok(boolean(left)? || boolean(right)?),
        E::Binary {
            operator,
            left,
            right,
        } => {
            let compare = |left: f64, right: f64| match operator {
                B::Equal => Some(left == right),
                B::NotEqual => Some(left != right),
                B::Less => Some(left < right),
                B::LessEqual => Some(left <= right),
                B::Greater => Some(left > right),
                B::GreaterEqual => Some(left >= right),
                _ => None,
            };
            if has_float(left) || has_float(right) {
                return compare(
                    floating(left, AbiScalar::F64)?,
                    floating(right, AbiScalar::F64)?,
                )
                .ok_or_else(|| invalid(expression.range, "unsupported comparison fallback"));
            }
            // Preserve integer precision; do not compare constants through floating point.
            let integer = |value| {
                crate::constants::evaluate_integer_expression(
                    value,
                    Some(AbiScalar::I64),
                    &Default::default(),
                )
            };
            if let (Ok(left), Ok(right)) = (integer(left), integer(right)) {
                return match operator {
                    B::Equal => Ok(left.value == right.value),
                    B::NotEqual => Ok(left.value != right.value),
                    B::Less => Ok(left.value < right.value),
                    B::LessEqual => Ok(left.value <= right.value),
                    B::Greater => Ok(left.value > right.value),
                    B::GreaterEqual => Ok(left.value >= right.value),
                    _ => Err(invalid(
                        expression.range,
                        "unsupported integer comparison fallback",
                    )),
                };
            }
            if matches!(operator, B::Equal | B::NotEqual) {
                let equal = match (&left.kind, &right.kind) {
                    (E::Literal(Literal::String(a)), E::Literal(Literal::String(b))) => a == b,
                    (E::Literal(Literal::Bytes(a)), E::Literal(Literal::Bytes(b))) => a == b,
                    (E::Unit, E::Unit) => true,
                    _ => boolean(left)? == boolean(right)?,
                };
                return Ok(if *operator == B::Equal { equal } else { !equal });
            }
            Err(invalid(
                expression.range,
                "fallback is not a boolean constant",
            ))
        }
        _ => Err(invalid(
            expression.range,
            "fallback is not a boolean constant",
        )),
    }
}
fn invalid(range: SourceRange, message: &str) -> Diagnostic {
    Diagnostic::error(
        DiagnosticCode::InvalidConstant,
        range,
        message,
        "use a compile-time constant matching the synchronous callback result ABI",
    )
}
