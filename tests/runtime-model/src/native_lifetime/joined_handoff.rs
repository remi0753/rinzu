use super::joined::Aux;
use super::search::{Violation, require};
use super::{
    ComposedAction as C, Counterexample, Domain, Joined, JoinedAction as J, Protocol, ReceiptStage,
    RegistrationAction as R, Report, explore,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JoinedHandoffAction {
    Local(Domain, J),
    Handoff,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Origin {
    local: Joined,
    handed_off: bool,
}

impl Origin {
    fn ready(&self) -> bool {
        let completion = &self.local.inner.inner;
        !self.handed_off
            && completion.completion.retired()
            && completion.completion.stage(0) >= ReceiptStage::OwnerSettled
            && self.local.auxiliary >= Aux::Physical
            && completion.wake.producer_reference
    }
}

impl Protocol for Origin {
    type Action = JoinedHandoffAction;

    fn name(&self) -> String {
        "joined-handoff-origin".into()
    }

    fn actions(&self) -> Vec<Self::Action> {
        let mut actions: Vec<_> = self
            .local
            .actions()
            .into_iter()
            .filter(|a| match a {
                J::Primary(R::Product(C::NotifyBegin)) => self.handed_off,
                J::Primary(R::Product(C::Publish)) => self.local.inner.returned(0),
                _ => true,
            })
            .map(|a| JoinedHandoffAction::Local(Domain::First, a))
            .collect();
        if self.ready() {
            actions.push(JoinedHandoffAction::Handoff);
        }
        actions
    }

    fn apply(&mut self, action: Self::Action) {
        match action {
            JoinedHandoffAction::Local(Domain::First, a) => self.local.apply(a),
            JoinedHandoffAction::Handoff => self.handed_off = true,
            JoinedHandoffAction::Local(Domain::Second, _) => unreachable!(),
        }
    }

    fn check(&self) -> Result<(), Violation> {
        self.local.check()
    }

    fn check_step(&self, action: Self::Action, after: &Self) -> Result<(), Violation> {
        match action {
            JoinedHandoffAction::Local(Domain::First, a) => {
                require(
                    self.handed_off == after.handed_off,
                    "NL-I4",
                    "local action moved the carrier",
                )?;
                self.local.check_step(a, &after.local)
            }
            JoinedHandoffAction::Handoff => {
                require(
                    self.ready() && after.handed_off,
                    "NL-I4",
                    "handoff bypassed physical retirement",
                )?;
                require(
                    self.local == after.local,
                    "NL-I7",
                    "handoff changed the old helper or SDK reference",
                )
            }
            JoinedHandoffAction::Local(Domain::Second, _) => unreachable!(),
        }
    }

    fn terminal(&self) -> bool {
        self.handed_off && self.local.terminal()
    }

    fn witnesses(&self) -> Vec<&'static str> {
        let mut result = self.local.witnesses();
        if self.handed_off && self.local.helper == 1 {
            result.push("storage-reused-with-old-helper-in-wake");
        }
        result
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct JoinedHandoff {
    origin: Origin,
    replacement: Joined,
}

impl Default for JoinedHandoff {
    fn default() -> Self {
        Self {
            origin: Origin {
                local: Joined::new(1),
                handed_off: false,
            },
            replacement: Joined::new(1),
        }
    }
}

impl JoinedHandoff {
    /// Exhaust the outgoing and receiving factors. The handoff activates the
    /// receiving factor without changing either helper or the old target.
    ///
    /// # Errors
    /// Returns a failure or incomplete exploration of either factor.
    pub fn verify(limit: usize) -> Result<[Report; 2], Counterexample> {
        let model = Self::default();
        Ok([
            explore(model.origin, limit)?,
            explore(model.replacement, limit)?,
        ])
    }
}

impl Protocol for JoinedHandoff {
    type Action = JoinedHandoffAction;

    fn name(&self) -> String {
        "joined-helper-domain-handoff".into()
    }

    fn actions(&self) -> Vec<Self::Action> {
        let mut actions = self.origin.actions();
        if self.origin.handed_off {
            actions.extend(
                self.replacement
                    .actions()
                    .into_iter()
                    .map(|a| JoinedHandoffAction::Local(Domain::Second, a)),
            );
        }
        actions
    }

    fn apply(&mut self, action: Self::Action) {
        if let JoinedHandoffAction::Local(Domain::Second, a) = action {
            self.replacement.apply(a);
        } else {
            self.origin.apply(action);
        }
    }

    fn check(&self) -> Result<(), Violation> {
        self.origin.check()?;
        self.replacement.check()
    }

    fn check_step(&self, action: Self::Action, after: &Self) -> Result<(), Violation> {
        if let JoinedHandoffAction::Local(Domain::Second, a) = action {
            require(
                self.origin.handed_off && self.origin == after.origin,
                "NL-I5",
                "replacement changed the old helper or target",
            )?;
            self.replacement.check_step(a, &after.replacement)
        } else {
            require(
                self.replacement == after.replacement,
                "NL-I5",
                "origin changed the replacement helper or target",
            )?;
            self.origin.check_step(action, &after.origin)
        }
    }

    fn terminal(&self) -> bool {
        self.origin.terminal() && self.replacement.terminal()
    }
}
