use super::cohort::{Owner, append_call};
use super::{Binding, Decision};
use std::sync::LazyLock;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum Turn {
    Idle,
    Full,
    Retry,
    Taking(usize),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct Recovery {
    pub turn: Turn,
    pub order: [usize; 3],
}

impl Recovery {
    pub fn new() -> Self {
        Self {
            turn: Turn::Idle,
            order: [3; 3],
        }
    }

    pub fn received(&mut self, g: usize) {
        let next = self.order.iter_mut().find(|i| **i == 3).unwrap();
        *next = g;
        self.turn = Turn::Idle;
    }

    pub fn order_witness(&self) -> Option<&'static str> {
        Some(match self.order {
            [0, 1, 2] => "recovered-first-second-poll",
            [0, 2, 1] => "recovered-first-poll-second",
            [1, 0, 2] => "recovered-second-first-poll",
            [1, 2, 0] => "recovered-second-poll-first",
            [2, 0, 1] => "recovered-poll-first-second",
            [2, 1, 0] => "recovered-poll-second-first",
            _ => return None,
        })
    }
}

static PLANS: LazyLock<[Vec<Owner>; 3]> = LazyLock::new(|| {
    std::array::from_fn(|profile| {
        let mut plan = Vec::new();
        append_call(&mut plan, 0, 1, false);
        plan.extend([Owner::Registered(0), Owner::BindPoll]);
        append_call(&mut plan, 1, 1, false);
        match profile {
            0 => plan.push(Owner::Registered(1)),
            2 => plan.push(Owner::Control(1, 0, Decision::TimedOut)),
            _ => {}
        }
        plan.extend([Owner::DrainAll, Owner::End]);
        plan
    })
});

pub(super) fn owner(binding: Binding, controlled: bool, pc: usize) -> Owner {
    let profile = if binding == Binding::CJoined {
        0
    } else {
        1 + usize::from(controlled)
    };
    PLANS[profile][pc]
}
