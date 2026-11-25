use super::search::{Violation, require};
use super::{Binding, Composed, ComposedAction as C, Protocol, ReceiptStage};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Slot {
    Empty,
    Target,
    Done,
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Phase {
    Idle,
    Begun,
    Retained,
    Won,
    LostDone,
    LostTarget,
    RolledBack,
    Returned,
}
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum RegistrationFault {
    #[default]
    None,
    TransferWithoutRetain,
    LeakFailedCandidate,
    DropTransferredReference,
    TransferAfterDone,
    TransferTwice,
    RetireDuringRegistration,
    NotifyThroughLockedRuntime,
}

/// Completion/wake product extended to an initially unregistered carrier.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Registration {
    pub(super) inner: Composed,
    phases: Vec<Phase>,
    temporary: Vec<bool>,
    active: Option<usize>,
    slot: Slot,
    pub(super) captured: bool,
    retained: u8,
    rolled_back: u8,
    transferred: u8,
    producer_released: u8,
    pub(super) notify_ready: bool,
    pub fault: RegistrationFault,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistrationAction {
    Product(C),
    Begin(usize),
    Retain(usize),
    CompareExchange(usize),
    Rollback(usize),
    Return(usize),
    NotifyReady,
}
impl Registration {
    /// Start before admission or registration, with a pinned carrier and an
    /// unexposed runtime-owned target. C lazy allocation is abstracted here.
    /// # Panics
    /// Panics unless the receipt bound is one or two.
    #[must_use]
    pub fn new(count: usize, binding: Binding) -> Self {
        Self::with_version(count, binding, 3)
    }
    pub(super) fn with_version(count: usize, binding: Binding, version: u8) -> Self {
        Self {
            inner: Composed::before_registration(count, binding, version),
            phases: vec![Phase::Idle; count],
            temporary: vec![false; count],
            active: None,
            slot: Slot::Empty,
            captured: false,
            retained: 0,
            rolled_back: 0,
            transferred: 0,
            producer_released: 0,
            notify_ready: false,
            fault: RegistrationFault::None,
        }
    }
    pub(super) fn temporary_count(&self) -> u8 {
        u8::try_from(self.temporary.iter().filter(|v| **v).count()).unwrap()
    }
    pub(super) fn registering(&self) -> bool {
        self.active.is_some()
    }
    pub(super) fn returned(&self, i: usize) -> bool {
        self.phases[i] == Phase::Returned
    }
    pub(super) fn return_value(&self, i: usize) -> Option<bool> {
        match self.phases[i] {
            Phase::Won => Some(true),
            Phase::RolledBack => Some(false),
            _ => None,
        }
    }
    fn c_owner_available(&self) -> bool {
        self.active.is_none()
            && !(0..self.inner.count)
                .any(|i| self.inner.completion.stage(i) == ReceiptStage::Copying)
    }
    pub(super) fn snapshot(&self, i: usize) -> Option<(bool, i64)> {
        match self.phases[i] {
            Phase::Won => Some((true, 0)),
            Phase::LostDone => Some((false, 1)),
            Phase::LostTarget => Some((false, 2)),
            _ => None,
        }
    }
    pub(super) fn return_matches(&self, i: usize, won: bool, references: i64) -> bool {
        self.active == Some(i)
            && ((won && self.phases[i] == Phase::Won)
                || (!won && self.phases[i] == Phase::RolledBack))
            && (1..=2).contains(&references)
            && (!self.inner.wake.producer_reference || references == 2)
            && (references != 2 || self.transferred == 1)
    }
    pub(super) fn enabled(&self, a: RegistrationAction) -> bool {
        self.actions().contains(&a)
    }
    pub(super) fn registered_all(&self) -> bool {
        self.phases.iter().all(|p| *p == Phase::Returned)
    }
    fn product_enabled(&self, a: C) -> bool {
        if a == C::Publish && !self.inner.completion.prefix() {
            return false;
        }
        if let C::BeginCopy(i) = a
            && self.phases[i] != Phase::Returned
        {
            return false;
        }
        if a == C::NotifyEnd && !self.notify_ready {
            return false;
        }
        if self.inner.binding != Binding::RustChannel && self.active.is_some() {
            return matches!(
                a,
                C::Write
                    | C::Prefix
                    | C::Publish
                    | C::NotifyBegin
                    | C::NotifyEnd
                    | C::ProducerRelease
            );
        }
        true
    }
}
impl Protocol for Registration {
    type Action = RegistrationAction;
    fn name(&self) -> String {
        format!(
            "registration-{:?}-w{}",
            self.inner.binding, self.inner.count
        )
    }
    fn actions(&self) -> Vec<Self::Action> {
        use RegistrationAction as A;
        let mut actions: Vec<_> = self
            .inner
            .actions()
            .into_iter()
            .filter(|a| self.product_enabled(*a))
            .map(A::Product)
            .collect();
        if self.active.is_none() {
            for i in 0..self.inner.count {
                if self.phases[i] == Phase::Idle
                    && self.inner.completion.stage(i) == ReceiptStage::Waiting
                    && (0..i).all(|j| self.phases[j] == Phase::Returned)
                    && (self.inner.binding == Binding::RustChannel
                        || (self.c_owner_available() && !self.inner.control_in_progress()))
                {
                    actions.push(A::Begin(i));
                }
            }
        }
        if let Some(i) = self.active {
            match self.phases[i] {
                Phase::Begun => {
                    actions.push(A::Retain(i));
                    if self.fault == RegistrationFault::TransferWithoutRetain {
                        actions.push(A::CompareExchange(i));
                    }
                }
                Phase::Retained => actions.push(A::CompareExchange(i)),
                Phase::LostDone | Phase::LostTarget => actions.push(A::Rollback(i)),
                Phase::Won | Phase::RolledBack => actions.push(A::Return(i)),
                _ => {}
            }
            if self.fault == RegistrationFault::RetireDuringRegistration
                && self.inner.completion.published()
            {
                actions.push(A::Product(C::RetireCarrier));
            }
        }
        if self.inner.wake.active_context
            && !self.notify_ready
            && (self.c_owner_available()
                || self.fault == RegistrationFault::NotifyThroughLockedRuntime)
        {
            actions.push(A::NotifyReady);
        }
        actions
    }
    fn apply(&mut self, action: Self::Action) {
        use RegistrationAction as A;
        match action {
            A::Product(a) => {
                if a == C::Publish {
                    self.captured = self.slot == Slot::Target;
                    self.slot = Slot::Done;
                }
                if a == C::ProducerRelease {
                    self.producer_released += 1;
                }
                self.inner.apply(a);
                if a == C::NotifyBegin {
                    self.notify_ready = !self.inner.wake.active_context;
                }
            }
            A::Begin(i) => {
                self.active = Some(i);
                self.phases[i] = Phase::Begun;
            }
            A::Retain(i) => {
                self.temporary[i] = true;
                self.retained += 1;
                self.phases[i] = Phase::Retained;
            }
            A::CompareExchange(i) => {
                let win = self.slot == Slot::Empty
                    || (self.slot == Slot::Done
                        && self.fault == RegistrationFault::TransferAfterDone)
                    || (self.slot == Slot::Target
                        && self.fault == RegistrationFault::TransferTwice);
                if win {
                    if self.slot == Slot::Empty {
                        self.slot = Slot::Target;
                    }
                    self.temporary[i] = false;
                    self.transferred += 1;
                    self.inner.wake.producer_reference = true;
                    self.phases[i] = Phase::Won;
                } else {
                    self.phases[i] = if self.slot == Slot::Done {
                        Phase::LostDone
                    } else {
                        Phase::LostTarget
                    };
                }
            }
            A::Rollback(i) => {
                if self.fault != RegistrationFault::LeakFailedCandidate {
                    self.temporary[i] = false;
                }
                self.rolled_back += 1;
                self.phases[i] = Phase::RolledBack;
            }
            A::Return(i) => {
                if self.fault == RegistrationFault::DropTransferredReference
                    && self.phases[i] == Phase::Won
                {
                    self.rolled_back += 1;
                }
                self.phases[i] = Phase::Returned;
                self.active = None;
            }
            A::NotifyReady => self.notify_ready = true,
        }
    }
    fn check(&self) -> Result<(), Violation> {
        self.inner.check()?;
        require(
            self.inner.completion.published() == (self.slot == Slot::Done),
            "NL-I2",
            "SDK slot and publication disagree",
        )?;
        require(
            usize::from(self.retained)
                == self.temporary.iter().filter(|x| **x).count()
                    + usize::from(self.rolled_back)
                    + usize::from(self.transferred),
            "NL-I7",
            "candidate reference was lost, duplicated, or transferred without retention",
        )?;
        require(
            self.transferred <= 1
                && self.producer_released <= 1
                && self.transferred
                    == u8::from(self.inner.wake.producer_reference) + self.producer_released,
            "NL-I7",
            "SDK reference has no unique owner/releaser",
        )?;
        require(
            !self.inner.completion.published() || self.captured == (self.transferred == 1),
            "NL-I7",
            "exchange captured no target but registration transferred a reference",
        )?;
        require(
            self.inner.wake.target || !self.temporary.iter().any(|x| *x),
            "NL-I7",
            "wake target freed while a registration candidate remains",
        )?;
        require(
            !self.inner.completion.retired() || self.phases.iter().all(|p| *p == Phase::Returned),
            "NL-I4",
            "carrier retired before registration access returned",
        )?;
        for i in 0..self.inner.count {
            require(
                self.inner.completion.stage(i) < ReceiptStage::Copying
                    || self.phases[i] == Phase::Returned,
                "NL-I4",
                "receipt copy bypassed its registration access period",
            )?;
        }
        Ok(())
    }
    fn check_step(&self, action: Self::Action, after: &Self) -> Result<(), Violation> {
        if let RegistrationAction::Product(a) = action {
            self.inner.check_step(a, &after.inner)?;
        }
        if action == RegistrationAction::NotifyReady {
            require(
                self.c_owner_available(),
                "NL-I7",
                "notification crossed the runtime gate during Owner registration/copy",
            )?;
        }
        Ok(())
    }
    fn terminal(&self) -> bool {
        self.inner.terminal()
            && self.active.is_none()
            && self.phases.iter().all(|p| *p == Phase::Returned)
            && !self.temporary.iter().any(|p| *p)
    }
    fn witnesses(&self) -> Vec<&'static str> {
        let mut witnesses = self.inner.witnesses();
        if self.inner.completion.published() && self.phases[0] == Phase::Idle {
            witnesses.push("sdk-published-before-first-registration");
        }
        if self.inner.completion.published() && self.phases.contains(&Phase::Won) {
            witnesses.push("registration-success-return-can-follow-sdk-exchange");
        }
        if self.phases.contains(&Phase::LostDone) {
            witnesses.push("candidate-loses-to-done");
        }
        if self.phases.contains(&Phase::LostTarget) {
            witnesses.push("candidate-loses-to-same-target");
        }
        if self.terminal() && self.transferred == 0 {
            witnesses.push("no-notify-after-unregistered-publication");
        }
        if self.phases.contains(&Phase::Won) && self.producer_released == 1 {
            witnesses.push("producer-release-before-registration-return");
        }
        witnesses
    }
}
