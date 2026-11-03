//! Versioned, normalizable event trace used by prototype and production tests.

use core::fmt;
use std::collections::BTreeMap;
use std::io::Read;

use serde::{Deserialize, Serialize};

pub const TRACE_SCHEMA: &str = "rinzu.event-trace";
pub const TRACE_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TraceDocument {
    pub schema: String,
    pub version: u32,
    pub producer: String,
    #[serde(default)]
    pub context: BTreeMap<String, String>,
    pub events: Vec<TraceEvent>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TraceEvent {
    pub sequence: u64,
    pub domain: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceLocation>,
    #[serde(flatten)]
    pub event: EventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<Observation>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum EventKind {
    TaskStart,
    AwaitPending { operation: String },
    AwaitReady { operation: String },
    Safepoint { reason: SafepointReason },
    NativeEnter { transition: NativeTransition },
    NativeLeave { transition: NativeTransition },
    TaskComplete { outcome: TaskOutcome },
    ResourceDelta { resource: String, delta: i64 },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SafepointReason {
    Allocation,
    Call,
    LoopBackedge,
    Await,
    Explicit,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NativeTransition {
    NativeSafe,
    CallbackEntry,
    CallbackManaged,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskOutcome {
    Success,
    Failure,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceLocation {
    pub file: String,
    pub line: u32,
    pub column: u32,
}

/// Nondeterministic evidence kept for diagnostics but excluded from comparison.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monotonic_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticTrace {
    pub events: Vec<SemanticEvent>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticEvent {
    pub sequence: u64,
    pub domain: String,
    pub task: Option<String>,
    pub source: Option<SourceLocation>,
    pub event: EventKind,
}

#[derive(Debug)]
pub enum TraceError {
    Io(std::io::Error),
    Json(serde_json::Error),
    WrongSchema(String),
    UnsupportedVersion(u32),
    EmptyProducer,
    EmptyDomain {
        sequence: u64,
    },
    NonContiguousSequence {
        expected: u64,
        actual: u64,
    },
    InvalidAddress {
        sequence: u64,
        address: String,
    },
    UnknownEventField {
        sequence: Option<u64>,
        field: String,
    },
}

impl fmt::Display for TraceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "trace read failed: {error}"),
            Self::Json(error) => write!(formatter, "trace JSON is invalid: {error}"),
            Self::WrongSchema(actual) => {
                write!(formatter, "expected schema {TRACE_SCHEMA}, found {actual}")
            }
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported trace schema version {version}")
            }
            Self::EmptyProducer => write!(formatter, "trace producer must not be empty"),
            Self::EmptyDomain { sequence } => {
                write!(formatter, "event {sequence} has an empty logical domain")
            }
            Self::NonContiguousSequence { expected, actual } => write!(
                formatter,
                "trace sequence must be contiguous: expected {expected}, found {actual}"
            ),
            Self::InvalidAddress { sequence, address } => write!(
                formatter,
                "event {sequence} observation address must use 0x-prefixed hexadecimal: {address}"
            ),
            Self::UnknownEventField { sequence, field } => write!(
                formatter,
                "event {} has unknown field `{field}`",
                sequence.map_or_else(|| "?".into(), |value| value.to_string())
            ),
        }
    }
}

impl std::error::Error for TraceError {}

impl From<std::io::Error> for TraceError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for TraceError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl TraceDocument {
    pub fn from_reader(reader: impl Read) -> Result<Self, TraceError> {
        let value: serde_json::Value = serde_json::from_reader(reader)?;
        reject_unknown_event_fields(&value)?;
        let trace: Self = serde_json::from_value(value)?;
        trace.validate()?;
        Ok(trace)
    }

    pub fn validate(&self) -> Result<(), TraceError> {
        if self.schema != TRACE_SCHEMA {
            return Err(TraceError::WrongSchema(self.schema.clone()));
        }
        if self.version != TRACE_VERSION {
            return Err(TraceError::UnsupportedVersion(self.version));
        }
        if self.producer.trim().is_empty() {
            return Err(TraceError::EmptyProducer);
        }

        for (expected, event) in (0_u64..).zip(&self.events) {
            if event.sequence != expected {
                return Err(TraceError::NonContiguousSequence {
                    expected,
                    actual: event.sequence,
                });
            }
            if event.domain.trim().is_empty() {
                return Err(TraceError::EmptyDomain {
                    sequence: event.sequence,
                });
            }
            if let Some(address) = event
                .observation
                .as_ref()
                .and_then(|observation| observation.address.as_ref())
                && !(address.starts_with("0x")
                    && address.len() > 2
                    && address[2..].bytes().all(|byte| byte.is_ascii_hexdigit()))
            {
                return Err(TraceError::InvalidAddress {
                    sequence: event.sequence,
                    address: address.clone(),
                });
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn semantic(&self) -> SemanticTrace {
        SemanticTrace {
            events: self
                .events
                .iter()
                .map(|event| SemanticEvent {
                    sequence: event.sequence,
                    domain: event.domain.clone(),
                    task: event.task.clone(),
                    source: event.source.clone(),
                    event: event.event.clone(),
                })
                .collect(),
        }
    }
}

fn reject_unknown_event_fields(document: &serde_json::Value) -> Result<(), TraceError> {
    let Some(events) = document.get("events").and_then(serde_json::Value::as_array) else {
        return Ok(());
    };
    for event in events {
        let Some(fields) = event.as_object() else {
            continue;
        };
        let kind = fields
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let sequence = fields.get("sequence").and_then(serde_json::Value::as_u64);
        for field in fields.keys() {
            let common = matches!(
                field.as_str(),
                "sequence" | "domain" | "task" | "source" | "kind" | "observation"
            );
            let variant = matches!(
                (kind, field.as_str()),
                ("await-pending" | "await-ready", "operation")
                    | ("safepoint", "reason")
                    | ("native-enter" | "native-leave", "transition")
                    | ("task-complete", "outcome")
                    | ("resource-delta", "resource" | "delta")
            );
            if !common && !variant {
                return Err(TraceError::UnknownEventField {
                    sequence,
                    field: field.clone(),
                });
            }
        }
    }
    Ok(())
}
