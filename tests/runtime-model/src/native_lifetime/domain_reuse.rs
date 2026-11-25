use super::search::{Violation, require};
use super::shared_target::{Receipt, Sdk};
use super::{
    Binding, Counterexample, Domain, Protocol, Report, SharedAction, SharedTarget, SharedTopology,
    explore,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaleControl {
    Cancel,
    Timeout,
}

impl StaleControl {
    const fn bit(self) -> u8 {
        match self {
            Self::Cancel => 1,
            Self::Timeout => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DomainReuseAction {
    Local(Domain, SharedAction),
    Handoff,
    ReadOldReceipt,
    ControlOldReceipt(StaleControl),
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum DomainReuseFault {
    #[default]
    None,
    ReuseBeforeRetirement,
    NotifyReplacement,
    ReturnReplacementReference,
    ReadReplacement,
    SignalReplacement,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Origin {
    shared: SharedTarget,
    handoff: Option<Handoff>,
    cache_read: Option<Domain>,
    rejected_controls: u8,
    fault: DomainReuseFault,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Handoff {
    LiveOrigin,
    ReleasedOrigin,
}

impl Origin {
    fn has_settled_receipt_and_saved_target(&self) -> bool {
        let [scalar, unit] = &self.shared.units[..] else {
            unreachable!("reuse has two receipts per Domain")
        };
        scalar.receipt >= Receipt::Settled
            && scalar.sdk == Sdk::Captured
            && unit.sdk == Sdk::Released
    }
}

impl Protocol for Origin {
    type Action = DomainReuseAction;

    fn name(&self) -> String {
        format!("reuse-origin-{:?}", self.shared.binding)
    }

    fn actions(&self) -> Vec<Self::Action> {
        use DomainReuseAction as A;
        use SharedAction as S;
        let mut actions: Vec<_> = self
            .shared
            .actions()
            .into_iter()
            .filter(|action| match action {
                S::NotifyBegin(0) => self.handoff.is_some(),
                S::ReleaseRecord(0) if self.handoff.is_none() => self.shared.units[0].retired,
                S::ReleaseRecord(0) => self.cache_read.is_some() && self.rejected_controls == 3,
                _ => true,
            })
            .map(|action| A::Local(Domain::First, action))
            .collect();
        if self.handoff.is_none()
            && self.has_settled_receipt_and_saved_target()
            && (self.shared.units[0].retired
                || self.fault == DomainReuseFault::ReuseBeforeRetirement)
        {
            actions.push(A::Handoff);
        }
        if self.handoff.is_some() && self.shared.units[0].receipt == Receipt::Settled {
            if self.cache_read.is_none() {
                actions.push(A::ReadOldReceipt);
            }
            for control in [StaleControl::Cancel, StaleControl::Timeout] {
                if self.rejected_controls & control.bit() == 0 {
                    actions.push(A::ControlOldReceipt(control));
                }
            }
        }
        actions
    }

    fn apply(&mut self, action: Self::Action) {
        match action {
            DomainReuseAction::Local(Domain::First, action) => self.shared.apply(action),
            DomainReuseAction::Handoff => {
                self.handoff = Some(if self.shared.environment {
                    Handoff::LiveOrigin
                } else {
                    Handoff::ReleasedOrigin
                });
            }
            DomainReuseAction::ReadOldReceipt => {
                self.cache_read = Some(if self.fault == DomainReuseFault::ReadReplacement {
                    Domain::Second
                } else {
                    Domain::First
                });
            }
            DomainReuseAction::ControlOldReceipt(control) => {
                self.rejected_controls |= control.bit();
            }
            DomainReuseAction::Local(Domain::Second, _) => unreachable!(),
        }
    }

    fn check(&self) -> Result<(), Violation> {
        self.shared.check()?;
        require(
            self.handoff.is_none() || self.shared.units[0].retired,
            "NL-I4",
            "another Domain took the carrier before retirement",
        )?;
        require(
            self.cache_read.is_none_or(|domain| domain == Domain::First),
            "NL-I8",
            "old receipt read the replacement generation",
        )
    }

    fn check_step(&self, action: Self::Action, after: &Self) -> Result<(), Violation> {
        match action {
            DomainReuseAction::Local(Domain::First, local) => {
                require(self.handoff == after.handoff, "NL-I4", "implicit reuse")?;
                self.shared.check_step(local, &after.shared)
            }
            DomainReuseAction::Handoff => {
                require(
                    self.handoff.is_none()
                        && after.handoff.is_some()
                        && self.shared.units[0].retired,
                    "NL-I4",
                    "handoff requires the old generation to be retired",
                )?;
                require(
                    self.has_settled_receipt_and_saved_target() && self.shared == after.shared,
                    "NL-I7",
                    "handoff changed or lost the retained old target",
                )
            }
            DomainReuseAction::ReadOldReceipt | DomainReuseAction::ControlOldReceipt(_) => require(
                self.handoff.is_some()
                    && self.handoff == after.handoff
                    && self.shared == after.shared,
                "NL-I8",
                "a stale receipt accessed the carrier or changed the old target",
            ),
            DomainReuseAction::Local(Domain::Second, _) => unreachable!(),
        }
    }

    fn terminal(&self) -> bool {
        self.handoff.is_some() && self.shared.terminal()
    }

    fn witnesses(&self) -> Vec<&'static str> {
        let mut result = self.shared.witnesses();
        if self.handoff.is_some() && self.shared.units[0].sdk == Sdk::Captured {
            result.push("old-sdk-reference-survives-cross-domain-handoff");
            if !self.shared.environment {
                result.push("old-sdk-reference-outlives-origin-after-handoff");
            }
        }
        if self.handoff == Some(Handoff::ReleasedOrigin) {
            result.push("environment-released-before-cross-domain-handoff");
        }
        if self.cache_read.is_some() && self.rejected_controls == 3 {
            result.push("old-receipt-reads-cache-and-rejects-both-controls");
        }
        result
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DomainReuse {
    origin: Origin,
    replacement: SharedTarget,
}

impl DomainReuse {
    #[must_use]
    pub fn new(binding: Binding) -> Self {
        let shared = SharedTarget::new(binding, SharedTopology::Coalesced);
        Self {
            origin: Origin {
                shared: shared.clone(),
                handoff: None,
                cache_read: None,
                rejected_controls: 0,
                fault: DomainReuseFault::None,
            },
            replacement: shared,
        }
    }

    pub fn inject(&mut self, fault: DomainReuseFault) {
        self.origin.fault = fault;
    }

    #[must_use]
    pub fn owner(&self, domain: Domain) -> &SharedTarget {
        match domain {
            Domain::First => &self.origin.shared,
            Domain::Second => &self.replacement,
        }
    }

    /// Exhaust both factors of the one-way storage handoff. Before handoff only
    /// the origin can step. Each handoff enters the same fresh replacement,
    /// and afterwards neither factor consults or changes the other. Thus each
    /// enabled product step projects to a checked edge. A reachable product
    /// deadlock would require a deadlock in at least one nonterminal factor.
    ///
    /// # Errors
    /// Returns a violation, deadlock or incomplete exploration of either factor.
    pub fn verify(binding: Binding, limit: usize) -> Result<[Report; 2], Counterexample> {
        let model = Self::new(binding);
        Ok([
            explore(model.origin, limit)?,
            explore(model.replacement, limit)?,
        ])
    }
}

impl Protocol for DomainReuse {
    type Action = DomainReuseAction;

    fn name(&self) -> String {
        format!("domain-reuse-{:?}", self.replacement.binding)
    }

    fn actions(&self) -> Vec<Self::Action> {
        let mut actions = self.origin.actions();
        if self.origin.handoff.is_some() {
            actions.extend(
                self.replacement
                    .actions()
                    .into_iter()
                    .map(|local| DomainReuseAction::Local(Domain::Second, local)),
            );
        }
        actions
    }

    fn apply(&mut self, action: Self::Action) {
        use DomainReuseAction as A;
        use SharedAction as S;
        if let A::Local(Domain::Second, local) = action {
            self.replacement.apply(local);
            return;
        }
        self.origin.apply(action);
        match (self.origin.fault, action) {
            (DomainReuseFault::NotifyReplacement, A::Local(_, S::NotifyBegin(0)))
                if self.replacement.enabled(S::NotifyBegin(0)) =>
            {
                self.replacement.apply(S::NotifyBegin(0));
            }
            (DomainReuseFault::ReturnReplacementReference, A::Local(_, S::Decrement(0)))
                if matches!(self.replacement.units[0].sdk, Sdk::Slot | Sdk::Captured) =>
            {
                self.replacement.apply(S::Decrement(0));
            }
            (DomainReuseFault::SignalReplacement, A::ControlOldReceipt(_)) => {
                self.replacement.units[0].cancelled = true;
            }
            _ => {}
        }
    }

    fn check(&self) -> Result<(), Violation> {
        self.origin.check()?;
        self.replacement.check()
    }

    fn check_step(&self, action: Self::Action, after: &Self) -> Result<(), Violation> {
        use DomainReuseAction as A;
        use SharedAction as S;
        if let A::Local(Domain::Second, local) = action {
            require(
                self.origin.handoff.is_some() && self.origin == after.origin,
                "NL-I4",
                "replacement accessed the origin",
            )?;
            return self.replacement.check_step(local, &after.replacement);
        }
        require(
            self.replacement == after.replacement,
            match action {
                A::Local(_, S::NotifyBegin(_) | S::NotifyReady(_) | S::NotifyEnd(_)) => "NL-I5",
                A::ReadOldReceipt | A::ControlOldReceipt(_) => "NL-I8",
                _ => "NL-I7",
            },
            "old Domain action changed the replacement",
        )?;
        self.origin.check_step(action, &after.origin)
    }

    fn terminal(&self) -> bool {
        self.origin.terminal() && self.replacement.terminal()
    }
}

/// Receipt orders constrained only by recycling receipt 0's carrier before
/// starting the new Domain (receipts 2 and 3).
#[must_use]
pub fn reuse_orders() -> Vec<[usize; 4]> {
    [[2, 3], [3, 2]]
        .into_iter()
        .flat_map(|[a, b]| [[0, 1, a, b], [0, a, 1, b], [0, a, b, 1], [1, 0, a, b]])
        .collect()
}
