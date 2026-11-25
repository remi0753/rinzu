use super::search::{Violation, require};
use super::{Domain, Protocol};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Slot {
    Empty,
    Installed(Domain),
    Done,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Phase {
    Waiting,
    Candidate,
    Won,
    LostDone,
    LostForeign,
    Returned,
    Rejected,
}

impl Phase {
    fn owns_candidate(self) -> bool {
        matches!(self, Self::Candidate | Self::LostDone | Self::LostForeign)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParallelRegistrationAction {
    Retain(Domain),
    Cas(Domain),
    Rollback(Domain),
    Exchange,
    ReleaseSdk,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ParallelRegistration {
    slot: Slot,
    phases: [Phase; 2],
    counts: [u8; 2],
    captured: Option<Domain>,
}

impl Default for ParallelRegistration {
    fn default() -> Self {
        Self {
            slot: Slot::Empty,
            phases: [Phase::Waiting; 2],
            counts: [1; 2],
            captured: None,
        }
    }
}

impl ParallelRegistration {
    #[must_use]
    pub fn references(&self, d: Domain) -> u8 {
        self.counts[d.index()]
    }

    #[must_use]
    pub fn published(&self) -> bool {
        self.slot == Slot::Done
    }

    #[must_use]
    pub fn rejected(&self) -> bool {
        self.phases.contains(&Phase::Rejected)
    }

    /// Actual CAS result encoding used by both lifetime probes.
    #[must_use]
    pub fn cas_result(&self, d: Domain) -> Option<(i64, i64)> {
        match self.phases[d.index()] {
            Phase::Won => Some((1, 0)),
            Phase::LostDone => Some((0, 1)),
            Phase::LostForeign => Some((0, 2)),
            _ => None,
        }
    }

    #[must_use]
    pub fn captured(&self) -> Option<Domain> {
        self.captured
    }
}

impl Protocol for ParallelRegistration {
    type Action = ParallelRegistrationAction;

    fn name(&self) -> String {
        "two-domain-registration-race".into()
    }

    fn actions(&self) -> Vec<Self::Action> {
        use ParallelRegistrationAction as A;
        let mut a = Vec::new();
        if self.phases.contains(&Phase::Rejected) {
            return a;
        }
        for d in Domain::ALL {
            match self.phases[d.index()] {
                Phase::Waiting => a.push(A::Retain(d)),
                Phase::Candidate => a.push(A::Cas(d)),
                Phase::LostDone | Phase::LostForeign => a.push(A::Rollback(d)),
                _ => {}
            }
        }
        if self.slot != Slot::Done {
            a.push(A::Exchange);
        }
        if self.captured.is_some() {
            a.push(A::ReleaseSdk);
        }
        a
    }

    fn apply(&mut self, action: Self::Action) {
        use ParallelRegistrationAction as A;
        match action {
            A::Retain(d) => {
                self.counts[d.index()] += 1;
                self.phases[d.index()] = Phase::Candidate;
            }
            A::Cas(d) => {
                self.phases[d.index()] = match self.slot {
                    Slot::Empty => {
                        self.slot = Slot::Installed(d);
                        Phase::Won
                    }
                    Slot::Installed(_) => Phase::LostForeign,
                    Slot::Done => Phase::LostDone,
                };
            }
            A::Rollback(d) => {
                self.counts[d.index()] -= 1;
                self.phases[d.index()] = if self.phases[d.index()] == Phase::LostForeign {
                    Phase::Rejected
                } else {
                    Phase::Returned
                };
            }
            A::Exchange => {
                if let Slot::Installed(d) = self.slot {
                    self.captured = Some(d);
                }
                self.slot = Slot::Done;
            }
            A::ReleaseSdk => self.counts[self.captured.take().unwrap().index()] -= 1,
        }
    }

    fn check(&self) -> Result<(), Violation> {
        for d in Domain::ALL {
            let transferred = self.slot == Slot::Installed(d) || self.captured == Some(d);
            let temporary = self.phases[d.index()].owns_candidate();
            require(
                self.counts[d.index()] == 1 + u8::from(transferred) + u8::from(temporary),
                "NL-I7",
                "candidate or SDK reference charged to the wrong Domain",
            )?;
        }
        require(
            self.phases.iter().filter(|&&p| p == Phase::Won).count() <= 1,
            "NL-I7",
            "both Domains installed a target",
        )
    }

    fn check_step(&self, action: Self::Action, after: &Self) -> Result<(), Violation> {
        use ParallelRegistrationAction as A;
        require(
            self.slot != Slot::Done || after.slot == Slot::Done,
            "NL-I2",
            "registration replaced DONE",
        )?;
        if let A::Rollback(d) = action {
            require(
                self.counts[d.other().index()] == after.counts[d.other().index()]
                    && self.slot == after.slot
                    && self.captured == after.captured,
                "NL-I7",
                "rollback changed a foreign target or the notification slot",
            )?;
        }
        Ok(())
    }

    fn terminal(&self) -> bool {
        self.phases.contains(&Phase::Rejected)
            || (self.slot == Slot::Done
                && self.captured.is_none()
                && self
                    .phases
                    .iter()
                    .all(|p| matches!(p, Phase::Won | Phase::Returned)))
    }

    fn witnesses(&self) -> Vec<&'static str> {
        let mut w = Vec::new();
        if self.phases == [Phase::Candidate; 2] && self.slot == Slot::Empty {
            w.push("overlapping-candidates");
        }
        if self.phases == [Phase::Returned; 2] {
            w.push("publication-before-both-cas-operations");
        }
        if self.phases.contains(&Phase::Returned) && self.captured.is_some() {
            w.push("publication-between-cas-operations-keeps-winner-reference");
        }
        if self.phases.contains(&Phase::Rejected) {
            w.push("foreign-cas-returns-only-losing-candidate");
        }
        w
    }
}
