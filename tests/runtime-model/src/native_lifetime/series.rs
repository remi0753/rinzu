use super::Decision;
use super::cohort::{Owner, append_call};
use std::sync::LazyLock;

/// Whether the Owner can collect and retire the first carrier before the
/// second exchange, or both exchanges precede collection in each batch.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MixedBatchSchedule {
    Staggered,
    Together,
    LateArrival,
}

pub(super) const VERSIONS: [u8; 4] = [3, 2, 3, 2];
pub(super) const RECEIPTS: [usize; 4] = [2, 1, 1, 1];
pub(super) const POLL_STORAGE: usize = 4;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(super) struct Round {
    pub first_exchange: Option<usize>,
    pub leader: Option<usize>,
    pub first_settle: Option<usize>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct Series {
    pub schedule: MixedBatchSchedule,
    pub rounds: [Round; 2],
    pub taking: Option<(usize, usize)>,
    pub late_selection: bool,
}

impl Series {
    pub fn new(schedule: MixedBatchSchedule) -> Self {
        Self {
            schedule,
            rounds: [Round::default(); 2],
            taking: None,
            late_selection: false,
        }
    }

    pub fn duplicate(&self, generation: usize) -> bool {
        self.rounds[generation / 2].leader == Some(generation ^ 1)
    }
}

static PLANS: LazyLock<[Vec<Owner>; 4]> = LazyLock::new(|| {
    std::array::from_fn(|profile| {
        let together = profile >= 2;
        let controlled = profile % 2 == 1;
        let mut plan = Vec::new();
        append_call(&mut plan, 0, RECEIPTS[0], false);
        plan.push(Owner::Registered(0));
        if controlled {
            plan.push(Owner::Control(0, 0, Decision::Cancelled));
        }
        append_call(&mut plan, 1, RECEIPTS[1], false);
        plan.extend([Owner::Registered(1), Owner::BindPoll]);
        if together {
            plan.push(Owner::Poll);
        }
        plan.push(Owner::Wave(0));
        for (g, &receipts) in RECEIPTS.iter().enumerate().skip(2) {
            append_call(&mut plan, g, receipts, false);
            plan.push(Owner::Registered(g));
        }
        plan.extend([
            Owner::Late(0, Decision::Cancelled),
            Owner::Late(1, Decision::TimedOut),
            Owner::Alias,
            Owner::Status,
            Owner::Wave(1),
            Owner::DropAliases,
            Owner::End,
        ]);
        plan
    })
});

pub(super) fn owner(schedule: MixedBatchSchedule, controlled: bool, pc: usize) -> Owner {
    PLANS[2 * usize::from(schedule == MixedBatchSchedule::Together) + usize::from(controlled)][pc]
}
