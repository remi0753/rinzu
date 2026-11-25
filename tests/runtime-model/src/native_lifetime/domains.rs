use super::search::{Violation, require};
use super::{
    Binding, Counterexample, Protocol, Report, SharedAction, SharedTarget, SharedTopology, explore,
};

/// Two concurrently live runtime notification Domains.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Domain {
    First,
    Second,
}

impl Domain {
    pub const ALL: [Self; 2] = [Self::First, Self::Second];

    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::First => 0,
            Self::Second => 1,
        }
    }

    #[must_use]
    pub const fn other(self) -> Self {
        match self {
            Self::First => Self::Second,
            Self::Second => Self::First,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DomainAction {
    Local(Domain, SharedAction),
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum DomainFault {
    #[default]
    None,
    ReturnForeignReference,
    DetachForeignEnvironment,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DomainBatches {
    owners: [SharedTarget; 2],
    pub fault: DomainFault,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainReport {
    pub local: [Report; 2],
    pub states: u128,
    pub transitions: u128,
    pub terminals: u128,
}

impl DomainBatches {
    #[must_use]
    pub fn new(binding: Binding) -> Self {
        let local = SharedTarget::new(binding, SharedTopology::Coalesced);
        Self {
            owners: [local.clone(), local],
            fault: DomainFault::None,
        }
    }

    #[must_use]
    pub fn owner(&self, domain: Domain) -> &SharedTarget {
        &self.owners[domain.index()]
    }

    /// Exhaust the local factors and count their exact asynchronous product.
    ///
    /// A tagged action has only one destination. Its guard reads that factor
    /// and its transition replaces only that factor. Every reachable pair is
    /// therefore reachable by concatenating two local paths, and projecting
    /// any enabled interleaving gives two local paths. A product deadlock
    /// would require both local paths to be stuck. Mutated routing is checked
    /// with explicit product transitions instead of this reduction.
    ///
    /// # Errors
    /// Returns a local violation, deadlock, or incomplete exploration.
    pub fn verify(binding: Binding, limit: usize) -> Result<DomainReport, Counterexample> {
        let model = Self::new(binding);
        let local = explore(model.owners[0].clone(), limit)?;
        // The second factor has the same initial state, guards and transitions.
        let states = local.states as u128;
        Ok(DomainReport {
            states: states * states,
            transitions: 2 * states * local.transitions as u128,
            terminals: (local.terminals as u128).pow(2),
            local: [local.clone(), local],
        })
    }

    fn local_actions(&self, domain: Domain) -> impl Iterator<Item = DomainAction> + '_ {
        self.owner(domain)
            .actions()
            .into_iter()
            .map(move |action| DomainAction::Local(domain, action))
    }
}

impl Protocol for DomainBatches {
    type Action = DomainAction;

    fn name(&self) -> String {
        format!("notification-domains-{:?}", self.owners[0].binding)
    }

    fn actions(&self) -> Vec<DomainAction> {
        Domain::ALL
            .into_iter()
            .flat_map(|domain| self.local_actions(domain))
            .collect()
    }

    fn apply(&mut self, DomainAction::Local(domain, action): DomainAction) {
        self.owners[domain.index()].apply(action);
        let other = &mut self.owners[domain.other().index()];
        match (self.fault, action) {
            (DomainFault::ReturnForeignReference, SharedAction::Decrement(_))
                if other.units[0].sdk == super::shared_target::Sdk::Captured =>
            {
                other.apply(SharedAction::Decrement(0));
            }
            (DomainFault::DetachForeignEnvironment, SharedAction::Detach) => {
                other.apply(SharedAction::Detach);
            }
            _ => {}
        }
    }

    fn check(&self) -> Result<(), Violation> {
        for owner in &self.owners {
            owner.check()?;
        }
        Ok(())
    }

    fn check_step(
        &self,
        DomainAction::Local(domain, action): DomainAction,
        after: &Self,
    ) -> Result<(), Violation> {
        require(
            self.owner(domain.other()) == after.owner(domain.other()),
            if action == SharedAction::Detach {
                "NL-I5"
            } else {
                "NL-I7"
            },
            "a local transition changed another Domain",
        )?;
        self.owner(domain).check_step(action, after.owner(domain))
    }

    fn terminal(&self) -> bool {
        self.owners.iter().all(Protocol::terminal)
    }
}
