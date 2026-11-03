//! Private immutable failure packets for asynchronous Domain completion.
//! The packet uses the ordinary transport budget; it is not a source Error ABI.
use crate::failure::{Diagnostic, ErrorKind, Failure, FailureCause, SourcePosition};
use crate::scalar::{ScalarKind as K, ScalarValue};
use crate::transport::{TransferDescriptor, TransferField, TransferKind, TransferValue};

fn descriptor(kind: TransferKind) -> TransferDescriptor {
    TransferDescriptor { kind, source: None }
}
fn record(
    fields: impl IntoIterator<Item = (&'static str, TransferDescriptor)>,
) -> TransferDescriptor {
    descriptor(TransferKind::Record(
        fields
            .into_iter()
            .map(|(name, descriptor)| TransferField {
                name: name.to_owned(),
                descriptor,
            })
            .collect(),
    ))
}
fn diagnostic_descriptor() -> TransferDescriptor {
    record([
        ("cause", descriptor(TransferKind::Scalar(K::U8))),
        ("present", descriptor(TransferKind::Scalar(K::Bool))),
        ("file", descriptor(TransferKind::Scalar(K::U64))),
        ("start", descriptor(TransferKind::Scalar(K::U32))),
        ("end", descriptor(TransferKind::Scalar(K::U32))),
    ])
}
pub fn failure_descriptor() -> TransferDescriptor {
    record([
        ("primary", diagnostic_descriptor()),
        (
            "cleanup",
            descriptor(TransferKind::PersistentList(Box::new(
                diagnostic_descriptor(),
            ))),
        ),
        (
            "traces",
            descriptor(TransferKind::PersistentList(Box::new(record([
                ("diagnostic", descriptor(TransferKind::Scalar(K::U64))),
                (
                    "frames",
                    descriptor(TransferKind::PersistentList(Box::new(record([
                        ("function", descriptor(TransferKind::String)),
                        (
                            "source",
                            record([
                                ("present", descriptor(TransferKind::Scalar(K::Bool))),
                                ("file", descriptor(TransferKind::Scalar(K::U64))),
                                ("start", descriptor(TransferKind::Scalar(K::U32))),
                                ("end", descriptor(TransferKind::Scalar(K::U32))),
                            ]),
                        ),
                    ])))),
                ),
            ])))),
        ),
    ])
}
fn cause_code(cause: FailureCause) -> u64 {
    match cause {
        FailureCause::ArithmeticTrap => 1,
        FailureCause::Cancelled => 2,
        FailureCause::Overloaded => 3,
        FailureCause::Shutdown => 4,
        FailureCause::Error(ErrorKind::Conversion) => 5,
        FailureCause::Unsupported => 6,
        FailureCause::RuntimePanic => 7,
        FailureCause::Panic => 16,
        FailureCause::Error(ErrorKind::NullNativeValue) => 8,
        FailureCause::Error(ErrorKind::InvalidUtf8) => 9,
        FailureCause::Error(ErrorKind::NativeRecordBusy) => 10,
        FailureCause::Error(ErrorKind::UnobservedChildFailure) => 11,
        FailureCause::Error(ErrorKind::Io) => 12,
        FailureCause::Error(ErrorKind::SupplantBusy) => 13,
        FailureCause::Error(ErrorKind::UnboundValue) => 14,
        FailureCause::Error(ErrorKind::User) => 15,
    }
}
fn cause(code: u64) -> Option<FailureCause> {
    Some(match code {
        1 => FailureCause::ArithmeticTrap,
        2 => FailureCause::Cancelled,
        3 => FailureCause::Overloaded,
        4 => FailureCause::Shutdown,
        5 => FailureCause::Error(ErrorKind::Conversion),
        6 => FailureCause::Unsupported,
        7 => FailureCause::RuntimePanic,
        16 => FailureCause::Panic,
        8 => FailureCause::Error(ErrorKind::NullNativeValue),
        9 => FailureCause::Error(ErrorKind::InvalidUtf8),
        10 => FailureCause::Error(ErrorKind::NativeRecordBusy),
        11 => FailureCause::Error(ErrorKind::UnobservedChildFailure),
        12 => FailureCause::Error(ErrorKind::Io),
        13 => FailureCause::Error(ErrorKind::SupplantBusy),
        14 => FailureCause::Error(ErrorKind::UnboundValue),
        15 => FailureCause::Error(ErrorKind::User),
        _ => return None,
    })
}
fn scalar(kind: K, bits: u64) -> TransferValue {
    TransferValue::Scalar(ScalarValue::from_bits(kind, bits).expect("canonical diagnostic scalar"))
}
fn diagnostic_value(diagnostic: Diagnostic) -> TransferValue {
    TransferValue::Record(vec![
        scalar(K::U8, cause_code(diagnostic.cause)),
        scalar(K::Bool, u64::from(diagnostic.source.is_some())),
        scalar(K::U64, diagnostic.source.map_or(0, |source| source.file)),
        scalar(
            K::U32,
            diagnostic
                .source
                .map_or(0, |source| u64::from(source.start_byte)),
        ),
        scalar(
            K::U32,
            diagnostic
                .source
                .map_or(0, |source| u64::from(source.end_byte)),
        ),
    ])
}
pub fn failure_value(failure: &Failure) -> TransferValue {
    assert!(
        failure.managed.is_empty(),
        "managed Error cannot cross a Domain boundary"
    );
    TransferValue::Record(vec![
        diagnostic_value(failure.primary),
        TransferValue::PersistentList(
            failure
                .cleanup
                .iter()
                .copied()
                .map(diagnostic_value)
                .collect(),
        ),
        TransferValue::PersistentList(
            failure
                .traces
                .iter()
                .map(|(index, trace)| {
                    TransferValue::Record(vec![
                        scalar(K::U64, *index as u64),
                        TransferValue::PersistentList(
                            trace
                                .frames
                                .iter()
                                .map(|frame| {
                                    let TransferValue::Record(mut source) =
                                        diagnostic_value(Diagnostic {
                                            cause: FailureCause::Error(ErrorKind::User),
                                            source: frame.source,
                                        })
                                    else {
                                        unreachable!()
                                    };
                                    source.remove(0);
                                    TransferValue::Record(vec![
                                        TransferValue::String(frame.function.to_string()),
                                        TransferValue::Record(source),
                                    ])
                                })
                                .collect(),
                        ),
                    ])
                })
                .collect(),
        ),
    ])
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidFailurePacket;
impl std::fmt::Display for InvalidFailurePacket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid immutable failure packet")
    }
}
impl std::error::Error for InvalidFailurePacket {}
fn diagnostic_from_value(value: TransferValue) -> Result<Diagnostic, InvalidFailurePacket> {
    let TransferValue::Record(values) = value else {
        return Err(InvalidFailurePacket);
    };
    if values.len() != 5 {
        return Err(InvalidFailurePacket);
    }
    let mut bits = [0; 5];
    for ((slot, value), kind) in
        bits.iter_mut()
            .zip(values)
            .zip([K::U8, K::Bool, K::U64, K::U32, K::U32])
    {
        let TransferValue::Scalar(value) = value else {
            return Err(InvalidFailurePacket);
        };
        if value.kind() != kind {
            return Err(InvalidFailurePacket);
        }
        *slot = value.bits();
    }
    let cause = cause(bits[0]).ok_or(InvalidFailurePacket)?;
    let source = if bits[1] == 1 {
        if bits[3] > bits[4] {
            return Err(InvalidFailurePacket);
        }
        Some(SourcePosition {
            file: bits[2],
            start_byte: bits[3] as u32,
            end_byte: bits[4] as u32,
        })
    } else {
        if bits[2..].iter().any(|bits| *bits != 0) {
            return Err(InvalidFailurePacket);
        }
        None
    };
    Ok(Diagnostic { cause, source })
}
pub fn materialize_failure(value: TransferValue) -> Result<Failure, InvalidFailurePacket> {
    let TransferValue::Record(mut values) = value else {
        return Err(InvalidFailurePacket);
    };
    if values.len() != 3 {
        return Err(InvalidFailurePacket);
    }
    let TransferValue::PersistentList(traces) = values.pop().unwrap() else {
        return Err(InvalidFailurePacket);
    };
    let TransferValue::PersistentList(cleanup) = values.pop().unwrap() else {
        return Err(InvalidFailurePacket);
    };
    let primary = diagnostic_from_value(values.pop().unwrap())?;
    let cleanup: Vec<_> = cleanup
        .into_iter()
        .map(diagnostic_from_value)
        .collect::<Result<_, _>>()?;
    let mut metadata = std::collections::BTreeMap::new();
    for entry in traces {
        let TransferValue::Record(mut entry) = entry else {
            return Err(InvalidFailurePacket);
        };
        if entry.len() != 2 {
            return Err(InvalidFailurePacket);
        }
        let TransferValue::PersistentList(frames) = entry.pop().unwrap() else {
            return Err(InvalidFailurePacket);
        };
        let TransferValue::Scalar(index) = entry.pop().unwrap() else {
            return Err(InvalidFailurePacket);
        };
        if index.kind() != K::U64 {
            return Err(InvalidFailurePacket);
        }
        let index = usize::try_from(index.bits()).map_err(|_| InvalidFailurePacket)?;
        if index > cleanup.len()
            || metadata
                .last_key_value()
                .is_some_and(|(previous, _)| *previous >= index)
        {
            return Err(InvalidFailurePacket);
        }
        let mut captured = Vec::new();
        for frame in frames {
            let TransferValue::Record(mut frame) = frame else {
                return Err(InvalidFailurePacket);
            };
            if frame.len() != 2 {
                return Err(InvalidFailurePacket);
            }
            let TransferValue::Record(mut position) = frame.pop().unwrap() else {
                return Err(InvalidFailurePacket);
            };
            if position.len() != 4 {
                return Err(InvalidFailurePacket);
            }
            let TransferValue::String(name) = frame.pop().unwrap() else {
                return Err(InvalidFailurePacket);
            };
            position.insert(0, scalar(K::U8, 15));
            let source = diagnostic_from_value(TransferValue::Record(position))?.source;
            captured.push(crate::error_trace::TraceFrame {
                function: std::sync::Arc::from(name),
                source,
            });
        }
        metadata.insert(
            index,
            std::sync::Arc::new(crate::error_trace::ErrorTrace {
                frames: captured.into(),
            }),
        );
    }
    Ok(Failure {
        primary,
        cleanup,
        managed: Default::default(),
        traces: metadata,
    })
}
