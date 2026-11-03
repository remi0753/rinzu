use super::*;
use crate::{
    error_value::ErrorValue,
    scalar::{ScalarKind, ScalarValue},
    value::Value,
};
use rinzu_abi::data_operation::DataOperation;

impl ManagedData {
    pub(super) fn text_operation(
        &mut self,
        heap: &mut DomainHeap,
        op: DataOperation,
        result: usize,
        types: &[usize],
        slots: &[ManagedSlot],
    ) -> Result<ManagedSlot, DataError> {
        use DataOperation::*;
        let integer = |index: usize| match slots[index] {
            ManagedSlot::Scalar(v) if v.kind() == ScalarKind::I64 => Ok(v.bits() as i64),
            _ => Err(DataError::WrongKind),
        };
        let scalar = |value: i64| {
            ManagedSlot::Scalar(ScalarValue::from_bits(ScalarKind::I64, value as u64).unwrap())
        };
        if op == FormatInt {
            return Ok(ManagedSlot::Reference(
                self.create_string(heap, &integer(0)?.to_string())?,
            ));
        }
        if op == Utf8Decode {
            let Value::Bytes(bytes) = self.snapshot(heap, types[0], slots[0])? else {
                return Err(DataError::WrongKind);
            };
            return match std::str::from_utf8(&bytes) {
                Ok(text) => Ok(ManagedSlot::Reference(self.create_string(heap, text)?)),
                Err(_) => Err(DataError::Thrown(
                    ErrorValue::new(200, "invalid UTF-8").with_context("utf8Decode".into()),
                )),
            };
        }
        let Value::String(text) = self.snapshot(heap, types[0], slots[0])? else {
            return Err(DataError::WrongKind);
        };
        if op == StringLength {
            let length = if text.is_ascii() {
                text.len()
            } else {
                text.chars().count()
            };
            return Ok(scalar(length as i64));
        }
        if op == Utf8Encode {
            return Ok(ManagedSlot::Reference(self.create_bytes(
                heap,
                text.len() as u64,
                text.as_bytes(),
            )?));
        }
        if op == ParseInt {
            let digits = text.strip_prefix(['+', '-']).unwrap_or(&text);
            let valid = !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit());
            let parsed = valid.then(|| text.parse::<i64>()).transpose();
            return match parsed {
                Ok(Some(n)) => Ok(scalar(n)),
                other => {
                    let (code, message) = if other.is_err() {
                        (202, "integer overflow")
                    } else {
                        (201, "invalid decimal integer")
                    };
                    Err(DataError::Thrown(
                        ErrorValue::new(code, message).with_context("parseInt".into()),
                    ))
                }
            };
        }
        let ascii = text.is_ascii();
        let offset = |n: i64| {
            let n = usize::try_from(n).ok()?;
            if ascii {
                (n <= text.len()).then_some(n)
            } else {
                text.char_indices()
                    .map(|(i, _)| i)
                    .chain(std::iter::once(text.len()))
                    .nth(n)
            }
        };
        let child = match op {
            StringGet => {
                let index = integer(1)?;
                match offset(index).zip(index.checked_add(1).and_then(offset)) {
                    Some((0, b)) if b == text.len() => Some(slots[0]),
                    Some((a, b)) => Some(ManagedSlot::Reference(
                        self.create_string(heap, &text[a..b])?,
                    )),
                    None => None,
                }
            }
            StringSlice => {
                let start = integer(1)?;
                let end = integer(2)?;
                match offset(start).zip(offset(end)).filter(|_| start <= end) {
                    Some((0, b)) if b == text.len() => Some(slots[0]),
                    Some((a, b)) => Some(ManagedSlot::Reference(
                        self.create_string(heap, &text[a..b])?,
                    )),
                    None => None,
                }
            }
            StringFind | StringSplit => {
                let Value::String(needle) = self.snapshot(heap, types[1], slots[1])? else {
                    return Err(DataError::WrongKind);
                };
                if op == StringFind {
                    let start = integer(2)?;
                    offset(start).and_then(|a| {
                        text[a..].find(&needle).map(|n| {
                            scalar(
                                start
                                    + if ascii {
                                        n
                                    } else {
                                        text[a..a + n].chars().count()
                                    } as i64,
                            )
                        })
                    })
                } else {
                    let mut children = Vec::new();
                    if needle.is_empty() {
                        for (a, c) in text.char_indices() {
                            children.push(ManagedSlot::Reference(
                                self.create_string(heap, &text[a..a + c.len_utf8()])?,
                            ));
                        }
                    } else {
                        for part in text.split(&needle) {
                            children.push(ManagedSlot::Reference(self.create_string(heap, part)?));
                        }
                    }
                    return Ok(ManagedSlot::Reference(
                        self.create_composite(heap, result, None, &children)?,
                    ));
                }
            }
            _ => return Err(DataError::WrongKind),
        };
        Ok(ManagedSlot::Reference(self.create_composite(
            heap,
            result,
            Some(u32::from(child.is_some())),
            child.as_slice(),
        )?))
    }
}
