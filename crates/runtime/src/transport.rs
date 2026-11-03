//! Closed cross-Domain value algebra and exact-size transport buffers.

use crate::scalar::{ScalarKind, ScalarValue};
use std::fmt;
use std::sync::{Arc, Mutex};

pub mod typed;

const LENGTH_BYTES: usize = size_of::<u64>();
const VARIANT_BYTES: usize = size_of::<u32>();

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferSource {
    pub file: String,
    pub line: u32,
    pub column: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferDescriptor {
    pub kind: TransferKind,
    pub source: Option<TransferSource>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransferKind {
    Scalar(ScalarKind),
    Unit,
    Bool,
    Int,
    Float,
    String,
    ImmutableBytes,
    Record(Vec<TransferField>),
    ClosedEnum(Vec<TransferVariant>),
    PersistentList(Box<TransferDescriptor>),
    PersistentMap {
        key: Box<TransferDescriptor>,
        value: Box<TransferDescriptor>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferField {
    pub name: String,
    pub descriptor: TransferDescriptor,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferVariant {
    pub name: String,
    pub fields: Vec<TransferField>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TransferValue {
    Scalar(ScalarValue),
    Unit,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    ImmutableBytes(Arc<[u8]>),
    Record(Vec<TransferValue>),
    ClosedEnum {
        variant: usize,
        fields: Vec<TransferValue>,
    },
    PersistentList(Vec<TransferValue>),
    PersistentMap(Vec<(TransferValue, TransferValue)>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportDirection {
    Input,
    Result,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransportError {
    TypedValue(crate::value::ValueError),
    NonTransferable(usize),
    TypeMismatch {
        path: String,
        expected: &'static str,
    },
    UnknownVariant {
        path: String,
        variant: usize,
    },
    SizeOverflow {
        path: String,
    },
    OperationLimit {
        requested: usize,
        limit: usize,
    },
    ProcessLimit {
        requested: usize,
        used: usize,
        limit: usize,
    },
    ReservationSize {
        exact: usize,
        reserved: usize,
    },
    Truncated,
    InvalidBool(u8),
    InvalidScalarEncoding(ScalarKind),
    InvalidUtf8,
    TrailingBytes(usize),
}

impl fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TypedValue(error) => write!(formatter, "invalid typed transfer value: {error:?}"),
            Self::NonTransferable(ty) => write!(
                formatter,
                "type {ty} is outside the closed transferable algebra"
            ),
            Self::TypeMismatch { path, expected } => {
                write!(formatter, "transfer value at {path} is not {expected}")
            }
            Self::UnknownVariant { path, variant } => {
                write!(
                    formatter,
                    "transfer enum at {path} has unknown variant {variant}"
                )
            }
            Self::SizeOverflow { path } => write!(formatter, "transfer size overflows at {path}"),
            Self::OperationLimit { requested, limit } => write!(
                formatter,
                "transport operation requires {requested} bytes, limit is {limit}"
            ),
            Self::ProcessLimit {
                requested,
                used,
                limit,
            } => write!(
                formatter,
                "transport process credit exhausted: requested {requested}, used {used}, limit {limit}"
            ),
            Self::ReservationSize { exact, reserved } => write!(
                formatter,
                "transport encoding requires exactly {exact} bytes but reserved {reserved}"
            ),
            Self::Truncated => formatter.write_str("transport payload is truncated"),
            Self::InvalidBool(value) => write!(formatter, "invalid encoded boolean {value}"),
            Self::InvalidScalarEncoding(kind) => {
                write!(formatter, "invalid encoded scalar {kind:?}")
            }
            Self::InvalidUtf8 => formatter.write_str("encoded transfer string is not UTF-8"),
            Self::TrailingBytes(count) => {
                write!(formatter, "transport payload has {count} trailing bytes")
            }
        }
    }
}

impl std::error::Error for TransportError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransportBudgetSnapshot {
    pub used_bytes: usize,
    pub peak_bytes: usize,
    pub process_limit: usize,
    pub operation_limit: usize,
}

/// Opt-in conformance evidence, recorded under the same lock as byte credit.
/// Intervals describe actual reservations, including zero-byte Unit packets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransportCreditEvidence {
    pub reservation: u64,
    pub operation: Option<u64>,
    pub direction: TransportDirection,
    pub bytes: usize,
    pub reserved_at: u64,
    pub released_at: Option<u64>,
}

#[derive(Debug)]
struct BudgetState {
    used: usize,
    peak: usize,
    next_reservation: u64,
    evidence: Option<Vec<TransportCreditEvidence>>,
    sequence: u64,
}

#[derive(Debug)]
struct TransportBudgetInner {
    process_limit: usize,
    operation_limit: usize,
    state: Mutex<BudgetState>,
}

#[derive(Clone, Debug)]
pub struct TransportBudget {
    inner: Arc<TransportBudgetInner>,
}

impl TransportBudget {
    #[must_use]
    pub fn new(process_limit: usize, operation_limit: usize) -> Self {
        assert!(
            process_limit > 0,
            "transport process limit must be non-zero"
        );
        assert!(
            operation_limit > 0,
            "transport operation limit must be non-zero"
        );
        Self {
            inner: Arc::new(TransportBudgetInner {
                process_limit,
                operation_limit,
                state: Mutex::new(BudgetState {
                    used: 0,
                    peak: 0,
                    next_reservation: 1,
                    evidence: None,
                    sequence: 0,
                }),
            }),
        }
    }

    pub fn reserve(
        &self,
        direction: TransportDirection,
        exact_bytes: usize,
    ) -> Result<TransportReservation, TransportError> {
        self.reserve_for_operation(direction, exact_bytes, None)
    }

    pub fn reserve_for_operation(
        &self,
        direction: TransportDirection,
        exact_bytes: usize,
        operation: Option<u64>,
    ) -> Result<TransportReservation, TransportError> {
        if exact_bytes > self.inner.operation_limit {
            return Err(TransportError::OperationLimit {
                requested: exact_bytes,
                limit: self.inner.operation_limit,
            });
        }
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(next) = state.used.checked_add(exact_bytes) else {
            return Err(TransportError::ProcessLimit {
                requested: exact_bytes,
                used: state.used,
                limit: self.inner.process_limit,
            });
        };
        if next > self.inner.process_limit {
            return Err(TransportError::ProcessLimit {
                requested: exact_bytes,
                used: state.used,
                limit: self.inner.process_limit,
            });
        }
        let id = state.next_reservation;
        state.next_reservation = state.next_reservation.saturating_add(1);
        state.used = next;
        state.peak = state.peak.max(next);
        let evidence_index = if state.evidence.is_some() {
            state.sequence = state
                .sequence
                .checked_add(1)
                .expect("credit sequence overflow");
            let reserved_at = state.sequence;
            let evidence = state.evidence.as_mut().unwrap();
            let index = evidence.len();
            evidence.push(TransportCreditEvidence {
                reservation: id,
                operation,
                direction,
                bytes: exact_bytes,
                reserved_at,
                released_at: None,
            });
            Some(index)
        } else {
            None
        };
        drop(state);
        Ok(TransportReservation {
            budget: Arc::clone(&self.inner),
            id,
            direction,
            bytes: exact_bytes,
            released: false,
            evidence_index,
            accounting: crate::resource_accounting::Allocation::new(
                crate::resource_accounting::Kind::Transport,
                exact_bytes as u64,
            ),
        })
    }

    pub fn enable_evidence(&self) {
        let mut state = self.inner.state.lock().unwrap_or_else(|p| p.into_inner());
        assert_eq!(
            state.next_reservation, 1,
            "enable evidence before reservation"
        );
        state.evidence.get_or_insert_with(Vec::new);
    }

    #[must_use]
    pub fn evidence(&self) -> Vec<TransportCreditEvidence> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .evidence
            .clone()
            .unwrap_or_default()
    }

    #[must_use]
    pub fn snapshot(&self) -> TransportBudgetSnapshot {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        TransportBudgetSnapshot {
            used_bytes: state.used,
            peak_bytes: state.peak,
            process_limit: self.inner.process_limit,
            operation_limit: self.inner.operation_limit,
        }
    }
}

#[derive(Debug)]
pub struct TransportReservation {
    budget: Arc<TransportBudgetInner>,
    id: u64,
    direction: TransportDirection,
    bytes: usize,
    released: bool,
    evidence_index: Option<usize>,
    accounting: crate::resource_accounting::Allocation,
}

impl TransportReservation {
    pub fn set_evidence_operation(&mut self, operation: u64) {
        if let Some(index) = self.evidence_index {
            let mut state = self.budget.state.lock().unwrap_or_else(|p| p.into_inner());
            let evidence = &mut state.evidence.as_mut().unwrap()[index];
            assert!(evidence.operation.is_none(), "operation assigned once");
            evidence.operation = Some(operation);
        }
    }
    #[must_use]
    pub fn id(&self) -> u64 {
        self.id
    }

    #[must_use]
    pub fn direction(&self) -> TransportDirection {
        self.direction
    }

    #[must_use]
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    fn release(&mut self) {
        if self.released {
            return;
        }
        let mut state = self
            .budget
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.used = state
            .used
            .checked_sub(self.bytes)
            .expect("transport reservation releases exactly once");
        if let Some(index) = self.evidence_index {
            state.sequence = state
                .sequence
                .checked_add(1)
                .expect("credit sequence overflow");
            let released_at = state.sequence;
            let evidence = &mut state.evidence.as_mut().unwrap()[index];
            assert!(evidence.released_at.replace(released_at).is_none());
        }
        self.released = true;
        self.accounting.release();
    }
}

impl Drop for TransportReservation {
    fn drop(&mut self) {
        self.release();
    }
}

#[derive(Debug)]
pub struct TransportBuffer {
    bytes: Vec<u8>,
    reservation: TransportReservation,
}

impl TransportBuffer {
    pub fn set_evidence_operation(&mut self, operation: u64) {
        self.reservation.set_evidence_operation(operation);
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub fn direction(&self) -> TransportDirection {
        self.reservation.direction()
    }

    pub fn materialize(
        self,
        descriptor: &TransferDescriptor,
    ) -> Result<TransferValue, TransportError> {
        let mut cursor = Cursor::new(&self.bytes);
        let value = decode_value(descriptor, &mut cursor, "$")?;
        if cursor.remaining() != 0 {
            return Err(TransportError::TrailingBytes(cursor.remaining()));
        }
        Ok(value)
    }
}

pub fn preflight(
    descriptor: &TransferDescriptor,
    value: &TransferValue,
) -> Result<usize, TransportError> {
    preflight_value(descriptor, value, "$")
}

pub fn encode(
    descriptor: &TransferDescriptor,
    value: &TransferValue,
    reservation: TransportReservation,
) -> Result<TransportBuffer, TransportError> {
    let exact = preflight(descriptor, value)?;
    if exact != reservation.bytes() {
        return Err(TransportError::ReservationSize {
            exact,
            reserved: reservation.bytes(),
        });
    }
    let mut bytes = Vec::with_capacity(exact);
    encode_value(descriptor, value, "$", &mut bytes)?;
    debug_assert_eq!(bytes.len(), exact);
    Ok(TransportBuffer { bytes, reservation })
}

fn preflight_value(
    descriptor: &TransferDescriptor,
    value: &TransferValue,
    path: &str,
) -> Result<usize, TransportError> {
    match (&descriptor.kind, value) {
        (TransferKind::Scalar(kind), TransferValue::Scalar(value)) if *kind == value.kind() => {
            Ok(kind.width() as usize / 8)
        }
        (TransferKind::Unit, TransferValue::Unit) => Ok(0),
        (TransferKind::Bool, TransferValue::Bool(_)) => Ok(1),
        (TransferKind::Int, TransferValue::Int(_))
        | (TransferKind::Float, TransferValue::Float(_)) => Ok(8),
        (TransferKind::String, TransferValue::String(value)) => {
            add_size(LENGTH_BYTES, value.len(), path)
        }
        (TransferKind::ImmutableBytes, TransferValue::ImmutableBytes(value)) => {
            add_size(LENGTH_BYTES, value.len(), path)
        }
        (TransferKind::Record(fields), TransferValue::Record(values)) => {
            ensure_count(path, "record", fields.len(), values.len())?;
            fields
                .iter()
                .zip(values)
                .try_fold(0_usize, |size, (field, value)| {
                    let field_path = format!("{path}.{}", field.name);
                    add_size(
                        size,
                        preflight_value(&field.descriptor, value, &field_path)?,
                        &field_path,
                    )
                })
        }
        (TransferKind::ClosedEnum(variants), TransferValue::ClosedEnum { variant, fields }) => {
            let selected =
                variants
                    .get(*variant)
                    .ok_or_else(|| TransportError::UnknownVariant {
                        path: path.into(),
                        variant: *variant,
                    })?;
            ensure_count(path, "enum variant", selected.fields.len(), fields.len())?;
            selected
                .fields
                .iter()
                .zip(fields)
                .try_fold(VARIANT_BYTES, |size, (field, value)| {
                    let field_path = format!("{path}.{}.{}", selected.name, field.name);
                    add_size(
                        size,
                        preflight_value(&field.descriptor, value, &field_path)?,
                        &field_path,
                    )
                })
        }
        (TransferKind::PersistentList(element), TransferValue::PersistentList(values)) => values
            .iter()
            .enumerate()
            .try_fold(LENGTH_BYTES, |size, (index, value)| {
                let item_path = format!("{path}[{index}]");
                add_size(
                    size,
                    preflight_value(element, value, &item_path)?,
                    &item_path,
                )
            }),
        (TransferKind::PersistentMap { key, value }, TransferValue::PersistentMap(entries)) => {
            entries.iter().enumerate().try_fold(
                LENGTH_BYTES,
                |size, (index, (entry_key, entry_value))| {
                    let key_path = format!("{path}[{index}].key");
                    let value_path = format!("{path}[{index}].value");
                    let size =
                        add_size(size, preflight_value(key, entry_key, &key_path)?, &key_path)?;
                    add_size(
                        size,
                        preflight_value(value, entry_value, &value_path)?,
                        &value_path,
                    )
                },
            )
        }
        (kind, _) => Err(TransportError::TypeMismatch {
            path: path.into(),
            expected: kind.name(),
        }),
    }
}

fn encode_value(
    descriptor: &TransferDescriptor,
    value: &TransferValue,
    path: &str,
    output: &mut Vec<u8>,
) -> Result<(), TransportError> {
    preflight_value(descriptor, value, path)?;
    match (&descriptor.kind, value) {
        (TransferKind::Scalar(kind), TransferValue::Scalar(value)) => {
            output.extend(&value.bits().to_le_bytes()[..kind.width() as usize / 8])
        }
        (TransferKind::Unit, TransferValue::Unit) => {}
        (TransferKind::Bool, TransferValue::Bool(value)) => output.push(u8::from(*value)),
        (TransferKind::Int, TransferValue::Int(value)) => output.extend(value.to_le_bytes()),
        (TransferKind::Float, TransferValue::Float(value)) => {
            output.extend(value.to_bits().to_le_bytes());
        }
        (TransferKind::String, TransferValue::String(value)) => {
            encode_length(value.len(), output, path)?;
            output.extend(value.as_bytes());
        }
        (TransferKind::ImmutableBytes, TransferValue::ImmutableBytes(value)) => {
            encode_length(value.len(), output, path)?;
            output.extend(value.iter());
        }
        (TransferKind::Record(fields), TransferValue::Record(values)) => {
            for (field, value) in fields.iter().zip(values) {
                encode_value(
                    &field.descriptor,
                    value,
                    &format!("{path}.{}", field.name),
                    output,
                )?;
            }
        }
        (TransferKind::ClosedEnum(variants), TransferValue::ClosedEnum { variant, fields }) => {
            let variant_u32 = u32::try_from(*variant)
                .map_err(|_| TransportError::SizeOverflow { path: path.into() })?;
            output.extend(variant_u32.to_le_bytes());
            for (field, value) in variants[*variant].fields.iter().zip(fields) {
                encode_value(
                    &field.descriptor,
                    value,
                    &format!("{path}.{}.{}", variants[*variant].name, field.name),
                    output,
                )?;
            }
        }
        (TransferKind::PersistentList(element), TransferValue::PersistentList(values)) => {
            encode_length(values.len(), output, path)?;
            for (index, value) in values.iter().enumerate() {
                encode_value(element, value, &format!("{path}[{index}]"), output)?;
            }
        }
        (TransferKind::PersistentMap { key, value }, TransferValue::PersistentMap(entries)) => {
            encode_length(entries.len(), output, path)?;
            for (index, (entry_key, entry_value)) in entries.iter().enumerate() {
                encode_value(key, entry_key, &format!("{path}[{index}].key"), output)?;
                encode_value(
                    value,
                    entry_value,
                    &format!("{path}[{index}].value"),
                    output,
                )?;
            }
        }
        _ => unreachable!("preflight rejected mismatched transfer value"),
    }
    Ok(())
}

fn decode_value(
    descriptor: &TransferDescriptor,
    cursor: &mut Cursor<'_>,
    path: &str,
) -> Result<TransferValue, TransportError> {
    Ok(match &descriptor.kind {
        TransferKind::Scalar(kind) => {
            let width = kind.width() as usize / 8;
            let mut bytes = [0; 8];
            bytes[..width].copy_from_slice(cursor.take(width)?);
            TransferValue::Scalar(
                ScalarValue::from_physical_bits(*kind, u64::from_le_bytes(bytes))
                    .map_err(|_| TransportError::InvalidScalarEncoding(*kind))?,
            )
        }
        TransferKind::Unit => TransferValue::Unit,
        TransferKind::Bool => match cursor.take(1)?[0] {
            0 => TransferValue::Bool(false),
            1 => TransferValue::Bool(true),
            value => return Err(TransportError::InvalidBool(value)),
        },
        TransferKind::Int => TransferValue::Int(i64::from_le_bytes(cursor.array()?)),
        TransferKind::Float => {
            TransferValue::Float(f64::from_bits(u64::from_le_bytes(cursor.array()?)))
        }
        TransferKind::String => {
            let length = cursor.length()?;
            let value = std::str::from_utf8(cursor.take(length)?)
                .map_err(|_| TransportError::InvalidUtf8)?;
            TransferValue::String(value.into())
        }
        TransferKind::ImmutableBytes => {
            let length = cursor.length()?;
            TransferValue::ImmutableBytes(Arc::from(cursor.take(length)?))
        }
        TransferKind::Record(fields) => TransferValue::Record(
            fields
                .iter()
                .map(|field| {
                    decode_value(&field.descriptor, cursor, &format!("{path}.{}", field.name))
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        TransferKind::ClosedEnum(variants) => {
            let variant = usize::try_from(u32::from_le_bytes(cursor.array()?))
                .expect("u32 variant fits usize");
            let selected = variants
                .get(variant)
                .ok_or_else(|| TransportError::UnknownVariant {
                    path: path.into(),
                    variant,
                })?;
            let fields = selected
                .fields
                .iter()
                .map(|field| {
                    decode_value(
                        &field.descriptor,
                        cursor,
                        &format!("{path}.{}.{}", selected.name, field.name),
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            TransferValue::ClosedEnum { variant, fields }
        }
        TransferKind::PersistentList(element) => {
            let count = cursor.length()?;
            let mut values = Vec::with_capacity(count);
            for index in 0..count {
                values.push(decode_value(element, cursor, &format!("{path}[{index}]"))?);
            }
            TransferValue::PersistentList(values)
        }
        TransferKind::PersistentMap { key, value } => {
            let count = cursor.length()?;
            let mut entries = Vec::with_capacity(count);
            for index in 0..count {
                entries.push((
                    decode_value(key, cursor, &format!("{path}[{index}].key"))?,
                    decode_value(value, cursor, &format!("{path}[{index}].value"))?,
                ));
            }
            TransferValue::PersistentMap(entries)
        }
    })
}

impl TransferKind {
    const fn name(&self) -> &'static str {
        match self {
            Self::Scalar(_) => "Scalar",
            Self::Unit => "Unit",
            Self::Bool => "Bool",
            Self::Int => "Int",
            Self::Float => "Float",
            Self::String => "String",
            Self::ImmutableBytes => "ImmutableBytes",
            Self::Record(_) => "Record",
            Self::ClosedEnum(_) => "ClosedEnum",
            Self::PersistentList(_) => "PersistentList",
            Self::PersistentMap { .. } => "PersistentMap",
        }
    }
}

fn ensure_count(
    path: &str,
    expected: &'static str,
    expected_count: usize,
    actual_count: usize,
) -> Result<(), TransportError> {
    if expected_count == actual_count {
        Ok(())
    } else {
        Err(TransportError::TypeMismatch {
            path: path.into(),
            expected,
        })
    }
}

fn add_size(left: usize, right: usize, path: &str) -> Result<usize, TransportError> {
    left.checked_add(right)
        .ok_or_else(|| TransportError::SizeOverflow { path: path.into() })
}

fn encode_length(length: usize, output: &mut Vec<u8>, path: &str) -> Result<(), TransportError> {
    let length =
        u64::try_from(length).map_err(|_| TransportError::SizeOverflow { path: path.into() })?;
    output.extend(length.to_le_bytes());
    Ok(())
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], TransportError> {
        let end = self
            .offset
            .checked_add(count)
            .filter(|end| *end <= self.bytes.len())
            .ok_or(TransportError::Truncated)?;
        let slice = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(slice)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], TransportError> {
        self.take(N)?
            .try_into()
            .map_err(|_| TransportError::Truncated)
    }

    fn length(&mut self) -> Result<usize, TransportError> {
        usize::try_from(u64::from_le_bytes(self.array()?)).map_err(|_| TransportError::Truncated)
    }

    const fn remaining(&self) -> usize {
        self.bytes.len() - self.offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scalar(kind: TransferKind) -> TransferDescriptor {
        TransferDescriptor { kind, source: None }
    }

    fn descriptor() -> TransferDescriptor {
        TransferDescriptor {
            kind: TransferKind::Record(vec![
                TransferField {
                    name: "name".into(),
                    descriptor: scalar(TransferKind::String),
                },
                TransferField {
                    name: "payload".into(),
                    descriptor: scalar(TransferKind::ClosedEnum(vec![TransferVariant {
                        name: "items".into(),
                        fields: vec![TransferField {
                            name: "values".into(),
                            descriptor: scalar(TransferKind::PersistentList(Box::new(scalar(
                                TransferKind::Int,
                            )))),
                        }],
                    }])),
                },
            ]),
            source: Some(TransferSource {
                file: "main.rinzu".into(),
                line: 4,
                column: 11,
            }),
        }
    }

    fn value() -> TransferValue {
        TransferValue::Record(vec![
            TransferValue::String("rinzu".into()),
            TransferValue::ClosedEnum {
                variant: 0,
                fields: vec![TransferValue::PersistentList(vec![
                    TransferValue::Int(1),
                    TransferValue::Int(2),
                    TransferValue::Int(3),
                ])],
            },
        ])
    }

    #[test]
    fn exact_preflight_round_trips_and_separates_input_from_result_credit() {
        let descriptor = descriptor();
        let value = value();
        let size = preflight(&descriptor, &value).unwrap();
        let budget = TransportBudget::new(size, size);

        let input = budget.reserve(TransportDirection::Input, size).unwrap();
        let input = encode(&descriptor, &value, input).unwrap();
        assert_eq!(input.bytes().len(), size);
        assert!(matches!(
            budget.reserve(TransportDirection::Result, size),
            Err(TransportError::ProcessLimit { .. })
        ));
        assert_eq!(input.materialize(&descriptor).unwrap(), value);
        assert_eq!(budget.snapshot().used_bytes, 0);

        let result = budget.reserve(TransportDirection::Result, size).unwrap();
        let result = encode(&descriptor, &value, result).unwrap();
        assert_eq!(result.direction(), TransportDirection::Result);
        assert_eq!(result.materialize(&descriptor).unwrap(), value);
        let snapshot = budget.snapshot();
        assert_eq!(snapshot.used_bytes, 0);
        assert_eq!(snapshot.peak_bytes, size);
    }

    #[test]
    fn preflight_reports_first_incompatible_field_path() {
        let invalid = TransferValue::Record(vec![
            TransferValue::String("rinzu".into()),
            TransferValue::Bool(true),
        ]);
        assert!(matches!(
            preflight(&descriptor(), &invalid),
            Err(TransportError::TypeMismatch { path, expected: "ClosedEnum" })
                if path == "$.payload"
        ));
    }
    #[test]
    fn malformed_scalar_payload_releases_transport_credit() {
        let budget = TransportBudget::new(64, 64);
        let descriptor = scalar(TransferKind::Scalar(ScalarKind::Bool));
        let reservation = budget.reserve(TransportDirection::Input, 1).unwrap();
        let mut buffer = encode(
            &descriptor,
            &TransferValue::Scalar(ScalarValue::from_bool(true)),
            reservation,
        )
        .unwrap();
        buffer.bytes[0] = 2;
        assert_eq!(
            buffer.materialize(&descriptor),
            Err(TransportError::InvalidScalarEncoding(ScalarKind::Bool))
        );
        assert_eq!(budget.snapshot().used_bytes, 0);
        let descriptor = scalar(TransferKind::Scalar(ScalarKind::I16));
        let reservation = budget.reserve(TransportDirection::Result, 2).unwrap();
        let mut buffer = encode(
            &descriptor,
            &TransferValue::Scalar(ScalarValue::from_integer(ScalarKind::I16, -1).unwrap()),
            reservation,
        )
        .unwrap();
        buffer.bytes.pop();
        assert_eq!(
            buffer.materialize(&descriptor),
            Err(TransportError::Truncated)
        );
        assert_eq!(budget.snapshot().used_bytes, 0);
    }
}
