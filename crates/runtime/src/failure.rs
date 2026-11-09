//! Immutable diagnostics and pending failure across synchronous finally regions.
//! A catch observes only recoverable operation Errors; aggregation never changes that rule.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    User,
    Conversion,
    NullNativeValue,
    InvalidUtf8,
    NativeRecordBusy,
    SupplantBusy,
    UnboundValue,
    UnobservedChildFailure,
    Io,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureCause {
    Error(ErrorKind),
    ArithmeticTrap,
    Cancelled,
    Shutdown,
    Overloaded,
    Unsupported,
    RuntimePanic,
    Panic,
}

impl FailureCause {
    /// Encoded cause used in native diagnostics and Worker failure packets.
    pub const fn diagnostic_code(self) -> u8 {
        match self {
            Self::ArithmeticTrap => 1,
            Self::Cancelled => 2,
            Self::Overloaded => 3,
            Self::Shutdown => 4,
            Self::Error(ErrorKind::Conversion) => 5,
            Self::Unsupported => 6,
            Self::RuntimePanic => 7,
            Self::Error(ErrorKind::NullNativeValue) => 8,
            Self::Error(ErrorKind::InvalidUtf8) => 9,
            Self::Error(ErrorKind::NativeRecordBusy) => 10,
            Self::Error(ErrorKind::UnobservedChildFailure) => 11,
            Self::Error(ErrorKind::Io) => 12,
            Self::Error(ErrorKind::SupplantBusy) => 13,
            Self::Error(ErrorKind::UnboundValue) => 14,
            Self::Error(ErrorKind::User) => 15,
            Self::Panic => 16,
        }
    }

    pub const fn recoverable(self) -> bool {
        matches!(self, Self::Error(_))
    }
    pub const fn message(self) -> &'static str {
        match self {
            Self::Error(ErrorKind::User) => "user-defined Error",
            Self::Error(ErrorKind::Conversion) => {
                "numeric conversion is out of range or loses required integer precision"
            }
            Self::Error(ErrorKind::NullNativeValue) => "native value is NULL",
            Self::Error(ErrorKind::InvalidUtf8) => "native string is not valid UTF-8",
            Self::Error(ErrorKind::NativeRecordBusy) => "native record is already in use",
            Self::Error(ErrorKind::SupplantBusy) => "another supplant call is still active",
            Self::Error(ErrorKind::UnobservedChildFailure) => "child task failure was not observed",
            Self::Error(ErrorKind::UnboundValue) => "local binding has no value",
            Self::Error(ErrorKind::Io) => "I/O operation failed",
            Self::ArithmeticTrap => "invalid or overflowing integer arithmetic",
            Self::Cancelled => "task was cancelled",
            Self::Shutdown => "runtime is shutting down",
            Self::Overloaded => "runtime admission limit was reached",
            Self::Unsupported => "unsupported runtime operation",
            Self::RuntimePanic => "runtime operation panicked",
            Self::Panic => "explicit panic",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourcePosition {
    pub file: u64,
    pub start_byte: u32,
    pub end_byte: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub cause: FailureCause,
    pub source: Option<SourcePosition>,
}

impl From<FailureCause> for Diagnostic {
    fn from(cause: FailureCause) -> Self {
        Self {
            cause,
            source: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Failure {
    pub primary: Diagnostic,
    pub cleanup: Vec<Diagnostic>,
    /// Only same-Domain managed Error references. Legacy runtime diagnostics
    /// have no entry. Index zero is primary; subsequent indices are cleanup.
    pub managed: std::collections::BTreeMap<usize, crate::data::ManagedError>,
    pub traces: std::collections::BTreeMap<usize, std::sync::Arc<crate::error_trace::ErrorTrace>>,
}

impl Failure {
    pub fn new(primary: impl Into<Diagnostic>) -> Self {
        Self {
            primary: primary.into(),
            cleanup: Vec::new(),
            managed: Default::default(),
            traces: Default::default(),
        }
    }
    pub fn recoverable(&self) -> bool {
        self.primary.cause.recoverable()
            && self
                .cleanup
                .iter()
                .all(|diagnostic| diagnostic.cause.recoverable())
    }
    pub fn with_managed(primary: Diagnostic, error: crate::data::ManagedError) -> Self {
        let mut failure = Self::new(primary);
        failure.managed.insert(0, error);
        failure
    }
    pub fn diagnostics_only(&self) -> Self {
        Self {
            primary: self.primary,
            cleanup: self.cleanup.clone(),
            managed: Default::default(),
            traces: self.traces.clone(),
        }
    }
    pub fn diagnostic_at(&self, index: usize) -> Option<Self> {
        let primary = if index == 0 {
            self.primary
        } else {
            *self.cleanup.get(index - 1)?
        };
        let mut failure = Self::new(primary);
        if let Some(error) = self.managed.get(&index) {
            failure.managed.insert(0, error.clone());
        }
        if let Some(trace) = self.traces.get(&index) {
            failure.traces.insert(0, std::sync::Arc::clone(trace));
        }
        Some(failure)
    }
    /// Native completion without an active managed stack must not acquire a
    /// fictitious producer stack later, when another task awaits its result.
    pub fn mark_unrecorded_traces(&mut self) {
        for (index, diagnostic) in std::iter::once(&self.primary)
            .chain(&self.cleanup)
            .enumerate()
        {
            if diagnostic.cause.recoverable() {
                self.traces.entry(index).or_insert_with(|| {
                    std::sync::Arc::new(crate::error_trace::ErrorTrace {
                        frames: std::sync::Arc::from([]),
                    })
                });
            }
        }
    }
    pub fn without_primary(mut self) -> Option<Self> {
        if self.cleanup.is_empty() {
            return None;
        }
        self.primary = self.cleanup.remove(0);
        self.managed = self
            .managed
            .into_iter()
            .filter_map(|(index, value)| index.checked_sub(1).map(|index| (index, value)))
            .collect();
        self.traces = self
            .traces
            .into_iter()
            .filter_map(|(index, value)| index.checked_sub(1).map(|index| (index, value)))
            .collect();
        Some(self)
    }
    pub fn append_cleanup(&mut self, cleanup: Self) {
        let offset = self.cleanup.len() + 1;
        self.traces.extend(
            cleanup
                .traces
                .into_iter()
                .map(|(index, value)| (index + offset, value)),
        );
        self.managed.extend(
            cleanup
                .managed
                .into_iter()
                .map(|(index, value)| (index + offset, value)),
        );
        self.cleanup.push(cleanup.primary);
        self.cleanup.extend(cleanup.cleanup);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureStateError {
    UnbalancedCleanup,
    ActiveCleanup,
}

/// Each finally temporarily suspends the incoming failure. New failures and local catches
/// operate on the active region, then its result is merged with the suspended primary.
#[derive(Debug, Default)]
pub struct PendingFailure {
    current: Option<Failure>,
    suspended: Vec<CleanupFrame>,
    tracer: crate::error_trace::TraceContext,
}

#[derive(Debug)]
struct CleanupFrame {
    incoming: Option<Failure>,
    owner: Option<(u64, u64)>,
}

impl PendingFailure {
    pub fn set_tracer(&mut self, tracer: crate::error_trace::TraceContext) {
        self.tracer = tracer;
    }
    pub fn tracer(&self) -> &crate::error_trace::TraceContext {
        &self.tracer
    }
    pub fn current(&self) -> Option<&Failure> {
        self.current.as_ref()
    }
    /// Incoming failure of the active cleanup, before its body runs.
    pub fn cleanup_incoming(&self) -> Option<&Failure> {
        self.suspended
            .last()
            .and_then(|frame| frame.incoming.as_ref())
    }
    pub fn replace_cleanup_incoming(
        &mut self,
        failure: Option<Failure>,
    ) -> Result<Option<Failure>, FailureStateError> {
        let frame = self
            .suspended
            .last_mut()
            .ok_or(FailureStateError::UnbalancedCleanup)?;
        Ok(std::mem::replace(&mut frame.incoming, failure))
    }
    pub fn settle_task_join(
        &mut self,
        activation: u64,
        region: u64,
        failure: Option<Failure>,
    ) -> Result<(), FailureStateError> {
        let current = self
            .suspended
            .len()
            .checked_sub(1)
            .ok_or(FailureStateError::UnbalancedCleanup)?;
        let outer = self
            .suspended
            .iter()
            .rposition(|frame| frame.owner == Some((activation, region)));
        if let Some(outer) = outer.filter(|outer| *outer != current) {
            self.suspended[current].incoming = None;
            let incoming = self.suspended[outer].incoming.take();
            self.suspended[outer].incoming = match (incoming, failure) {
                (Some(mut primary), Some(cleanup)) => {
                    primary.append_cleanup(cleanup);
                    Some(primary)
                }
                (Some(primary), None) => Some(primary),
                (None, failure) => failure,
            };
        } else {
            self.suspended[current].incoming = failure;
        }
        Ok(())
    }
    pub fn cleanup_depth(&self) -> usize {
        self.suspended.len()
    }
    pub fn fail(&mut self, diagnostic: impl Into<Diagnostic>) {
        if self.current.is_none() {
            self.propagate(Failure::new(diagnostic));
        }
    }
    /// Adopt an already immutable failure from a callee or completed Future.
    /// Propagation preserves its producing source and ordered cleanup causes.
    pub fn propagate(&mut self, mut failure: Failure) {
        if self.current.is_some() {
            return;
        }
        if failure.primary.cause.recoverable()
            && !failure.managed.contains_key(&0)
            && !failure.traces.contains_key(&0)
            && let Some(trace) = self.tracer.capture(failure.primary.source)
        {
            failure.traces.insert(0, trace);
        }
        self.current.get_or_insert(failure);
    }
    pub fn begin_cleanup(&mut self) {
        self.suspended.push(CleanupFrame {
            incoming: self.current.take(),
            owner: None,
        });
    }
    pub fn begin_cleanup_scoped(&mut self, activation: u64, region: u64) {
        self.suspended.push(CleanupFrame {
            incoming: self.current.take(),
            owner: Some((activation, region)),
        });
    }
    /// Error routes can revisit lexical scopes whose cleanup was already
    /// settled. Never pop a different region or a caller's suspended primary.
    pub fn end_cleanup_scoped(
        &mut self,
        activation: u64,
        region: u64,
    ) -> Result<bool, FailureStateError> {
        let owner = Some((activation, region));
        if self
            .suspended
            .last()
            .is_some_and(|frame| frame.owner == owner)
        {
            self.end_cleanup()?;
            return Ok(true);
        }
        if self.suspended.iter().any(|frame| frame.owner == owner) {
            return Err(FailureStateError::UnbalancedCleanup);
        }
        Ok(false)
    }
    pub fn end_cleanup(&mut self) -> Result<(), FailureStateError> {
        let incoming = self
            .suspended
            .pop()
            .ok_or(FailureStateError::UnbalancedCleanup)?
            .incoming;
        self.current = match (incoming, self.current.take()) {
            (Some(mut primary), Some(cleanup)) => {
                primary.append_cleanup(cleanup);
                Some(primary)
            }
            (Some(primary), None) => Some(primary),
            (None, cleanup) => cleanup,
        };
        Ok(())
    }
    pub fn observe_error(&mut self) -> Option<Failure> {
        if self.current.as_ref().is_some_and(Failure::recoverable) {
            self.current.take()
        } else {
            None
        }
    }
    pub fn finish(self) -> Result<Option<Failure>, FailureStateError> {
        if self.suspended.is_empty() {
            Ok(self.current)
        } else {
            Err(FailureStateError::ActiveCleanup)
        }
    }
}
