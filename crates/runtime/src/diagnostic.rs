//! Runtime diagnostics expressed as logical managed/native frames.

use std::thread::ThreadId;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceLocation {
    pub file: String,
    pub line: u32,
    pub column: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogicalFrameKind {
    Managed,
    Native,
    OwnerSync,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogicalFrame {
    pub kind: LogicalFrameKind,
    pub label: String,
    pub source: Option<SourceLocation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeDiagnostic {
    pub code: &'static str,
    pub message: String,
    pub frames: Vec<LogicalFrame>,
    pub expected_thread: Option<ThreadId>,
    pub actual_thread: Option<ThreadId>,
}

impl RuntimeDiagnostic {
    #[must_use]
    pub fn host_callback_failure(label: impl Into<String>, source: Option<SourceLocation>) -> Self {
        Self {
            code: "RZRT1002",
            message: "managed callback panicked; the failure was contained at the native boundary"
                .into(),
            frames: vec![
                LogicalFrame {
                    kind: LogicalFrameKind::Managed,
                    label: label.into(),
                    source: source.clone(),
                },
                LogicalFrame {
                    kind: LogicalFrameKind::Native,
                    label: "HostLoopProfileV1 owner turn".into(),
                    source: None,
                },
                LogicalFrame {
                    kind: LogicalFrameKind::OwnerSync,
                    label: "generated callback trampoline".into(),
                    source,
                },
            ],
            expected_thread: None,
            actual_thread: None,
        }
    }

    #[must_use]
    pub fn wrong_thread(
        operation: impl Into<String>,
        expected: ThreadId,
        actual: ThreadId,
    ) -> Self {
        Self {
            code: "RZRT1001",
            message: format!(
                "{} requires the process-main Host owner thread",
                operation.into()
            ),
            frames: vec![LogicalFrame {
                kind: LogicalFrameKind::Native,
                label: "HostLoopProfileV1 owner entry".into(),
                source: None,
            }],
            expected_thread: Some(expected),
            actual_thread: Some(actual),
        }
    }
}
