use super::search::{Violation, require};
use super::wake::Action as W;
use super::{Completion, CompletionAction as C, Decision, Protocol, ReceiptStage, Wake};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Binding {
    CDirect,
    CJoined,
    RustChannel,
}
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ComposedFault {
    #[default]
    None,
    DetachAfterOneCopy,
    FreeEnvironmentOnCancel,
    FreeWithProducerReference,
    IgnoreRuntimeReference,
    ReleaseDuringWorkerWake,
}
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Composed {
    pub(super) completion: Completion,
    pub(super) wake: Wake,
    pub(super) binding: Binding,
    pub(super) count: usize,
    claim_attempted: Vec<bool>,
    control_kind: Vec<Decision>,
    control_phase: Vec<u8>,
    control_used: Vec<u8>,
    pub(super) shutdown: bool,
    pub(super) runtime_reference: bool,
    pub(super) environment: bool,
    pub(super) worker_active: Option<usize>,
    pub(super) woke: Vec<bool>,
    pub fault: ComposedFault,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComposedAction {
    Admit(usize),
    Write,
    Prefix,
    BeginCopy(usize),
    Copy(usize),
    Claim(usize),
    Physical(usize),
    Publish,
    CloseAdmission,
    RetireCarrier,
    BeginControl(usize, Decision),
    ResolveControl(usize),
    EndControl(usize, bool),
    Settle(usize),
    Release(usize),
    Shutdown,
    WorkerWakeBegin(usize),
    WorkerWakeEnd(usize),
    Quiescent,
    Join,
    NotifyBegin,
    NotifyEnd,
    ProducerRelease,
    Detach,
    RuntimeReferenceRelease,
    EnvironmentFree,
    TargetFree,
}
impl Composed {
    /// Start after one successful notification registration, before publication.
    /// # Panics
    /// Panics unless the total receipt bound is one or two.
    #[must_use]
    pub fn new(count: usize, binding: Binding) -> Self {
        let mut state = Self::before_registration(count, binding, 3);
        state.completion.apply(C::Admit(0));
        state.wake.producer_reference = true;
        state
    }
    pub(super) fn before_registration(count: usize, binding: Binding, version: u8) -> Self {
        assert!((1..=2).contains(&count));
        let completion = Completion::new(version, u8::try_from(count).unwrap(), 1, false, true);
        let mut wake = if binding == Binding::RustChannel {
            Wake::channel()
        } else {
            Wake::new()
        };
        if binding == Binding::CDirect {
            wake.helper = 3;
        }
        wake.producer_reference = false;
        Self {
            completion,
            wake,
            binding,
            count,
            claim_attempted: vec![false; count],
            control_kind: vec![Decision::Pending; count],
            control_phase: vec![0; count],
            control_used: vec![0; count],
            shutdown: false,
            runtime_reference: true,
            environment: true,
            worker_active: None,
            woke: vec![false; count],
            fault: ComposedFault::None,
        }
    }
    fn all_admitted(&self) -> bool {
        (0..self.count).all(|i| self.completion.stage(i) != ReceiptStage::Absent)
    }
    fn all_physical(&self) -> bool {
        (0..self.count).all(|i| self.completion.stage(i) >= ReceiptStage::Acquired)
    }
    pub(super) fn enabled(&self, a: ComposedAction) -> bool {
        self.actions().contains(&a)
    }
    pub(super) fn control_result(&self, i: usize) -> Option<bool> {
        match self.control_phase[i] {
            2 => Some(true),
            3 => Some(false),
            _ => None,
        }
    }
    pub(super) fn claim_result(&self, i: usize) -> Option<bool> {
        self.claim_attempted[i].then(|| self.completion.decision(i) == Decision::Completed)
    }
    pub(super) fn control_in_progress(&self) -> bool {
        self.control_phase.iter().any(|p| *p != 0)
    }
    fn receipt_active(&self, i: usize) -> bool {
        self.completion.stage(i) >= ReceiptStage::Waiting
            && self.completion.stage(i) < ReceiptStage::Released
    }
    fn completion_actions(&self) -> Vec<ComposedAction> {
        use ComposedAction as A;
        let mut a = Vec::new();
        for c in self.completion.actions() {
            match c {
                C::Admit(i) if !self.shutdown => a.push(A::Admit(i)),
                C::Write => a.push(A::Write),
                C::PrefixDone => a.push(A::Prefix),
                C::Publish => a.push(A::Publish),
                C::CloseAdmission => a.push(A::CloseAdmission),
                C::RetireCarrier => a.push(A::RetireCarrier),
                C::BeginCopy(i) => a.push(A::BeginCopy(i)),
                C::Settle(i) => a.push(A::Settle(i)),
                C::Release(i)
                    if self.control_phase[i] == 0
                        && (self.binding != Binding::RustChannel
                            || self.woke[i]
                            || self.fault == ComposedFault::ReleaseDuringWorkerWake) =>
                {
                    a.push(A::Release(i));
                }
                _ => {}
            }
        }
        a
    }
}
impl Protocol for Composed {
    type Action = ComposedAction;
    fn name(&self) -> String {
        format!("completion-wake-{:?}-w{}", self.binding, self.count)
    }
    fn actions(&self) -> Vec<Self::Action> {
        use ComposedAction as A;
        let mut a = self.completion_actions();
        for i in 0..self.count {
            if self.completion.stage(i) == ReceiptStage::Copying {
                if !self.completion.copied(i) {
                    a.push(A::Copy(i));
                } else if !self.claim_attempted[i] {
                    a.push(A::Claim(i));
                } else {
                    a.push(A::Physical(i));
                }
            }
            if self.receipt_active(i) && self.control_phase[i] == 0 {
                for d in [Decision::Cancelled, Decision::TimedOut] {
                    if self.control_used[i] == 0 {
                        a.push(A::BeginControl(i, d));
                    }
                }
            }
            match self.control_phase[i] {
                1 => a.push(A::ResolveControl(i)),
                2 => a.push(A::EndControl(i, true)),
                3 => a.push(A::EndControl(i, false)),
                _ => {}
            }
            if self.binding == Binding::RustChannel
                && self.completion.stage(i) >= ReceiptStage::Acquired
                && !self.woke[i]
                && self.worker_active.is_none()
                && self.wake.helper < 2
            {
                a.push(A::WorkerWakeBegin(i));
            }
        }
        if let Some(i) = self.worker_active {
            a.push(A::WorkerWakeEnd(i));
        }
        if !self.shutdown && self.all_admitted() {
            a.push(A::Shutdown);
        }
        if self.shutdown
            && self.all_physical()
            && self.wake.helper == 0
            && self.worker_active.is_none()
            && (self.binding != Binding::RustChannel || self.woke.iter().all(|x| *x))
        {
            a.push(A::Quiescent);
        }
        if self.wake.helper == 2 {
            a.push(A::Join);
        }
        for w in self.wake.actions() {
            match w {
                W::BeginNotify => a.push(A::NotifyBegin),
                W::EndNotify => a.push(A::NotifyEnd),
                W::ReleaseProducerReference => a.push(A::ProducerRelease),
                W::Detach if self.shutdown => a.push(A::Detach),
                W::FreeTarget
                    if !self.runtime_reference
                        || self.fault == ComposedFault::IgnoreRuntimeReference =>
                {
                    a.push(A::TargetFree);
                }
                _ => {}
            }
        }
        if self.fault == ComposedFault::DetachAfterOneCopy
            && self.shutdown
            && self.wake.attached
            && (0..self.count).any(|i| self.completion.stage(i) >= ReceiptStage::Acquired)
        {
            a.push(A::Detach);
        }
        if self.fault == ComposedFault::FreeWithProducerReference
            && !self.wake.attached
            && self.wake.target
            && !self.runtime_reference
            && self.wake.producer_reference
        {
            a.push(A::TargetFree);
        }
        if !self.wake.attached && self.runtime_reference {
            a.push(A::RuntimeReferenceRelease);
        }
        if self.environment && !self.wake.context && self.worker_active.is_none() {
            a.push(A::EnvironmentFree);
        }
        a
    }
    fn apply(&mut self, a: Self::Action) {
        use ComposedAction as A;
        match a {
            A::Admit(i) => self.completion.apply(C::Admit(i)),
            A::Write => self.completion.apply(C::Write),
            A::Prefix => self.completion.apply(C::PrefixDone),
            A::Publish => {
                self.completion.apply(C::Publish);
                self.wake.apply(W::Publish);
            }
            A::BeginCopy(i) => self.completion.apply(C::BeginCopy(i)),
            A::Copy(i) => self.completion.copy_payload(i),
            A::Claim(i) => {
                self.completion.claim_completion(i);
                self.claim_attempted[i] = true;
            }
            A::Physical(i) => {
                self.completion.apply(C::FinishCopy(i));
                self.wake.physical_done = self.all_physical();
            }
            A::CloseAdmission => self.completion.apply(C::CloseAdmission),
            A::RetireCarrier => self.completion.apply(C::RetireCarrier),
            A::BeginControl(i, d) => {
                self.control_kind[i] = d;
                self.control_phase[i] = 1;
                self.control_used[i] |= if d == Decision::Cancelled { 1 } else { 2 };
            }
            A::ResolveControl(i) => {
                let accepted = self.completion.control(i, self.control_kind[i]);
                self.control_phase[i] = if accepted { 2 } else { 3 };
                if self.fault == ComposedFault::FreeEnvironmentOnCancel {
                    self.environment = false;
                }
            }
            A::EndControl(i, accepted) => {
                if accepted {
                    self.completion.apply(C::EndControl(i));
                }
                self.control_phase[i] = 0;
            }
            A::Settle(i) => self.completion.apply(C::Settle(i)),
            A::Release(i) => self.completion.apply(C::Release(i)),
            A::Shutdown => self.shutdown = true,
            A::WorkerWakeBegin(i) => self.worker_active = Some(i),
            A::WorkerWakeEnd(i) => {
                self.worker_active = None;
                self.woke[i] = true;
            }
            A::Quiescent => {
                self.wake.apply(W::HelperTail);
                self.wake.apply(W::HelperReturn);
            }
            A::Join => self.wake.apply(W::Join),
            A::NotifyBegin => self.wake.apply(W::BeginNotify),
            A::NotifyEnd => self.wake.apply(W::EndNotify),
            A::ProducerRelease => self.wake.apply(W::ReleaseProducerReference),
            A::Detach => self.wake.apply(W::Detach),
            A::RuntimeReferenceRelease => self.runtime_reference = false,
            A::EnvironmentFree => self.environment = false,
            A::TargetFree => self.wake.apply(W::FreeTarget),
        }
    }
    fn check(&self) -> Result<(), Violation> {
        self.completion.check()?;
        self.wake.check()?;
        require(
            self.completion.published() == self.wake.published,
            "NL-I2",
            "completion and wake disagree on SDK publication",
        )?;
        require(
            self.wake.physical_done == self.all_physical(),
            "NL-I2",
            "wake teardown substituted one receipt for all physical copies",
        )?;
        require(
            self.wake.context || (self.all_physical() && self.worker_active.is_none()),
            "NL-I5",
            "context detached before all physical users drained",
        )?;
        require(
            self.environment || (!self.wake.context && self.worker_active.is_none()),
            "NL-I5",
            "wake environment released while a reader remains",
        )?;
        require(
            self.wake.target || !self.runtime_reference,
            "NL-I7",
            "target freed while the runtime still owns its reference",
        )?;
        for i in 0..self.count {
            if self.completion.stage(i) == ReceiptStage::Released
                && self.binding == Binding::RustChannel
            {
                require(
                    self.woke[i],
                    "NL-I6",
                    "receipt record freed before the worker wake tail ended",
                )?;
            }
        }
        Ok(())
    }
    fn check_step(&self, a: Self::Action, after: &Self) -> Result<(), Violation> {
        if matches!(
            a,
            ComposedAction::BeginControl(..)
                | ComposedAction::ResolveControl(..)
                | ComposedAction::EndControl(..)
        ) {
            require(
                self.wake == after.wake
                    && self.environment == after.environment
                    && (0..self.count)
                        .all(|i| self.completion.stage(i) == after.completion.stage(i)),
                "NL-I3",
                "control changed physical completion or environment lifetime",
            )?;
        }
        Ok(())
    }
    fn terminal(&self) -> bool {
        self.completion.terminal()
            && !self.wake.target
            && !self.environment
            && (self.binding == Binding::CDirect || self.wake.helper == 3)
    }
    fn witnesses(&self) -> Vec<&'static str> {
        let mut w = self.completion.witnesses();
        w.extend(self.wake.witnesses());
        if (0..self.count).any(|i| {
            self.completion.decision(i) == Decision::Completed
                && self.completion.stage(i) == ReceiptStage::Copying
        }) {
            w.push("completion-claimed-before-physical-publication");
        }
        if !self.environment && self.wake.producer_reference {
            w.push("producer-notify-outlives-environment");
        }
        if self.count == 2
            && self.completion.stage(0) >= ReceiptStage::Acquired
            && self.completion.stage(1) < ReceiptStage::Acquired
        {
            w.push("one-copy-does-not-authorize-detach");
        }
        w
    }
}
