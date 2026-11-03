//! Indexed typed transport: no heap identity or native address enters a packet.
use super::*;
use crate::value::{Value, ValueError, ValueSchema, ValueType};
use std::collections::HashSet;

impl From<ValueError> for TransportError {
    fn from(error: ValueError) -> Self {
        Self::TypedValue(error)
    }
}

#[derive(Clone, Debug)]
pub struct Descriptor {
    schema: Arc<ValueSchema>,
    roots: Vec<usize>,
}
impl Descriptor {
    pub fn new(schema: Arc<ValueSchema>, roots: Vec<usize>) -> Result<Self, TransportError> {
        let mut done = HashSet::new();
        let mut active = HashSet::new();
        let mut work = roots
            .iter()
            .rev()
            .map(|ty| (*ty, false))
            .collect::<Vec<_>>();
        while let Some((index, finish)) = work.pop() {
            if finish {
                active.remove(&index);
                done.insert(index);
                continue;
            }
            if done.contains(&index) {
                continue;
            }
            if !active.insert(index) {
                return Err(TransportError::NonTransferable(index));
            }
            work.push((index, true));
            let children = match schema.ty(index)? {
                ValueType::Scalar(_)
                | ValueType::Unit
                | ValueType::String
                | ValueType::Bytes(_)
                | ValueType::DynamicBytes
                | ValueType::Error => Vec::new(),
                ValueType::Record { fields, .. } => fields.clone(),
                ValueType::Enum { variants, .. } => variants.iter().flatten().copied().collect(),
                ValueType::List(element) => vec![*element],
                ValueType::Map { key, value } => vec![*key, *value],
                ValueType::Other | ValueType::State { .. } | ValueType::ErrorRecord { .. } => {
                    return Err(TransportError::NonTransferable(index));
                }
            };
            work.extend(children.into_iter().rev().map(|index| (index, false)));
        }
        Ok(Self { schema, roots })
    }

    pub fn validate(&self, values: &[Value]) -> Result<(), TransportError> {
        ensure_count("$", "typed arguments", self.roots.len(), values.len())?;
        for (ty, value) in self.roots.iter().zip(values) {
            self.schema.validate(*ty, value)?;
        }
        Ok(())
    }

    fn walk<'a>(
        &self,
        values: &'a [Value],
        mut visit: impl FnMut(&ValueType, &'a Value) -> Result<(), TransportError>,
    ) -> Result<(), TransportError> {
        let mut work = self
            .roots
            .iter()
            .copied()
            .zip(values)
            .rev()
            .collect::<Vec<_>>();
        while let Some((index, value)) = work.pop() {
            let ty = self.schema.ty(index)?;
            visit(ty, value)?;
            match (ty, value) {
                (ValueType::Record { fields: types, .. }, Value::Record { fields, .. }) => {
                    work.extend(types.iter().copied().zip(fields).rev())
                }
                (
                    ValueType::Enum { variants, .. },
                    Value::Enum {
                        variant, fields, ..
                    },
                ) => work.extend(
                    variants[*variant as usize]
                        .iter()
                        .copied()
                        .zip(fields)
                        .rev(),
                ),
                (ValueType::List(element), Value::List(values)) => {
                    work.extend(values.iter().rev().map(|value| (*element, value)))
                }
                (ValueType::Map { key, value }, Value::Map(entries)) => {
                    for (k, v) in entries.iter().rev() {
                        work.push((*value, v));
                        work.push((*key, k));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn preflight(&self, values: &[Value]) -> Result<usize, TransportError> {
        self.validate(values)?;
        let mut total = 0;
        self.walk(values, |ty, value| {
            let size = match (ty, value) {
                (ValueType::Scalar(kind), _) => kind.width() as usize / 8,
                (ValueType::Error, Value::Error(error)) => add_size(8, error.encode().len(), "$")?,
                (ValueType::Unit, _) => 0,
                (ValueType::String, Value::String(text)) => add_size(8, text.len(), "$")?,
                (ValueType::Bytes(_) | ValueType::DynamicBytes, Value::Bytes(bytes)) => {
                    add_size(8, bytes.len(), "$")?
                }
                (ValueType::Record { identity, .. }, _) => add_size(8, identity.len(), "$")?,
                (ValueType::Enum { identity, .. }, _) => add_size(12, identity.len(), "$")?,
                (ValueType::List(_) | ValueType::Map { .. }, _) => 8,
                _ => unreachable!("validated typed value"),
            };
            total = add_size(total, size, "$")?;
            Ok(())
        })?;
        Ok(total)
    }

    pub fn encode(
        &self,
        values: &[Value],
        reservation: TransportReservation,
    ) -> Result<TransportBuffer, TransportError> {
        let exact = self.preflight(values)?;
        if reservation.bytes() != exact {
            return Err(TransportError::ReservationSize {
                exact,
                reserved: reservation.bytes(),
            });
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(exact)
            .unwrap_or_else(|_| std::process::abort());
        self.walk(values, |ty, value| {
            match (ty, value) {
                (ValueType::Scalar(kind), Value::Scalar(value)) => bytes
                    .extend_from_slice(&value.bits().to_le_bytes()[..kind.width() as usize / 8]),
                (ValueType::Unit, Value::Unit) => {}
                (ValueType::Error, Value::Error(error)) => {
                    let encoded = error.encode();
                    encode_length(encoded.len(), &mut bytes, "$")?;
                    bytes.extend_from_slice(&encoded);
                }
                (ValueType::String, Value::String(text)) => {
                    encode_length(text.len(), &mut bytes, "$")?;
                    bytes.extend_from_slice(text.as_bytes());
                }
                (ValueType::Bytes(_) | ValueType::DynamicBytes, Value::Bytes(value)) => {
                    encode_length(value.len(), &mut bytes, "$")?;
                    bytes.extend_from_slice(value);
                }
                (ValueType::Record { identity, .. }, _) | (ValueType::Enum { identity, .. }, _) => {
                    encode_length(identity.len(), &mut bytes, "$")?;
                    bytes.extend_from_slice(identity.as_bytes());
                    if let Value::Enum { variant, .. } = value {
                        bytes.extend_from_slice(&variant.to_le_bytes());
                    }
                }
                (ValueType::List(_), Value::List(values)) => {
                    encode_length(values.len(), &mut bytes, "$")?
                }
                (ValueType::Map { .. }, Value::Map(values)) => {
                    encode_length(values.len(), &mut bytes, "$")?
                }
                _ => unreachable!("validated typed value"),
            }
            Ok(())
        })?;
        debug_assert_eq!(bytes.len(), exact);
        Ok(TransportBuffer { bytes, reservation })
    }

    pub fn decode(&self, bytes: &[u8]) -> Result<Vec<Value>, TransportError> {
        enum Work {
            Visit(usize),
            Repeat {
                remaining: usize,
                first: usize,
                second: Option<usize>,
            },
            Build {
                ty: usize,
                variant: Option<u32>,
                count: usize,
            },
        }
        let mut cursor = Cursor::new(bytes);
        let mut work = self
            .roots
            .iter()
            .rev()
            .map(|ty| Work::Visit(*ty))
            .collect::<Vec<_>>();
        let mut output = Vec::new();
        while let Some(item) = work.pop() {
            match item {
                Work::Repeat {
                    remaining,
                    first,
                    second,
                } => {
                    if remaining != 0 {
                        work.push(Work::Repeat {
                            remaining: remaining - 1,
                            first,
                            second,
                        });
                        if let Some(second) = second {
                            work.push(Work::Visit(second));
                        }
                        work.push(Work::Visit(first));
                    }
                }
                Work::Visit(index) => match self.schema.ty(index)? {
                    ValueType::Scalar(kind) => {
                        let width = kind.width() as usize / 8;
                        let mut bits = [0; 8];
                        bits[..width].copy_from_slice(cursor.take(width)?);
                        output.push(Value::Scalar(
                            ScalarValue::from_physical_bits(*kind, u64::from_le_bytes(bits))
                                .map_err(|_| TransportError::InvalidScalarEncoding(*kind))?,
                        ));
                    }
                    ValueType::Unit => output.push(Value::Unit),
                    ValueType::String => {
                        let count = cursor.length()?;
                        let text = std::str::from_utf8(cursor.take(count)?)
                            .map_err(|_| TransportError::InvalidUtf8)?;
                        output.push(Value::String(text.to_owned()));
                    }
                    ValueType::Error => {
                        let count = cursor.length()?;
                        output.push(Value::Error(
                            crate::error_value::ErrorValue::decode(cursor.take(count)?)
                                .ok_or(ValueError::TypeMismatch)?,
                        ));
                    }
                    ValueType::DynamicBytes => {
                        let count = cursor.length()?;
                        if count > i64::MAX as usize {
                            return Err(ValueError::TypeMismatch.into());
                        }
                        output.push(Value::Bytes(cursor.take(count)?.to_vec()));
                    }
                    ValueType::Bytes(expected) => {
                        let count = cursor.length()?;
                        if count as u64 != *expected {
                            return Err(ValueError::TypeMismatch.into());
                        }
                        output.push(Value::Bytes(cursor.take(count)?.to_vec()));
                    }
                    ValueType::Record { identity, fields } => {
                        nominal(&mut cursor, identity)?;
                        work.push(Work::Build {
                            ty: index,
                            variant: None,
                            count: fields.len(),
                        });
                        work.extend(fields.iter().rev().map(|ty| Work::Visit(*ty)));
                    }
                    ValueType::Enum { identity, variants } => {
                        nominal(&mut cursor, identity)?;
                        let variant = u32::from_le_bytes(cursor.array()?);
                        let fields = variants.get(variant as usize).ok_or_else(|| {
                            TransportError::UnknownVariant {
                                path: "$".into(),
                                variant: variant as usize,
                            }
                        })?;
                        work.push(Work::Build {
                            ty: index,
                            variant: Some(variant),
                            count: fields.len(),
                        });
                        work.extend(fields.iter().rev().map(|ty| Work::Visit(*ty)));
                    }
                    ValueType::List(element) => {
                        let count = cursor.length()?;
                        work.push(Work::Build {
                            ty: index,
                            variant: None,
                            count,
                        });
                        work.push(Work::Repeat {
                            remaining: count,
                            first: *element,
                            second: None,
                        });
                    }
                    ValueType::Map { key, value } => {
                        let count = cursor
                            .length()?
                            .checked_mul(2)
                            .ok_or(TransportError::Truncated)?;
                        work.push(Work::Build {
                            ty: index,
                            variant: None,
                            count,
                        });
                        work.push(Work::Repeat {
                            remaining: count / 2,
                            first: *key,
                            second: Some(*value),
                        });
                    }
                    ValueType::Other | ValueType::State { .. } | ValueType::ErrorRecord { .. } => {
                        unreachable!("closed transferable schema")
                    }
                },
                Work::Build { ty, variant, count } => {
                    let start = output
                        .len()
                        .checked_sub(count)
                        .ok_or(TransportError::Truncated)?;
                    let fields = output.split_off(start);
                    output.push(match self.schema.ty(ty)? {
                        ValueType::Record { identity, .. } => Value::Record {
                            identity: identity.clone(),
                            fields,
                        },
                        ValueType::Enum { identity, .. } => Value::Enum {
                            identity: identity.clone(),
                            variant: variant.expect("enum tag"),
                            fields,
                        },
                        ValueType::List(_) => Value::List(fields),
                        ValueType::Map { .. } => {
                            let mut pairs = Vec::with_capacity(count / 2);
                            let mut fields = fields.into_iter();
                            while let Some(key) = fields.next() {
                                pairs.push((key, fields.next().expect("pair count")));
                            }
                            Value::Map(pairs)
                        }
                        _ => unreachable!("composite build"),
                    });
                }
            }
        }
        if cursor.remaining() != 0 {
            return Err(TransportError::TrailingBytes(cursor.remaining()));
        }
        self.validate(&output)?;
        Ok(output)
    }
}

fn nominal(cursor: &mut Cursor<'_>, expected: &str) -> Result<(), TransportError> {
    let count = cursor.length()?;
    if cursor.take(count)? != expected.as_bytes() {
        return Err(ValueError::TypeMismatch.into());
    }
    Ok(())
}

impl TransportBuffer {
    /// Borrowed decode lets the receiver retain credit until its heap copy and roots are ready.
    pub fn decode_typed(&self, descriptor: &Descriptor) -> Result<Vec<Value>, TransportError> {
        descriptor.decode(&self.bytes)
    }
    pub fn materialize_typed(self, descriptor: &Descriptor) -> Result<Vec<Value>, TransportError> {
        descriptor.decode(&self.bytes)
    }
}
