use super::*;
use crate::{
    error_value::ErrorValue,
    scalar::{ScalarKind, ScalarValue},
};
use rinzu_abi::{
    data_operation::DataOperation,
    integer_operation::{IntegerOperationError, integer_operation},
};
impl ManagedData {
    pub(super) fn numeric_operation(
        &mut self,
        _heap: &mut DomainHeap,
        op: DataOperation,
        _result: usize,
        slots: &[ManagedSlot],
    ) -> Result<ManagedSlot, DataError> {
        use DataOperation::*;
        let ManagedSlot::Scalar(input) = slots[0] else {
            return Err(DataError::WrongKind);
        };
        if matches!(op, MathAbs | MathSqrt | FloatIsNan) {
            let value = match input.kind() {
                ScalarKind::F32 => {
                    let n = f32::from_bits(input.bits() as u32);
                    if op == FloatIsNan {
                        ScalarValue::from_bool(n.is_nan())
                    } else {
                        ScalarValue::from_f32(if op == MathAbs { n.abs() } else { n.sqrt() })
                    }
                }
                ScalarKind::F64 => {
                    let n = f64::from_bits(input.bits());
                    if op == FloatIsNan {
                        ScalarValue::from_bool(n.is_nan())
                    } else {
                        ScalarValue::from_f64(if op == MathAbs { n.abs() } else { n.sqrt() })
                    }
                }
                _ => return Err(DataError::WrongKind),
            };
            return Ok(ManagedSlot::Scalar(value));
        }
        if !input.kind().integer() {
            return Err(DataError::WrongKind);
        }
        let right = if slots.len() > 1 {
            let ManagedSlot::Scalar(v) = slots[1] else {
                return Err(DataError::WrongKind);
            };
            v.bits()
        } else {
            0
        };
        let outcome = integer_operation(
            op,
            input.kind().width(),
            input.kind().signed(),
            input.bits(),
            right,
        );
        match outcome {
            Ok(bits) => Ok(ManagedSlot::Scalar(
                ScalarValue::from_bits(input.kind(), bits)
                    .map_err(|_| DataError::InvalidEncoding)?,
            )),
            Err(error) => {
                let (code, message) = match error {
                    IntegerOperationError::Overflow => (300, "integer overflow"),
                    IntegerOperationError::InvalidShift => (301, "invalid shift count"),
                    _ => return Err(DataError::InvalidEncoding),
                };
                Err(DataError::Thrown(
                    ErrorValue::new(code, message).with_context(op.name().into()),
                ))
            }
        }
    }
}
