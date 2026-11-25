use super::Decision;
use super::cohort::{Owner, append_call};
use std::sync::LazyLock;

/// The Owner can run between the two exchanges, or after the SDK's batch has notified.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BatchSchedule {
    Staggered,
    Together,
    TogetherDeferred,
    TogetherRetry,
}

impl BatchSchedule {
    #[must_use]
    pub const fn split_admission(self) -> bool {
        matches!(self, Self::TogetherDeferred | Self::TogetherRetry)
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct Batch {
    pub schedule: BatchSchedule,
    pub first_exchange: Option<usize>,
    pub leader: Option<usize>,
    pub taking: Option<(usize, usize)>,
    pub first_settle: Option<usize>,
}

impl Batch {
    pub fn new(schedule: BatchSchedule) -> Self {
        Self {
            schedule,
            first_exchange: None,
            leader: None,
            taking: None,
            first_settle: None,
        }
    }

    pub fn duplicate(&self) -> Option<usize> {
        self.leader.map(|g| 3 - g)
    }
}

static PLANS: LazyLock<[Vec<Owner>; 4]> = LazyLock::new(|| {
    std::array::from_fn(|profile| {
        let controlled = profile % 2 == 1;
        let split_admission = profile >= 2;
        let mut plan = Vec::new();
        append_call(&mut plan, 0, 2, false);
        plan.push(Owner::Registered(0));
        if controlled {
            plan.push(Owner::Control(0, 0, Decision::Cancelled));
        }
        append_call(&mut plan, 1, 2, false);
        plan.extend([Owner::Registered(1), Owner::Wait(0), Owner::Retire(0)]);
        if split_admission {
            plan.push(Owner::BindPoll);
        }
        append_call(&mut plan, 2, 2, false);
        plan.push(Owner::Registered(2));
        if !split_admission {
            plan.push(Owner::BindPoll);
        }
        plan.extend([
            Owner::Late(0, Decision::Cancelled),
            Owner::Late(1, Decision::TimedOut),
            Owner::Alias,
            Owner::Status,
        ]);
        if split_admission && controlled {
            plan.push(Owner::Control(2, 1, Decision::TimedOut));
        }
        plan.extend([Owner::DrainAll, Owner::DropAliases, Owner::End]);
        plan
    })
});

pub(super) fn owner(schedule: BatchSchedule, controlled: bool, pc: usize) -> Owner {
    PLANS[2 * usize::from(schedule.split_admission()) + usize::from(controlled)][pc]
}
