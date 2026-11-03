//! Standalone root lifetime and explicit-exit prototype.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleState {
    Running,
    ShutdownUnwind,
    RootClosing,
    Terminated,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExitInitiator {
    Task(u64),
    HostLoopReturn,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExitRequest {
    Latched,
    AlreadyLatched,
    RejectedOwnerSync,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdmissionError {
    Closed,
    Limit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LifecycleSnapshot {
    pub state: LifecycleState,
    pub initiator: Option<ExitInitiator>,
    pub admission_open: bool,
    pub general_slots_used: usize,
    pub cleanup_pending: bool,
    pub cleanup_completions: usize,
    pub shutdown_completions: usize,
    pub late_callback_releases: usize,
    pub self_join_attempts: usize,
}

#[derive(Debug)]
pub struct StandaloneLifecycle {
    state: LifecycleState,
    initiator: Option<ExitInitiator>,
    admission_open: bool,
    general_capacity: usize,
    general_slots_used: usize,
    cleanup_pending: bool,
    cleanup_completions: usize,
    shutdown_completions: usize,
    late_callback_releases: usize,
    self_join_attempts: usize,
}

impl StandaloneLifecycle {
    #[must_use]
    pub const fn new(general_capacity: usize) -> Self {
        Self {
            state: LifecycleState::Running,
            initiator: None,
            admission_open: true,
            general_capacity,
            general_slots_used: 0,
            cleanup_pending: false,
            cleanup_completions: 0,
            shutdown_completions: 0,
            late_callback_releases: 0,
            self_join_attempts: 0,
        }
    }

    /// Admit ordinary work before shutdown.
    ///
    /// # Errors
    ///
    /// Returns `Closed` after an exit is latched and `Limit` at capacity.
    pub fn admit(&mut self) -> Result<(), AdmissionError> {
        if !self.admission_open {
            return Err(AdmissionError::Closed);
        }
        if self.general_slots_used == self.general_capacity {
            return Err(AdmissionError::Limit);
        }
        self.general_slots_used += 1;
        Ok(())
    }

    pub fn request_exit(
        &mut self,
        initiator: ExitInitiator,
        inside_owner_sync: bool,
    ) -> ExitRequest {
        if inside_owner_sync {
            return ExitRequest::RejectedOwnerSync;
        }
        if self.initiator.is_some() {
            return ExitRequest::AlreadyLatched;
        }
        self.initiator = Some(initiator);
        self.admission_open = false;
        self.state = LifecycleState::ShutdownUnwind;
        ExitRequest::Latched
    }

    /// Use the runtime-reserved cleanup slot while the initiating task's
    /// `finally` unwinds. It is independent of ordinary admission capacity.
    pub fn begin_cleanup_safe(&mut self) -> bool {
        if self.state != LifecycleState::ShutdownUnwind || self.cleanup_pending {
            return false;
        }
        self.cleanup_pending = true;
        true
    }

    pub fn complete_cleanup_safe(&mut self) -> bool {
        if !self.cleanup_pending {
            return false;
        }
        self.cleanup_pending = false;
        self.cleanup_completions += 1;
        true
    }

    /// Terminally settle the original initiator after all pending cleanup.
    pub fn settle_initiator(&mut self) -> bool {
        if self.state != LifecycleState::ShutdownUnwind || self.cleanup_pending {
            return false;
        }
        self.state = LifecycleState::RootClosing;
        true
    }

    /// Complete work that was admitted before the exit latch.
    pub fn complete_admitted_during_shutdown(&mut self) -> bool {
        if self.general_slots_used == 0 {
            return false;
        }
        self.general_slots_used -= 1;
        self.shutdown_completions += 1;
        true
    }

    /// A native callback arriving after close releases only its native result.
    pub fn release_late_callback(&mut self) {
        self.late_callback_releases += 1;
    }

    /// Finish root close without ever joining the task currently executing
    /// the close protocol.
    pub fn finish_root_close(&mut self, current_task: Option<u64>) -> bool {
        if self.state != LifecycleState::RootClosing || self.general_slots_used != 0 {
            return false;
        }
        if let (Some(ExitInitiator::Task(initiator)), Some(current)) =
            (self.initiator, current_task)
        {
            // The initiator has already reached terminal settlement.  Equal
            // IDs are therefore excluded from a join list, not joined.
            if initiator != current {
                // A different coordinator could join it; this model records
                // only forbidden self-join attempts.
            }
        }
        self.state = LifecycleState::Terminated;
        true
    }

    #[must_use]
    pub const fn snapshot(&self) -> LifecycleSnapshot {
        LifecycleSnapshot {
            state: self.state,
            initiator: self.initiator,
            admission_open: self.admission_open,
            general_slots_used: self.general_slots_used,
            cleanup_pending: self.cleanup_pending,
            cleanup_completions: self.cleanup_completions,
            shutdown_completions: self.shutdown_completions,
            late_callback_releases: self.late_callback_releases,
            self_join_attempts: self.self_join_attempts,
        }
    }
}
