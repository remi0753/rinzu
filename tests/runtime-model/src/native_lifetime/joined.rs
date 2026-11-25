use super::search::{Violation, require};
use super::{
    Binding, ComposedAction as C, Protocol, ReceiptStage, Registration, RegistrationAction as R,
};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) enum Aux {
    Absent,
    Waiting,
    Copied,
    Claimed,
    Physical,
    Settled,
    Released,
}
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum JoinedFault {
    #[default]
    None,
    DetachWithoutJoin,
    FreeDuringHelperWake,
    RetireAuxEarly,
    JoinDuringHelperWake,
}
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Auxiliary carrier pin, publication and worker wake differ"
)]
pub struct Joined {
    pub(super) inner: Registration,
    pub(super) auxiliary: Aux,
    pub(super) bound: bool,
    pub(super) published: bool,
    pub(super) retired: bool,
    pub(super) helper: u8,
    pub fault: JoinedFault,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JoinedAction {
    Primary(R),
    AdmitAux,
    BindAux,
    PublishAux,
    CopyAux,
    ClaimAux,
    PhysicalAux,
    SettleAux,
    ReleaseAux,
    RetireAux,
    HelperWakeBegin,
    HelperWakeEnd,
    UnsafeDetach,
    UnsafeJoin,
}
impl Joined {
    /// One notified v3 carrier with one or two receipts and one polling v1 receipt.
    /// # Panics
    /// Panics unless the notified receipt bound is one or two.
    #[must_use]
    pub fn new(count: usize) -> Self {
        Self {
            inner: Registration::new(count, Binding::CJoined),
            auxiliary: Aux::Absent,
            bound: false,
            published: false,
            retired: false,
            helper: 0,
            fault: JoinedFault::None,
        }
    }
    pub(super) fn physical(&self) -> bool {
        (0..self.inner.inner.count)
            .all(|i| self.inner.inner.completion.stage(i) >= ReceiptStage::Acquired)
    }
    pub(super) fn enabled(&self, a: JoinedAction) -> bool {
        self.actions().contains(&a)
    }
}
impl Protocol for Joined {
    type Action = JoinedAction;
    fn name(&self) -> String {
        format!("joined-mixed-v1-v3-w{}", self.inner.inner.count)
    }
    fn actions(&self) -> Vec<Self::Action> {
        use JoinedAction as A;
        let mut a: Vec<_> = self
            .inner
            .actions()
            .into_iter()
            .filter(|r| {
                (!matches!(r, R::Product(C::Quiescent))
                    || (self.auxiliary >= Aux::Physical && self.helper == 2))
                    && (!matches!(r, R::Product(C::Shutdown)) || self.auxiliary != Aux::Absent)
                    && (!matches!(r, R::NotifyReady | R::Product(C::BeginCopy(_)))
                        || !matches!(self.auxiliary, Aux::Copied | Aux::Claimed))
            })
            .map(A::Primary)
            .collect();
        if self.auxiliary == Aux::Absent
            && self.inner.registered_all()
            && !self.inner.inner.shutdown
        {
            a.push(A::AdmitAux);
        }
        if self.auxiliary == Aux::Waiting && !self.bound {
            a.push(A::BindAux);
        }
        if self.bound && !self.published {
            a.push(A::PublishAux);
        }
        match self.auxiliary {
            Aux::Waiting
                if self.bound
                    && self.published
                    && !(0..self.inner.inner.count)
                        .any(|i| self.inner.inner.completion.stage(i) == ReceiptStage::Copying) =>
            {
                a.push(A::CopyAux);
            }
            Aux::Copied => a.push(A::ClaimAux),
            Aux::Claimed => a.push(A::PhysicalAux),
            Aux::Physical => a.push(A::SettleAux),
            Aux::Settled => a.push(A::ReleaseAux),
            _ => {}
        }
        if self.bound
            && !self.retired
            && (self.auxiliary >= Aux::Physical || self.fault == JoinedFault::RetireAuxEarly)
        {
            a.push(A::RetireAux);
        }
        if self.helper == 0
            && self.physical()
            && self.auxiliary >= Aux::Physical
            && self.inner.inner.wake.helper == 0
        {
            a.push(A::HelperWakeBegin);
        }
        if self.helper == 1 {
            a.push(A::HelperWakeEnd);
            if self.fault == JoinedFault::JoinDuringHelperWake {
                a.push(A::UnsafeJoin);
            }
        }
        if self.fault == JoinedFault::DetachWithoutJoin
            && self.inner.inner.shutdown
            && self.physical()
            && self.inner.inner.wake.attached
        {
            a.push(A::UnsafeDetach);
        }
        a
    }
    fn apply(&mut self, a: Self::Action) {
        use JoinedAction as A;
        match a {
            A::Primary(r) => self.inner.apply(r),
            A::AdmitAux => self.auxiliary = Aux::Waiting,
            A::BindAux => self.bound = true,
            A::PublishAux => self.published = true,
            A::CopyAux => self.auxiliary = Aux::Copied,
            A::ClaimAux => self.auxiliary = Aux::Claimed,
            A::PhysicalAux => self.auxiliary = Aux::Physical,
            A::SettleAux => self.auxiliary = Aux::Settled,
            A::ReleaseAux => self.auxiliary = Aux::Released,
            A::RetireAux => self.retired = true,
            A::HelperWakeBegin => {
                self.helper = 1;
                if self.fault == JoinedFault::FreeDuringHelperWake {
                    self.inner.inner.environment = false;
                }
            }
            A::HelperWakeEnd => self.helper = 2,
            A::UnsafeDetach => {
                self.inner.inner.wake.attached = false;
                self.inner.inner.wake.context = false;
            }
            A::UnsafeJoin => self.inner.apply(R::Product(C::Join)),
        }
    }
    fn check(&self) -> Result<(), Violation> {
        self.inner.check()?;
        require(
            !self.retired || (self.published && self.auxiliary >= Aux::Physical),
            "NL-I4",
            "polling carrier retired before physical copy",
        )?;
        require(
            self.auxiliary < Aux::Copied || (self.bound && self.published),
            "NL-I2",
            "polling receipt copied before acquired publication",
        )?;
        require(
            self.inner.inner.wake.attached
                || (self.auxiliary >= Aux::Physical
                    && self.helper == 2
                    && self.inner.inner.wake.helper == 3),
            "NL-I5",
            "context detached without polling drain and actual helper join",
        )?;
        require(
            self.inner.inner.wake.helper != 3 || self.helper == 2,
            "NL-I5",
            "helper joined while its wake access remains active",
        )?;
        require(
            self.inner.inner.environment || self.helper != 1,
            "NL-I5",
            "helper wake accessed a freed environment",
        )?;
        Ok(())
    }
    fn check_step(&self, a: Self::Action, n: &Self) -> Result<(), Violation> {
        if let JoinedAction::Primary(r) = a {
            self.inner.check_step(r, &n.inner)?;
        }
        Ok(())
    }
    fn terminal(&self) -> bool {
        self.inner.terminal() && self.auxiliary == Aux::Released && self.retired && self.helper == 2
    }
    fn witnesses(&self) -> Vec<&'static str> {
        let mut w = self.inner.witnesses();
        if self.helper == 1 && self.inner.inner.wake.active_context {
            w.push("helper-and-sdk-context-readers-overlap");
        }
        if self.helper == 1 && self.auxiliary == Aux::Released {
            w.push("polling-record-released-during-helper-wake");
        }
        if self.helper == 1 && self.inner.inner.shutdown {
            w.push("shutdown-waits-for-helper-wake");
        }
        w
    }
}
