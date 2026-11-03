//! Pure private integer semantics shared by constant evaluation and the runtime.
use crate::data_operation::DataOperation;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntegerOperationError {
    Overflow,
    InvalidShift,
    InvalidOperation,
}
pub fn integer_operation(
    op: DataOperation,
    width: u32,
    signed: bool,
    left: u64,
    right: u64,
) -> Result<u64, IntegerOperationError> {
    use DataOperation::*;
    use IntegerOperationError::*;
    if !matches!(width, 8 | 16 | 32 | 64) {
        return Err(InvalidOperation);
    }
    let mask = u64::MAX >> (64 - width);
    let raw = match op {
        BitAnd => left & right,
        BitOr => left | right,
        BitXor => left ^ right,
        BitNot => !left,
        WrappingAdd => left.wrapping_add(right),
        WrappingSub => left.wrapping_sub(right),
        WrappingMul => left.wrapping_mul(right),
        ShiftLeft | ShiftRight => {
            let count = right as i64;
            if count < 0 || count >= i64::from(width) {
                return Err(InvalidShift);
            }
            if op == ShiftLeft {
                left << count
            } else if signed {
                ((left as i64) >> count) as u64
            } else {
                left >> count
            }
        }
        CheckedAdd | CheckedSub | CheckedMul => {
            let a = if signed {
                i128::from(left as i64)
            } else {
                i128::from(left)
            };
            let b = if signed {
                i128::from(right as i64)
            } else {
                i128::from(right)
            };
            let n = match op {
                CheckedAdd => a.checked_add(b),
                CheckedSub => a.checked_sub(b),
                _ => a.checked_mul(b),
            }
            .ok_or(Overflow)?;
            let (min, max) = if signed {
                (-(1_i128 << (width - 1)), (1_i128 << (width - 1)) - 1)
            } else {
                (0, i128::from(mask))
            };
            if n < min || n > max {
                return Err(Overflow);
            }
            n as u64
        }
        _ => return Err(InvalidOperation),
    } & mask;
    Ok(
        if signed && width < 64 && raw & (1_u64 << (width - 1)) != 0 {
            raw | !mask
        } else {
            raw
        },
    )
}
