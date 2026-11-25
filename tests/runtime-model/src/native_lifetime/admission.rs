use super::search::{Protocol, Violation, require};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum AdmissionFault {
    #[default]
    None,
    LoseDrainOnRejection,
    RetryDropsControl,
    AcceptSecondControl,
    ReleaseRootWithDeferred,
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum Owner {
    Caller,
    Deferred,
    Receipt,
    Released,
    Lost,
}
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Independent protocol axes must remain separately observable"
)]
pub struct Admission {
    full: bool,
    pub(super) credit_available: bool,
    pub(super) attempted: bool,
    pub(super) owner: Owner,
    pub(super) cancelled: bool,
    pub(super) runtime_cancelled: bool,
    accepted_controls: u8,
    pub(super) published: bool,
    pub(super) collected: bool,
    pub(super) root_released: bool,
    pub fault: AdmissionFault,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    TryAdmit,
    CreditAvailable,
    RetryAdmit,
    Cancel,
    Publish,
    Collect,
    CollectDeferred,
    Release,
    ReleaseRoot,
}
impl Admission {
    #[must_use]
    pub const fn new(full: bool) -> Self {
        Self {
            full,
            credit_available: !full,
            attempted: false,
            owner: Owner::Caller,
            cancelled: false,
            runtime_cancelled: false,
            accepted_controls: 0,
            published: false,
            collected: false,
            root_released: false,
            fault: AdmissionFault::None,
        }
    }
}
impl Protocol for Admission {
    type Action = Action;
    fn name(&self) -> String {
        format!("already-started-sdk-admission-full{}", self.full)
    }
    fn actions(&self) -> Vec<Action> {
        use Action as A;
        let mut a = Vec::new();
        if !self.attempted {
            a.push(A::TryAdmit);
        }
        if !self.credit_available {
            a.push(A::CreditAvailable);
        }
        if self.owner == Owner::Deferred && self.credit_available {
            a.push(A::RetryAdmit);
        }
        if !self.cancelled
            || (self.fault == AdmissionFault::AcceptSecondControl
                && self.owner == Owner::Deferred
                && self.accepted_controls == 1)
        {
            a.push(A::Cancel);
        }
        if !self.published {
            a.push(A::Publish);
        }
        if self.owner == Owner::Receipt && self.published && !self.collected {
            a.push(A::Collect);
        }
        if self.owner == Owner::Deferred && self.published && !self.collected {
            a.push(A::CollectDeferred);
        }
        if self.collected && self.owner != Owner::Released {
            a.push(A::Release);
        }
        if self.owner == Owner::Released && !self.root_released {
            a.push(A::ReleaseRoot);
        }
        if self.fault == AdmissionFault::ReleaseRootWithDeferred
            && self.owner == Owner::Deferred
            && !self.root_released
        {
            a.push(A::ReleaseRoot);
        }
        a
    }
    fn apply(&mut self, a: Action) {
        use Action as A;
        match a {
            A::TryAdmit => {
                self.attempted = true;
                self.owner = if self.credit_available {
                    Owner::Receipt
                } else if self.fault == AdmissionFault::LoseDrainOnRejection {
                    Owner::Lost
                } else {
                    Owner::Deferred
                };
                if self.owner == Owner::Receipt {
                    self.runtime_cancelled = self.cancelled;
                }
            }
            A::CreditAvailable => self.credit_available = true,
            A::RetryAdmit => {
                self.owner = Owner::Receipt;
                if self.fault != AdmissionFault::RetryDropsControl {
                    self.runtime_cancelled = self.cancelled;
                }
            }
            A::Cancel => {
                self.cancelled = true;
                self.accepted_controls += 1;
                if self.owner == Owner::Receipt {
                    self.runtime_cancelled = true;
                }
            }
            A::Publish => self.published = true,
            A::Collect | A::CollectDeferred => self.collected = true,
            A::Release => self.owner = Owner::Released,
            A::ReleaseRoot => self.root_released = true,
        }
    }
    fn check_step(&self, a: Action, after: &Self) -> Result<(), Violation> {
        if matches!(a, Action::Cancel) {
            require(
                self.owner == after.owner
                    && self.published == after.published
                    && self.collected == after.collected
                    && self.root_released == after.root_released,
                "NL-I3",
                "cancellation fabricated physical collection",
            )?;
        }
        Ok(())
    }
    fn check(&self) -> Result<(), Violation> {
        require(
            self.accepted_controls <= 1
                && (self.owner != Owner::Receipt || !self.cancelled || self.runtime_cancelled),
            "NL-I1",
            "deferred control winner was duplicated or lost during retry",
        )?;
        require(
            self.collected || !matches!(self.owner, Owner::Lost | Owner::Released),
            "NL-I9",
            "admission rejection lost drain ownership of already-started work",
        )?;
        require(
            !self.collected || self.published,
            "NL-I2",
            "collection before publication",
        )?;
        require(
            !self.root_released || (self.collected && self.owner == Owner::Released),
            "NL-I5",
            "Root released with a physical SDK user",
        )?;
        Ok(())
    }
    fn terminal(&self) -> bool {
        self.root_released
    }
    fn witnesses(&self) -> Vec<&'static str> {
        let mut w = Vec::new();
        if self.full && self.owner == Owner::Deferred && self.cancelled && !self.published {
            w.push("saturated-cancelled-sdk-retains-deferred-drain");
        }
        if self.owner == Owner::Deferred && self.collected && !self.credit_available {
            w.push("deferred-direct-copy-without-runtime-credit");
        }
        if self.full && self.owner == Owner::Receipt && self.credit_available {
            w.push("deferred-or-late-admission-uses-returned-credit");
        }
        w
    }
}
