use super::search::{Protocol, Violation, require};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Decision {
    Pending,
    Completed,
    Cancelled,
    TimedOut,
}
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ReceiptStage {
    Absent,
    Waiting,
    Copying,
    Acquired,
    OwnerSettled,
    Released,
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Helper {
    None,
    Running,
    Returned,
    Joined,
}
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum CompletionFault {
    #[default]
    None,
    OverwriteWinner,
    AcquireWithoutPublication,
    CancelCompletesPhysical,
    RetireOnPrefix,
    RetireAfterOneCopy,
    SettleBeforeJoin,
    DoubleDiscard,
    LoseLateResult,
    ReadReusedCarrier,
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Publication, access closure, and retirement are independent"
)]
struct Carrier {
    written: bool,
    prefix: bool,
    published: bool,
    closed: bool,
    retired: bool,
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct Receipt {
    generation: u8,
    stage: ReceiptStage,
    decision: Decision,
    first_decision: Decision,
    helper: Helper,
    control_access: bool,
    copied: Option<u8>,
    takes: u8,
    discards: u8,
    releases: u8,
}
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Completion {
    version: u8,
    waiters: u8,
    generations: u8,
    dedicated: bool,
    cooperative: bool,
    current: u8,
    carriers: Vec<Carrier>,
    receipts: Vec<Receipt>,
    read_old_alias: Option<(u8, u8)>,
    pub fault: CompletionFault,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionAction {
    Admit(usize),
    Write,
    PrefixDone,
    Publish,
    CloseAdmission,
    RetireCarrier,
    Reuse,
    Control(usize, Decision),
    EndControl(usize),
    BeginCopy(usize),
    FinishCopy(usize),
    HelperReturn(usize),
    Join(usize),
    Settle(usize),
    Release(usize),
    ReadOldAlias(usize),
}
impl Completion {
    /// Bounds are explicit: at most two receipts total and two generations.
    ///
    /// # Panics
    /// Panics for an unsupported ABI version or bounds outside this finite model.
    #[must_use]
    pub fn new(
        version: u8,
        waiters: u8,
        generations: u8,
        dedicated: bool,
        cooperative: bool,
    ) -> Self {
        assert!((1..=3).contains(&version));
        assert!(
            (1..=2).contains(&waiters)
                && (1..=2).contains(&generations)
                && waiters * generations <= 2
        );
        Self {
            version,
            waiters,
            generations,
            dedicated,
            cooperative,
            current: 0,
            carriers: vec![
                Carrier {
                    written: false,
                    prefix: false,
                    published: false,
                    closed: false,
                    retired: false
                };
                usize::from(generations)
            ],
            receipts: (0..waiters * generations)
                .map(|i| Receipt {
                    generation: i / waiters,
                    stage: ReceiptStage::Absent,
                    decision: Decision::Pending,
                    first_decision: Decision::Pending,
                    helper: Helper::None,
                    control_access: false,
                    copied: None,
                    takes: 0,
                    discards: 0,
                    releases: 0,
                })
                .collect(),
            read_old_alias: None,
            fault: CompletionFault::None,
        }
    }
    #[must_use]
    pub fn decision(&self, index: usize) -> Decision {
        self.receipts[index].decision
    }
    #[must_use]
    pub fn stage(&self, index: usize) -> ReceiptStage {
        self.receipts[index].stage
    }
    pub(super) fn published(&self) -> bool {
        self.carrier().published
    }
    pub(super) fn written(&self) -> bool {
        self.carrier().written
    }
    pub(super) fn prefix(&self) -> bool {
        self.carrier().prefix
    }
    pub(super) fn retired(&self) -> bool {
        self.carrier().retired
    }
    pub(super) fn copied(&self, index: usize) -> bool {
        self.receipts[index].copied.is_some()
    }
    pub(super) fn copy_payload(&mut self, index: usize) {
        let r = &mut self.receipts[index];
        r.copied = Some(r.generation);
        r.takes += 1;
    }
    pub(super) fn claim_completion(&mut self, index: usize) {
        Self::decide(&mut self.receipts[index], Decision::Completed);
    }
    fn carrier(&self) -> &Carrier {
        &self.carriers[usize::from(self.current)]
    }
    fn all_copied(&self) -> bool {
        self.receipts
            .iter()
            .filter(|r| r.generation == self.current)
            .all(|r| r.stage >= ReceiptStage::Acquired && !r.control_access)
    }
    fn helper_ready(r: &Receipt) -> bool {
        matches!(r.helper, Helper::None | Helper::Joined)
    }
    fn decide(r: &mut Receipt, decision: Decision) {
        if r.decision == Decision::Pending {
            r.decision = decision;
            r.first_decision = decision;
        }
    }
    /// Same public request may be rejected without accessing an old carrier.
    ///
    /// # Panics
    /// Panics for an invalid receipt index, unsupported control kind, or an
    /// attempt to control a pending receipt that has not been admitted.
    pub fn control(&mut self, index: usize, decision: Decision) -> bool {
        assert!(matches!(decision, Decision::Cancelled | Decision::TimedOut));
        let r = &mut self.receipts[index];
        if r.decision != Decision::Pending {
            return false;
        }
        assert!(r.stage != ReceiptStage::Absent && r.stage != ReceiptStage::Released);
        Self::decide(r, decision);
        r.control_access = true;
        true
    }
}
impl Protocol for Completion {
    type Action = CompletionAction;
    fn name(&self) -> String {
        format!(
            "completion-v{}-w{}-g{}-helper{}-cooperative{}",
            self.version, self.waiters, self.generations, self.dedicated, self.cooperative
        )
    }
    fn actions(&self) -> Vec<Self::Action> {
        use CompletionAction as A;
        let mut a = Vec::new();
        let c = self.carrier();
        if !c.retired {
            if !c.published {
                if !c.written {
                    a.push(A::Write);
                }
                if self.version > 1 && !c.prefix {
                    a.push(A::PrefixDone);
                }
                if c.written && self.cooperative {
                    a.push(A::Publish);
                }
            }
            if !c.closed
                && self
                    .receipts
                    .iter()
                    .filter(|r| r.generation == self.current)
                    .all(|r| r.stage != ReceiptStage::Absent)
            {
                a.push(A::CloseAdmission);
            }
            let ordinary = c.published && c.closed && self.all_copied();
            let bad_prefix = self.fault == CompletionFault::RetireOnPrefix && c.prefix;
            let bad_one = self.fault == CompletionFault::RetireAfterOneCopy
                && c.published
                && c.closed
                && self
                    .receipts
                    .iter()
                    .any(|r| r.generation == self.current && r.stage >= ReceiptStage::Acquired);
            if ordinary || bad_prefix || bad_one {
                a.push(A::RetireCarrier);
            }
        } else if self.current + 1 < self.generations {
            a.push(A::Reuse);
        }
        for (i, r) in self.receipts.iter().enumerate() {
            match r.stage {
                ReceiptStage::Absent if r.generation == self.current && !c.closed && !c.retired => {
                    a.push(A::Admit(i));
                }
                ReceiptStage::Waiting
                    if self.carriers[usize::from(r.generation)].published
                        || self.fault == CompletionFault::AcquireWithoutPublication =>
                {
                    a.push(A::BeginCopy(i));
                }
                ReceiptStage::Copying => a.push(A::FinishCopy(i)),
                ReceiptStage::Acquired
                    if Self::helper_ready(r) || self.fault == CompletionFault::SettleBeforeJoin =>
                {
                    a.push(A::Settle(i));
                }
                ReceiptStage::OwnerSettled if !r.control_access => a.push(A::Release(i)),
                _ => {}
            }
            if r.stage >= ReceiptStage::Waiting && r.stage < ReceiptStage::Released {
                a.push(A::Control(i, Decision::Cancelled));
                a.push(A::Control(i, Decision::TimedOut));
            }
            if r.control_access {
                a.push(A::EndControl(i));
            }
            if r.stage >= ReceiptStage::Acquired {
                if r.helper == Helper::Running {
                    a.push(A::HelperReturn(i));
                }
                if r.helper == Helper::Returned {
                    a.push(A::Join(i));
                }
            }
            if self.current > r.generation
                && r.stage == ReceiptStage::OwnerSettled
                && r.decision == Decision::Completed
                && self.read_old_alias.is_none()
            {
                a.push(A::ReadOldAlias(i));
            }
        }
        a
    }
    fn apply(&mut self, a: Self::Action) {
        use CompletionAction as A;
        match a {
            A::Admit(i) => {
                self.receipts[i].stage = ReceiptStage::Waiting;
                self.receipts[i].helper = if self.dedicated {
                    Helper::Running
                } else {
                    Helper::None
                };
            }
            A::Write => self.carriers[usize::from(self.current)].written = true,
            A::PrefixDone => self.carriers[usize::from(self.current)].prefix = true,
            A::Publish => {
                let c = &mut self.carriers[usize::from(self.current)];
                c.published = true;
                c.prefix = true;
            }
            A::CloseAdmission => self.carriers[usize::from(self.current)].closed = true,
            A::RetireCarrier => self.carriers[usize::from(self.current)].retired = true,
            A::Reuse => self.current += 1,
            A::Control(i, d) => {
                if self.control(i, d) && self.fault == CompletionFault::CancelCompletesPhysical {
                    self.receipts[i].stage = ReceiptStage::Acquired;
                }
            }
            A::EndControl(i) => self.receipts[i].control_access = false,
            A::BeginCopy(i) => self.receipts[i].stage = ReceiptStage::Copying,
            A::FinishCopy(i) => {
                if !self.copied(i) {
                    self.copy_payload(i);
                }
                let r = &mut self.receipts[i];
                r.stage = ReceiptStage::Acquired;
                Self::decide(r, Decision::Completed);
                if self.fault == CompletionFault::OverwriteWinner {
                    r.decision = Decision::Completed;
                }
            }
            A::HelperReturn(i) => self.receipts[i].helper = Helper::Returned,
            A::Join(i) => self.receipts[i].helper = Helper::Joined,
            A::Settle(i) => {
                let r = &mut self.receipts[i];
                r.stage = ReceiptStage::OwnerSettled;
                if r.decision != Decision::Completed
                    && self.fault != CompletionFault::LoseLateResult
                {
                    r.discards += 1;
                }
            }
            A::Release(i) => {
                let r = &mut self.receipts[i];
                r.stage = ReceiptStage::Released;
                r.releases += 1;
                if self.fault == CompletionFault::DoubleDiscard && r.decision != Decision::Completed
                {
                    r.discards += 1;
                }
            }
            A::ReadOldAlias(i) => {
                let r = &self.receipts[i];
                let value = if self.fault == CompletionFault::ReadReusedCarrier {
                    self.current
                } else {
                    r.copied.expect("Owner-settled receipt has a copied result")
                };
                self.read_old_alias = Some((r.generation, value));
            }
        }
    }
    fn check_step(&self, a: Self::Action, after: &Self) -> Result<(), Violation> {
        if let CompletionAction::Control(i, _) = a {
            require(
                self.carriers == after.carriers
                    && self.receipts.iter().zip(&after.receipts).all(|(x, y)| {
                        x.stage == y.stage && x.helper == y.helper && x.copied == y.copied
                    }),
                "NL-I3",
                "control changed a physical, collection, or join axis",
            )?;
            if self.receipts[i].decision != Decision::Pending {
                require(
                    self == after,
                    "NL-I1",
                    "rejected control modified an existing Future",
                )?;
            }
        }
        Ok(())
    }
    fn check(&self) -> Result<(), Violation> {
        for r in &self.receipts {
            let c = &self.carriers[usize::from(r.generation)];
            require(
                r.decision == r.first_decision,
                "NL-I1",
                "a decided winner changed",
            )?;
            if r.stage >= ReceiptStage::Copying {
                require(
                    c.published,
                    "NL-I2",
                    "copy/acquire occurred before SDK publication",
                )?;
            }
            if r.stage >= ReceiptStage::Acquired {
                require(
                    r.copied == Some(r.generation),
                    "NL-I2",
                    "physical_done preceded required result copying",
                )?;
                require(
                    r.decision != Decision::Pending,
                    "NL-I1",
                    "an acquired result has no terminal decision",
                )?;
                require(
                    r.takes == 1,
                    "NL-I10",
                    "acquired result has no unique owner",
                )?;
            }
            if r.stage >= ReceiptStage::OwnerSettled {
                require(
                    if self.dedicated {
                        r.helper == Helper::Joined
                    } else {
                        r.helper == Helper::None
                    },
                    "NL-I6",
                    "Owner settled or freed a helper record before join",
                )?;
                require(
                    r.discards == u8::from(r.decision != Decision::Completed),
                    "NL-I10",
                    "late result lacks exactly one responsible releaser",
                )?;
            }
            if r.stage == ReceiptStage::Released {
                require(
                    r.releases == 1,
                    "NL-I10",
                    "retired receipt did not release its credit exactly once",
                )?;
            }
            require(
                r.takes <= 1 && r.discards <= 1 && r.releases <= 1,
                "NL-I10",
                "result or credit ownership was consumed twice",
            )?;
            if c.retired {
                require(
                    c.published
                        && c.closed
                        && r.stage >= ReceiptStage::Acquired
                        && !r.control_access,
                    "NL-I4",
                    "carrier retired with a producer, waiter, or control access remaining",
                )?;
            }
        }
        if let Some((generation, value)) = self.read_old_alias {
            require(
                generation == value,
                "NL-I8",
                "old Future reread a reused carrier instead of its retained result",
            )?;
        }
        Ok(())
    }
    fn terminal(&self) -> bool {
        self.carriers.iter().all(|c| c.retired)
            && self
                .receipts
                .iter()
                .all(|r| r.stage == ReceiptStage::Released)
    }
    fn allowed_stall(&self) -> bool {
        !self.cooperative
            && !self.carrier().published
            && self
                .receipts
                .iter()
                .any(|r| r.stage == ReceiptStage::Waiting)
    }
    fn witnesses(&self) -> Vec<&'static str> {
        let mut w = Vec::new();
        if self.receipts.iter().any(|r| {
            matches!(r.decision, Decision::Cancelled | Decision::TimedOut)
                && r.stage == ReceiptStage::Waiting
        }) {
            w.push("controlled-but-physically-running");
        }
        if self
            .receipts
            .iter()
            .any(|r| r.stage == ReceiptStage::Acquired && r.helper == Helper::Running)
        {
            w.push("physical-done-before-helper-return");
        }
        if self
            .receipts
            .iter()
            .any(|r| r.control_access && self.carriers[usize::from(r.generation)].published)
        {
            w.push("publication-during-control-access");
        }
        if self.read_old_alias.is_some() {
            w.push("old-alias-read-after-carrier-reuse");
        }
        w
    }
}
