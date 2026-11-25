use super::Protocol;
use super::search::{Violation, require};

const RECEIPTS: usize = 3;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ReadyInput {
    Candidates,
    Packets,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Turn {
    Reading,
    Selected,
    Finished,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadyAction {
    Publish(usize),
    Observe(usize),
    Select,
    Finish,
}

/// One Owner turn with three receipt identities, numbered oldest to newest.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ReadySelection {
    input: ReadyInput,
    physical: u8,
    observed: u8,
    next_candidate: usize,
    turn: Turn,
    selected: Option<usize>,
}

impl ReadySelection {
    #[must_use]
    pub const fn new(input: ReadyInput) -> Self {
        Self {
            input,
            physical: 0,
            observed: 0,
            next_candidate: RECEIPTS,
            turn: Turn::Reading,
            selected: None,
        }
    }

    #[must_use]
    pub const fn selection(&self) -> Option<usize> {
        self.selected
    }

    #[must_use]
    pub fn observed(&self, receipt: usize) -> bool {
        self.observed & (1 << receipt) != 0
    }

    fn oldest_observed(&self) -> Option<usize> {
        (self.observed != 0).then_some(self.observed.trailing_zeros() as usize)
    }
}

impl Protocol for ReadySelection {
    type Action = ReadyAction;

    fn name(&self) -> String {
        format!("three-receipt-selection-from-{:?}", self.input)
    }

    fn actions(&self) -> Vec<Self::Action> {
        use ReadyAction as A;
        if self.terminal() {
            return Vec::new();
        }
        let mut actions = Vec::new();
        for i in 0..RECEIPTS {
            if self.physical & (1 << i) == 0 {
                actions.push(A::Publish(i));
            }
        }
        if self.turn == Turn::Selected {
            actions.push(A::Finish);
            return actions;
        }
        match self.input {
            ReadyInput::Candidates => {
                if self.next_candidate == 0 {
                    actions.push(A::Select);
                } else {
                    actions.push(A::Observe(self.next_candidate - 1));
                }
            }
            ReadyInput::Packets => {
                let unread = self.physical & !self.observed;
                for i in 0..RECEIPTS {
                    if unread & (1 << i) != 0 {
                        actions.push(A::Observe(i));
                    }
                }
                if unread == 0 {
                    actions.push(A::Select);
                }
            }
        }
        actions
    }

    fn apply(&mut self, action: Self::Action) {
        use ReadyAction as A;
        match action {
            A::Publish(i) => self.physical |= 1 << i,
            A::Observe(i) => {
                self.observed |= self.physical & (1 << i);
                if self.input == ReadyInput::Candidates {
                    self.next_candidate -= 1;
                }
            }
            A::Select => {
                self.selected = self.oldest_observed();
                self.turn = Turn::Selected;
            }
            A::Finish => self.turn = Turn::Finished,
        }
    }

    fn check(&self) -> Result<(), Violation> {
        require(
            self.observed & !self.physical == 0,
            "NL-I2",
            "observed a receipt before physical publication",
        )?;
        require(
            self.turn == Turn::Reading || self.selection() == self.oldest_observed(),
            "NL-I2",
            "changed the selected receipt after completing its observations",
        )
    }

    fn check_step(&self, action: Self::Action, after: &Self) -> Result<(), Violation> {
        if matches!(action, ReadyAction::Publish(_)) {
            require(
                self.observed == after.observed && self.selected == after.selected,
                "NL-I2",
                "late publication retroactively changed an Owner observation",
            )?;
        }
        Ok(())
    }

    fn terminal(&self) -> bool {
        self.turn == Turn::Finished
    }

    fn witnesses(&self) -> Vec<&'static str> {
        let mut witnesses = Vec::new();
        if self
            .selection()
            .is_some_and(|id| (self.physical.trailing_zeros() as usize) < id)
        {
            witnesses.push("older-publication-outside-the-selected-observations");
        }
        if self.turn != Turn::Reading && self.selection().is_none() && self.physical != 0 {
            witnesses.push("late-publication-after-an-empty-turn");
        }
        witnesses
    }
}
