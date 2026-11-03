//! Source-attributed Host Loop responsiveness diagnostics.

use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostDriving {
    QueuePost,
    FrameTick,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticCause {
    PendingWait,
    CpuManaged,
    BlockingNative,
    Transport,
    GcPause,
    CallbackReentry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceLocation {
    pub file: &'static str,
    pub line: u32,
    pub function: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticEvent {
    pub cause: DiagnosticCause,
    pub source: SourceLocation,
    pub observed: Duration,
    pub owner_latency: Duration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostLatencyReport {
    pub driving: HostDriving,
    pub event_source: SourceLocation,
    pub event_latency: Duration,
    pub events: Vec<DiagnosticEvent>,
}

#[derive(Debug)]
pub struct LatencyProfiler {
    driving: HostDriving,
    event_source: SourceLocation,
    events: Vec<DiagnosticEvent>,
}

impl LatencyProfiler {
    #[must_use]
    pub const fn new(driving: HostDriving, event_source: SourceLocation) -> Self {
        Self {
            driving,
            event_source,
            events: Vec::new(),
        }
    }

    /// Record elapsed Future time. A pending `await` contributes no owner
    /// latency because the Domain has returned to its scheduler.
    pub fn record_pending_wait(&mut self, source: SourceLocation, observed: Duration) {
        self.events.push(DiagnosticEvent {
            cause: DiagnosticCause::PendingWait,
            source,
            observed,
            owner_latency: Duration::ZERO,
        });
    }

    /// Measure a synchronous segment that occupies the owner thread.
    ///
    /// # Panics
    ///
    /// Panics when `PendingWait` is passed; pending waits must use
    /// [`Self::record_pending_wait`] because they do not occupy the owner.
    pub fn measure_owner_segment<F>(
        &mut self,
        cause: DiagnosticCause,
        source: SourceLocation,
        action: F,
    ) where
        F: FnOnce(),
    {
        assert_ne!(cause, DiagnosticCause::PendingWait);
        let started = Instant::now();
        action();
        let elapsed = started.elapsed();
        self.events.push(DiagnosticEvent {
            cause,
            source,
            observed: elapsed,
            owner_latency: elapsed,
        });
    }

    #[must_use]
    pub fn finish(self) -> HostLatencyReport {
        let event_latency = self.events.iter().map(|event| event.owner_latency).sum();
        HostLatencyReport {
            driving: self.driving,
            event_source: self.event_source,
            event_latency,
            events: self.events,
        }
    }
}
