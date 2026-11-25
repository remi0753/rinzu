use super::search::{Protocol, Violation, require};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) enum Stage {
    Prepared,
    Entered,
    Returned,
    Closed,
    Settled,
    Read,
    Abandoned,
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum Callback {
    Unused,
    Queued,
    Executing,
    Returned,
    Settled,
}
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum CallFault {
    #[default]
    None,
    CancelReleasesEnvironment,
    ReleaseWithCallback,
    LedgerDropsAliasRoot,
    FreeLiveCapability,
}
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Independent lifetime and ownership axes"
)]
pub struct Call {
    pub(super) stage: Stage,
    pub(super) callbacks: Vec<Callback>,
    closed_consumer: bool,
    pub(super) cancelled: bool,
    pub(super) environment: bool,
    pub(super) continuation: bool,
    capability: bool,
    control: bool,
    late_probe: u8,
    alias: bool,
    ledger: bool,
    result_root: bool,
    exported: bool,
    captured_value: bool,
    borrower: bool,
    handle_open: bool,
    parent_finally: bool,
    pub fault: CallFault,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Enter,
    Cancel,
    Abandon,
    Admit(usize),
    Begin(usize),
    ReturnCallback(usize),
    SettleCallback(usize),
    ReturnNative,
    CloseGate,
    SettleGate,
    ReadResult,
    FinishContinuation,
    ReleaseEnvironment,
    RetireLedger,
    DropAlias,
    ExportResult,
    DropResult,
    CollectValue,
    RevokeCapability,
    BeginLateProbe,
    EndLateProbe,
    FreeControl,
    ReleaseBorrower,
    CloseHandle,
    ParentFinally,
}
impl Call {
    /// # Panics
    /// Panics unless the callback bound is one or two.
    #[must_use]
    pub fn new(callbacks: usize, closed_consumer: bool) -> Self {
        assert!((1..=2).contains(&callbacks));
        Self {
            stage: Stage::Prepared,
            callbacks: vec![Callback::Unused; callbacks],
            closed_consumer,
            cancelled: false,
            environment: true,
            continuation: true,
            capability: false,
            control: true,
            late_probe: 0,
            alias: true,
            ledger: true,
            result_root: false,
            exported: false,
            captured_value: true,
            borrower: true,
            handle_open: true,
            parent_finally: false,
            fault: CallFault::None,
        }
    }
    fn callbacks_settled(&self) -> bool {
        self.callbacks
            .iter()
            .all(|c| matches!(c, Callback::Unused | Callback::Settled))
    }
    fn physically_safe(&self) -> bool {
        matches!(self.stage, Stage::Read | Stage::Abandoned)
            && self.callbacks_settled()
            && !self.continuation
    }
    fn roots(&self) -> bool {
        self.environment || self.alias || self.ledger || self.result_root
    }
}
impl Protocol for Call {
    type Action = Action;
    fn name(&self) -> String {
        format!(
            "call-callbacks{}-closed-consumer{}",
            self.callbacks.len(),
            self.closed_consumer
        )
    }
    fn actions(&self) -> Vec<Action> {
        use Action as A;
        let mut a = Vec::new();
        if !self.cancelled {
            a.push(A::Cancel);
        }
        if !self.parent_finally {
            a.push(A::ParentFinally);
        }
        match self.stage {
            Stage::Prepared => {
                a.push(A::Enter);
                if self.cancelled {
                    a.push(A::Abandon);
                }
            }
            Stage::Entered => a.push(A::ReturnNative),
            Stage::Returned => a.push(A::CloseGate),
            Stage::Closed if self.callbacks_settled() => a.push(A::SettleGate),
            Stage::Settled => a.push(A::ReadResult),
            Stage::Read if self.continuation => a.push(A::FinishContinuation),
            _ => {}
        }
        for (i, c) in self.callbacks.iter().enumerate() {
            match c {
                Callback::Unused if self.stage == Stage::Entered => a.push(A::Admit(i)),
                Callback::Queued => a.push(A::Begin(i)),
                Callback::Executing => a.push(A::ReturnCallback(i)),
                Callback::Returned => a.push(A::SettleCallback(i)),
                _ => {}
            }
        }
        if self.environment
            && (self.physically_safe()
                || (self.fault == CallFault::ReleaseWithCallback
                    && self.stage == Stage::Closed
                    && !self.callbacks_settled()))
        {
            a.push(A::ReleaseEnvironment);
        }
        if self.ledger && matches!(self.stage, Stage::Read | Stage::Abandoned) {
            a.push(A::RetireLedger);
        }
        if self.alias {
            a.push(A::DropAlias);
        }
        if !self.exported && self.stage == Stage::Read && self.alias {
            a.push(A::ExportResult);
        }
        if self.result_root {
            a.push(A::DropResult);
        }
        if self.captured_value && !self.roots() {
            a.push(A::CollectValue);
        }
        if self.stage >= Stage::Closed && self.stage != Stage::Abandoned {
            if self.capability && self.late_probe == 0 {
                a.push(A::BeginLateProbe);
            }
            if self.late_probe == 1 {
                a.push(A::EndLateProbe);
            }
            if self.capability && self.closed_consumer && self.late_probe != 1 {
                a.push(A::RevokeCapability);
            }
        }
        if self.control
            && !self.environment
            && self.late_probe != 1
            && (!self.capability || self.fault == CallFault::FreeLiveCapability)
        {
            a.push(A::FreeControl);
        }
        if self.borrower && self.physically_safe() {
            a.push(A::ReleaseBorrower);
        }
        if self.handle_open && !self.borrower {
            a.push(A::CloseHandle);
        }
        a
    }
    fn apply(&mut self, a: Action) {
        use Action as A;
        match a {
            A::Enter => {
                self.stage = Stage::Entered;
                self.capability = true;
            }
            A::Cancel => {
                self.cancelled = true;
                if self.fault == CallFault::CancelReleasesEnvironment {
                    self.environment = false;
                }
            }
            A::Abandon => {
                self.stage = Stage::Abandoned;
                self.continuation = false;
            }
            A::Admit(i) => self.callbacks[i] = Callback::Queued,
            A::Begin(i) => self.callbacks[i] = Callback::Executing,
            A::ReturnCallback(i) => self.callbacks[i] = Callback::Returned,
            A::SettleCallback(i) => self.callbacks[i] = Callback::Settled,
            A::ReturnNative => self.stage = Stage::Returned,
            A::CloseGate => self.stage = Stage::Closed,
            A::SettleGate => self.stage = Stage::Settled,
            A::ReadResult => self.stage = Stage::Read,
            A::FinishContinuation => self.continuation = false,
            A::ReleaseEnvironment => self.environment = false,
            A::RetireLedger => {
                self.ledger = false;
                if self.fault == CallFault::LedgerDropsAliasRoot {
                    self.captured_value = false;
                }
            }
            A::DropAlias => self.alias = false,
            A::ExportResult => {
                self.result_root = true;
                self.exported = true;
            }
            A::DropResult => self.result_root = false,
            A::CollectValue => self.captured_value = false,
            A::RevokeCapability => self.capability = false,
            A::BeginLateProbe => self.late_probe = 1,
            A::EndLateProbe => self.late_probe = 2,
            A::FreeControl => self.control = false,
            A::ReleaseBorrower => self.borrower = false,
            A::CloseHandle => self.handle_open = false,
            A::ParentFinally => self.parent_finally = true,
        }
    }
    fn check_step(&self, a: Action, after: &Self) -> Result<(), Violation> {
        if matches!(a, Action::Cancel | Action::ParentFinally) {
            let mut physical_after = after.clone();
            physical_after.cancelled = self.cancelled;
            physical_after.parent_finally = self.parent_finally;
            require(
                self == &physical_after,
                "NL-I3",
                "logical control or parent finally fabricated physical completion or release",
            )?;
        }
        Ok(())
    }
    fn check(&self) -> Result<(), Violation> {
        if !self.environment {
            require(
                self.physically_safe(),
                "NL-I5",
                "native/callback/continuation users outlived their environment",
            )?;
        }
        require(
            self.control || (!self.capability && self.late_probe != 1),
            "NL-I5",
            "callback rejection metadata freed with a usable capability",
        )?;
        require(
            !self.roots() || self.captured_value,
            "NL-I8",
            "retiring one ledger lost an independent alias/result root",
        )?;
        require(
            self.handle_open || (!self.borrower && self.physically_safe()),
            "NL-I8",
            "user handle closed while a native or managed borrower remained",
        )?;
        Ok(())
    }
    fn terminal(&self) -> bool {
        !self.environment
            && !self.captured_value
            && !self.handle_open
            && (!self.control || (!self.closed_consumer && self.capability && self.late_probe == 2))
    }
    fn witnesses(&self) -> Vec<&'static str> {
        let mut w = Vec::new();
        if self.cancelled && self.stage == Stage::Entered && self.environment {
            w.push("cancelled-call-still-entered");
        }
        if self.stage >= Stage::Closed
            && self.stage != Stage::Abandoned
            && !self.callbacks_settled()
        {
            w.push("closed-gate-with-admitted-callback");
        }
        if !self.environment && self.control && self.capability {
            w.push("environment-freed-rejection-control-retained");
        }
        if !self.environment && self.result_root && self.captured_value {
            w.push("escaped-result-outlives-call-environment");
        }
        if !self.ledger && self.alias && self.captured_value {
            w.push("future-alias-outlives-parent-ledger");
        }
        w
    }
}
