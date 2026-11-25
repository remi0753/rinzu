use super::search::{Violation, require};
use super::shared_target::{Receipt, Reg, Sdk};
use super::{
    Binding, Counterexample, Protocol, Report, SharedAction, SharedTarget, SharedTopology, explore,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LaneAction {
    Local(SharedAction),
    Transfer,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Outgoing {
    Waiting,
    Transferred,
    Last,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Lane {
    shared: SharedTarget,
    outgoing: Outgoing,
}

impl Lane {
    fn new(binding: Binding, topology: SharedTopology, outgoing: Outgoing) -> Self {
        Self {
            shared: SharedTarget::new(binding, topology),
            outgoing,
        }
    }

    fn saved_references(&self) -> bool {
        let duplicate = self
            .shared
            .topology
            .coalesced_leader()
            .map(|leader| leader + 1);
        self.shared.units.iter().enumerate().all(|(g, unit)| {
            unit.exchanged
                && unit.sdk
                    == if duplicate == Some(g) {
                        Sdk::Released
                    } else {
                        Sdk::Captured
                    }
        })
    }

    fn ready_for_transfer(&self) -> bool {
        let first = &self.shared.units[0];
        self.outgoing == Outgoing::Waiting
            && first.retired
            && first.receipt >= Receipt::Settled
            && self.saved_references()
    }
}

impl Protocol for Lane {
    type Action = LaneAction;

    fn name(&self) -> String {
        format!(
            "reuse-lane-{:?}-{:?}-{:?}",
            self.shared.binding, self.shared.topology, self.outgoing
        )
    }

    fn actions(&self) -> Vec<Self::Action> {
        let mut actions: Vec<_> = self
            .shared
            .actions()
            .into_iter()
            .filter(|action| match action {
                SharedAction::PublishBegin(g) => self.shared.units[*g].reg == Reg::Returned,
                SharedAction::NotifyBegin(_) => self.outgoing != Outgoing::Waiting,
                _ => true,
            })
            .map(LaneAction::Local)
            .collect();
        if self.ready_for_transfer() {
            actions.push(LaneAction::Transfer);
        }
        actions
    }

    fn apply(&mut self, action: Self::Action) {
        match action {
            LaneAction::Local(action) => self.shared.apply(action),
            LaneAction::Transfer => self.outgoing = Outgoing::Transferred,
        }
    }

    fn check(&self) -> Result<(), Violation> {
        self.shared.check()
    }

    fn check_step(&self, action: Self::Action, after: &Self) -> Result<(), Violation> {
        match action {
            LaneAction::Local(action) => {
                require(
                    self.outgoing == after.outgoing,
                    "NL-I4",
                    "local action moved storage",
                )?;
                self.shared.check_step(action, &after.shared)
            }
            LaneAction::Transfer => {
                require(
                    self.ready_for_transfer() && after.outgoing == Outgoing::Transferred,
                    "NL-I4",
                    "handoff skipped carrier retirement or the two final exchanges",
                )?;
                require(
                    self.shared == after.shared,
                    "NL-I7",
                    "handoff changed the sender's SDK references",
                )
            }
        }
    }

    fn terminal(&self) -> bool {
        self.outgoing != Outgoing::Waiting && self.shared.terminal()
    }

    fn witnesses(&self) -> Vec<&'static str> {
        let mut result = self.shared.witnesses();
        if self.outgoing == Outgoing::Transferred && self.saved_references() {
            result.push("two-sdk-references-survive-handoff");
        }
        result
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChainAction {
    Local(usize, SharedAction),
    Handoff(usize),
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ChainFault {
    #[default]
    None,
    ReturnToLatestOwner,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DomainChain<const N: usize> {
    lanes: [Lane; N],
    latest: usize,
    pub fault: ChainFault,
}

impl<const N: usize> DomainChain<N> {
    /// # Panics
    /// Requires at least two Domains.
    #[must_use]
    pub fn new(binding: Binding) -> Self {
        Self::with_topology(binding, SharedTopology::Carriers)
    }

    /// Use one notification with a single publisher followed by a merged batch.
    ///
    /// # Panics
    /// Requires at least two Domains.
    #[must_use]
    pub fn with_consecutive_batches(binding: Binding) -> Self {
        Self::with_topology(binding, SharedTopology::Consecutive)
    }

    fn with_topology(binding: Binding, topology: SharedTopology) -> Self {
        assert!(N >= 2);
        Self {
            lanes: std::array::from_fn(|index| {
                Lane::new(
                    binding,
                    topology,
                    if index + 1 == N {
                        Outgoing::Last
                    } else {
                        Outgoing::Waiting
                    },
                )
            }),
            latest: 0,
            fault: ChainFault::None,
        }
    }

    /// Exhaust the sender and terminal factors. Every handoff enables a
    /// fresh factor with the same initial state. Local actions in one
    /// factor have no dependencies on older factors or the current owner.
    /// This reduction holds for every finite chain length.
    ///
    /// # Errors
    /// Returns a local invariant violation, deadlock or incomplete search.
    pub fn verify(binding: Binding, limit: usize) -> Result<[Report; 2], Counterexample> {
        Self::verify_topology(binding, SharedTopology::Carriers, limit)
    }

    /// Exhaust the factors of the consecutive-batch handoff.
    ///
    /// # Errors
    /// Returns a violation, deadlock, or incomplete local exploration.
    pub fn verify_consecutive_batches(
        binding: Binding,
        limit: usize,
    ) -> Result<[Report; 2], Counterexample> {
        Self::verify_topology(binding, SharedTopology::Consecutive, limit)
    }

    fn verify_topology(
        binding: Binding,
        topology: SharedTopology,
        limit: usize,
    ) -> Result<[Report; 2], Counterexample> {
        Ok([
            explore(Lane::new(binding, topology, Outgoing::Waiting), limit)?,
            explore(Lane::new(binding, topology, Outgoing::Last), limit)?,
        ])
    }

    /// # Panics
    /// The Domain index must be smaller than the chain length.
    #[must_use]
    pub fn owner(&self, domain: usize) -> &SharedTarget {
        &self.lanes[domain].shared
    }
}

impl<const N: usize> Protocol for DomainChain<N> {
    type Action = ChainAction;

    fn name(&self) -> String {
        format!("{N}-domain-reuse-{:?}", self.lanes[0].shared.binding)
    }

    fn actions(&self) -> Vec<Self::Action> {
        self.lanes[..=self.latest]
            .iter()
            .enumerate()
            .flat_map(|(index, lane)| {
                lane.actions().into_iter().map(move |action| match action {
                    LaneAction::Local(action) => ChainAction::Local(index, action),
                    LaneAction::Transfer => ChainAction::Handoff(index),
                })
            })
            .collect()
    }

    fn apply(&mut self, action: Self::Action) {
        match action {
            ChainAction::Local(index, local) => {
                self.lanes[index].apply(LaneAction::Local(local));
                let latest = &mut self.lanes[self.latest].shared;
                if self.fault == ChainFault::ReturnToLatestOwner
                    && index != self.latest
                    && matches!(local, SharedAction::Decrement(_))
                    && matches!(latest.units[0].sdk, Sdk::Slot | Sdk::Captured)
                {
                    latest.apply(SharedAction::Decrement(0));
                }
            }
            ChainAction::Handoff(index) => {
                self.lanes[index].apply(LaneAction::Transfer);
                self.latest += 1;
            }
        }
    }

    fn check(&self) -> Result<(), Violation> {
        for lane in &self.lanes {
            lane.check()?;
        }
        Ok(())
    }

    fn check_step(&self, action: Self::Action, after: &Self) -> Result<(), Violation> {
        let (index, step) = match action {
            ChainAction::Local(index, action) => {
                require(
                    index <= self.latest && self.latest == after.latest,
                    "NL-I4",
                    "local action used an inactive carrier owner",
                )?;
                (index, LaneAction::Local(action))
            }
            ChainAction::Handoff(index) => {
                require(
                    index == self.latest && after.latest == index + 1 && after.latest < N,
                    "NL-I4",
                    "handoff did not activate the next Domain",
                )?;
                (index, LaneAction::Transfer)
            }
        };
        for (other, (before, next)) in self.lanes.iter().zip(&after.lanes).enumerate() {
            if index == other {
                before.check_step(step, next)?;
            } else {
                require(
                    before == next,
                    "NL-I7",
                    "a Domain changed another Domain's state",
                )?;
            }
        }
        Ok(())
    }

    fn terminal(&self) -> bool {
        self.latest + 1 == N && self.lanes.iter().all(Protocol::terminal)
    }
}

/// Settlement orders for three Domains. Each scalar starts the next Domain
/// after its retirement. Each second receipt can settle after its own scalar.
#[must_use]
pub fn chain_orders() -> Vec<[usize; 6]> {
    settlement_orders([0, 1 << 0, 1 << 0, 1 << 2, 1 << 2, 1 << 4])
}

/// A scalar and a merged pair in each of two Domains. Each successor
/// starts after its predecessor's scalar, and each pair settles after
/// its own Domain's scalar.
#[must_use]
pub fn consecutive_orders() -> Vec<[usize; 6]> {
    settlement_orders([0, 1 << 0, 1 << 0, 1 << 0, 1 << 3, 1 << 3])
}

fn settlement_orders(prerequisites: [u8; 6]) -> Vec<[usize; 6]> {
    fn extend(
        prerequisites: [u8; 6],
        prefix: [usize; 6],
        used: u8,
        depth: usize,
        result: &mut Vec<[usize; 6]>,
    ) {
        if depth == prefix.len() {
            result.push(prefix);
            return;
        }
        for (receipt, &required) in prerequisites.iter().enumerate() {
            let bit = 1 << receipt;
            if used & bit == 0 && used & required == required {
                let mut next = prefix;
                next[depth] = receipt;
                extend(prerequisites, next, used | bit, depth + 1, result);
            }
        }
    }
    let mut result = Vec::new();
    extend(prerequisites, [0; 6], 0, 0, &mut result);
    result
}
