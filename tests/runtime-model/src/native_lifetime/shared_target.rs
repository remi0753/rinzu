use super::search::{Violation, require};
use super::{Binding, Protocol};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SharedTopology {
    Carriers,
    Reuse,
    Mixed,
    Coalesced,
    Consecutive,
}
impl SharedTopology {
    #[must_use]
    pub const fn count(self) -> usize {
        if matches!(self, Self::Mixed | Self::Consecutive) {
            3
        } else {
            2
        }
    }
    #[must_use]
    pub const fn storage(self, g: usize) -> usize {
        if matches!(self, Self::Reuse) || (matches!(self, Self::Mixed) && g == 2) {
            0
        } else {
            g
        }
    }
    #[must_use]
    pub const fn replacement(self) -> Option<usize> {
        match self {
            Self::Carriers | Self::Coalesced | Self::Consecutive => None,
            Self::Reuse => Some(1),
            Self::Mixed => Some(2),
        }
    }

    pub(super) const fn coalesced_leader(self) -> Option<usize> {
        match self {
            Self::Coalesced => Some(0),
            Self::Consecutive => Some(1),
            _ => None,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum Reg {
    Idle,
    Begun,
    Retained,
    Won,
    LostDone,
    LostTarget,
    RolledBack,
    Returned,
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum Sdk {
    None,
    Slot,
    Captured,
    Merged,
    Notify,
    Ready,
    Ended,
    ReleaseOpen,
    Decremented,
    Released,
}
impl Sdk {
    fn holds(self) -> bool {
        !matches!(self, Self::None | Self::Decremented | Self::Released)
    }
    fn notifying(self) -> bool {
        matches!(self, Self::Notify | Self::Ready)
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) enum Receipt {
    Absent,
    Waiting,
    Copied,
    Physical,
    Settled,
    Released,
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Slot {
    Empty,
    Target,
    Done,
}
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Carrier access, registration, references and notification observations are independent"
)]
pub(super) struct Generation {
    pub active: bool,
    pub reg: Reg,
    pub sdk: Sdk,
    pub receipt: Receipt,
    slot: Slot,
    pub prefix: bool,
    pub exchanged: bool,
    pub cas_seen: bool,
    pub publishing: u8,
    pub closed: bool,
    pub retired: bool,
    pub cancelled: bool,
    pub context: bool,
    pub woke: bool,
    pub sample: Option<u8>,
    retained: u8,
    rollback: u8,
    transferred: u8,
    released: u8,
}
impl Generation {
    fn new(active: bool) -> Self {
        Self {
            active,
            reg: Reg::Idle,
            sdk: Sdk::None,
            receipt: Receipt::Absent,
            slot: Slot::Empty,
            prefix: false,
            exchanged: false,
            cas_seen: false,
            publishing: 0,
            closed: false,
            retired: false,
            cancelled: false,
            context: false,
            woke: false,
            sample: None,
            retained: 0,
            rollback: 0,
            transferred: 0,
            released: 0,
        }
    }
    fn temporary(&self) -> bool {
        matches!(self.reg, Reg::Retained | Reg::LostDone | Reg::LostTarget)
    }
    fn registering(&self) -> bool {
        !matches!(self.reg, Reg::Idle | Reg::Returned)
    }
    pub fn published(&self) -> bool {
        self.slot == Slot::Done
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum SharedFault {
    #[default]
    None,
    FreeAfterOneRelease,
    DetachAfterOneCopy,
    FreeEnvironmentOnCancel,
    ForgetReferenceOnReuse,
    ReadReplacementValue,
    OldControlTouchesReplacement,
    CandidateDropConsumesOtherProducer,
    LateNotifyTouchesCarrier,
    MergeConsumesLeader,
    MergeConsumesEarlierBatch,
    NotifyBeforeCoveredPublication,
}
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Runtime ownership, context attachment, environment and target destruction differ"
)]
pub struct SharedTarget {
    pub(super) units: Vec<Generation>,
    pub(super) binding: Binding,
    pub(super) topology: SharedTopology,
    pub(super) shutdown: bool,
    pub(super) attached: bool,
    pub(super) runtime_reference: bool,
    pub(super) environment: bool,
    pub(super) target: bool,
    pub(super) worker: Option<usize>,
    pub(super) helper: u8,
    pub(super) control: Option<usize>,
    first_settled: Option<usize>,
    pub(super) alias: Option<usize>,
    pub(super) old_control: bool,
    pub fault: SharedFault,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SharedAction {
    Admit(usize),
    Begin(usize),
    Retain(usize),
    Cas(usize),
    CasObserved(usize),
    Rollback(usize),
    Sample(usize),
    Return(usize),
    PublishBegin(usize),
    Prefix(usize),
    Exchange(usize),
    ExchangeObserved(usize),
    PublishEnd(usize),
    Copy(usize),
    Physical(usize),
    Settle(usize),
    ReleaseRecord(usize),
    Close(usize),
    Retire(usize),
    Reuse,
    WakeBegin(usize),
    WakeEnd(usize),
    NotifyBegin(usize),
    NotifyReady(usize),
    NotifyEnd(usize),
    MergeWake,
    ReleaseBegin(usize),
    Decrement(usize),
    ReleaseEnd(usize),
    ControlBegin(usize),
    ControlEnd(usize),
    AliasRead,
    Shutdown,
    Quiescent,
    Join,
    Detach,
    RuntimeRelease,
    EnvironmentFree,
    TargetFree,
}
impl SharedTarget {
    #[must_use]
    pub fn new(binding: Binding, topology: SharedTopology) -> Self {
        Self {
            units: (0..topology.count())
                .map(|g| Generation::new(Some(g) != topology.replacement()))
                .collect(),
            binding,
            topology,
            shutdown: false,
            attached: true,
            runtime_reference: true,
            environment: true,
            target: true,
            worker: None,
            helper: if binding == Binding::CDirect { 3 } else { 0 },
            control: None,
            first_settled: None,
            alias: None,
            old_control: false,
            fault: SharedFault::None,
        }
    }
    #[must_use]
    pub fn references(&self) -> u8 {
        u8::from(self.runtime_reference)
            + self
                .units
                .iter()
                .map(|u| u8::from(u.temporary()) + u8::from(u.sdk.holds()))
                .sum::<u8>()
    }
    fn registering(&self) -> bool {
        self.units.iter().any(Generation::registering)
    }
    fn copying(&self) -> bool {
        self.units.iter().any(|u| u.receipt == Receipt::Copied)
    }
    fn physical(&self) -> bool {
        self.units
            .iter()
            .all(|u| u.active && u.receipt >= Receipt::Physical)
    }
    fn context_busy(&self) -> bool {
        self.units.iter().any(|u| u.context)
    }
    pub(super) fn enabled(&self, a: SharedAction) -> bool {
        self.actions().contains(&a)
    }
    pub(super) fn snapshot(&self, g: usize) -> Option<(bool, i64)> {
        match self.units[g].reg {
            Reg::Won => Some((true, 0)),
            Reg::LostDone => Some((false, 1)),
            Reg::LostTarget => Some((false, 2)),
            _ => None,
        }
    }
    pub(super) fn accepted_control(&self, g: usize) -> bool {
        !(g == 0 && self.topology.replacement().is_some())
    }
    pub(super) fn captured(&self, g: usize) -> bool {
        self.units[g].transferred == 1
    }
    #[allow(
        clippy::too_many_lines,
        reason = "Explicit transition table keeps each generation guard next to its action"
    )]
    fn generation_actions(&self, g: usize, a: &mut Vec<SharedAction>) {
        use SharedAction as A;
        let u = &self.units[g];
        if !u.active {
            return;
        }
        let leader = self
            .topology
            .coalesced_leader()
            .filter(|&leader| g >= leader);
        if u.receipt == Receipt::Absent
            && !self.shutdown
            && (g == 0
                || if leader.is_some_and(|leader| g > leader) {
                    self.units[g - 1].reg == Reg::Returned
                } else {
                    self.units[g - 1].retired
                        && self.units[g - 1].receipt >= Receipt::Settled
                        && (self.binding != Binding::RustChannel || self.units[g - 1].woke)
                })
        {
            a.push(A::Admit(g));
        }
        let owner_free =
            self.binding == Binding::RustChannel || (!self.copying() && self.control.is_none());
        if u.receipt == Receipt::Waiting
            && u.reg == Reg::Idle
            && !self.registering()
            && owner_free
            && self.worker.is_none()
        {
            a.push(A::Begin(g));
        }
        match u.reg {
            Reg::Begun => a.push(A::Retain(g)),
            Reg::Retained => a.push(A::Cas(g)),
            Reg::Won | Reg::LostDone | Reg::LostTarget if !u.cas_seen => a.push(A::CasObserved(g)),
            Reg::LostDone | Reg::LostTarget if u.cas_seen => a.push(A::Rollback(g)),
            _ => {}
        }
        if u.cas_seen && matches!(u.reg, Reg::Won | Reg::RolledBack) {
            if u.sample.is_none() {
                a.push(A::Sample(g));
            } else {
                a.push(A::Return(g));
            }
        }
        if u.publishing == 0
            && u.receipt != Receipt::Absent
            && leader.is_none_or(|leader| {
                self.units[leader..].iter().all(|v| v.reg == Reg::Returned)
                    && (g == leader || self.units[leader].exchanged)
            })
        {
            a.push(A::PublishBegin(g));
        }
        if u.publishing == 1 && !u.prefix {
            a.push(A::Prefix(g));
        }
        if u.prefix && !u.published() {
            a.push(A::Exchange(g));
        }
        if u.published() && !u.exchanged {
            a.push(A::ExchangeObserved(g));
        }
        if u.publishing == 1 && u.exchanged && matches!(u.sdk, Sdk::None | Sdk::Released) {
            a.push(A::PublishEnd(g));
        }
        let worker_ready_to_copy = self.binding != Binding::RustChannel
            || self.topology.coalesced_leader().is_none()
            || self
                .units
                .iter()
                .all(|v| v.receipt < Receipt::Physical || v.woke);
        if u.receipt == Receipt::Waiting
            && u.reg == Reg::Returned
            && u.published()
            && !self.registering()
            && !self.copying()
            && self.worker.is_none()
            && worker_ready_to_copy
            && self.control != Some(g)
        {
            a.push(A::Copy(g));
        }
        if u.receipt == Receipt::Copied {
            a.push(A::Physical(g));
        }
        if u.receipt == Receipt::Physical && self.control != Some(g) {
            a.push(A::Settle(g));
        }
        let old_alias = g == 0 && self.topology.replacement().is_some();
        if u.receipt == Receipt::Settled
            && self.physical()
            && (self.binding != Binding::RustChannel || u.woke)
            && (!old_alias || (self.alias.is_some() && self.old_control))
            && self.control != Some(g)
        {
            a.push(A::ReleaseRecord(g));
        }
        if u.reg == Reg::Returned && !u.closed {
            a.push(A::Close(g));
        }
        if u.closed
            && u.published()
            && u.receipt >= Receipt::Physical
            && !u.retired
            && self.control != Some(g)
        {
            a.push(A::Retire(g));
        }
        if self.binding == Binding::RustChannel
            && u.receipt >= Receipt::Physical
            && !u.woke
            && self.worker.is_none()
            && self.helper == 0
            && !self.registering()
            && !self.copying()
        {
            a.push(A::WakeBegin(g));
        }
        if self.worker == Some(g) {
            a.push(A::WakeEnd(g));
        }
        let gate_free =
            self.binding == Binding::RustChannel || !self.units.iter().any(|v| v.sdk.notifying());
        let can_notify = leader.is_none_or(|leader| {
            g == leader
                && (self.units[leader + 1].sdk == Sdk::Released
                    || self.fault == SharedFault::NotifyBeforeCoveredPublication)
        });
        if u.sdk == Sdk::Captured && u.exchanged && gate_free && can_notify {
            a.push(A::NotifyBegin(g));
        }
        if u.sdk == Sdk::Notify && u.context && !self.registering() && !self.copying() {
            a.push(A::NotifyReady(g));
        }
        if u.sdk == Sdk::Ready || (u.sdk == Sdk::Notify && !u.context) {
            a.push(A::NotifyEnd(g));
        }
        match u.sdk {
            Sdk::Ended | Sdk::Merged => a.push(A::ReleaseBegin(g)),
            Sdk::ReleaseOpen => a.push(A::Decrement(g)),
            Sdk::Decremented => a.push(A::ReleaseEnd(g)),
            _ => {}
        }
        if self.control.is_none()
            && (self.binding == Binding::RustChannel || (!self.registering() && !self.copying()))
        {
            let fresh = g == self.units.len() - 1
                && u.reg == Reg::Returned
                && !u.published()
                && !u.cancelled;
            let late = old_alias
                && u.receipt == Receipt::Settled
                && self.alias.is_some()
                && !self.old_control;
            if fresh || late {
                a.push(A::ControlBegin(g));
            }
        }
        if self.control == Some(g) {
            a.push(A::ControlEnd(g));
        }
    }
}
impl Protocol for SharedTarget {
    type Action = SharedAction;
    fn name(&self) -> String {
        format!("shared-{:?}-{:?}", self.binding, self.topology)
    }
    fn actions(&self) -> Vec<Self::Action> {
        use SharedAction as A;
        let mut a = Vec::new();
        for g in 0..self.units.len() {
            self.generation_actions(g, &mut a);
        }
        if let Some(leader) = self.topology.coalesced_leader()
            && self.units[leader..]
                .iter()
                .all(|u| u.exchanged && u.sdk == Sdk::Captured)
        {
            a.push(A::MergeWake);
        }
        if let Some(next) = self.topology.replacement() {
            if !self.units[next].active
                && self.units[0].retired
                && self.units[next - 1].receipt >= Receipt::Physical
            {
                a.push(A::Reuse);
            }
            if self.units[next].active
                && self.units[0].receipt == Receipt::Settled
                && self.alias.is_none()
            {
                a.push(A::AliasRead);
            }
        }
        if !self.shutdown
            && self
                .units
                .iter()
                .all(|u| u.active && u.receipt != Receipt::Absent)
            && (self.binding == Binding::RustChannel || !self.registering())
        {
            a.push(A::Shutdown);
        }
        if self.shutdown
            && self.physical()
            && self.worker.is_none()
            && self.helper == 0
            && (self.binding != Binding::RustChannel || self.units.iter().all(|u| u.woke))
        {
            a.push(A::Quiescent);
        }
        if self.helper == 2 {
            a.push(A::Join);
        }
        let helper = self.helper == 3 || (self.binding == Binding::RustChannel && self.helper == 2);
        let ready = self.physical()
            || (self.fault == SharedFault::DetachAfterOneCopy
                && self.units.iter().any(|u| u.receipt >= Receipt::Physical));
        if self.attached && self.shutdown && ready && helper && !self.context_busy() {
            a.push(A::Detach);
        }
        if !self.attached && self.runtime_reference {
            a.push(A::RuntimeRelease);
        }
        if self.environment && !self.attached && !self.context_busy() && self.worker.is_none() {
            a.push(A::EnvironmentFree);
        }
        let released_one = self.fault == SharedFault::FreeAfterOneRelease
            && self.units.iter().any(|u| u.released == 1);
        if self.target && !self.runtime_reference && (self.references() == 0 || released_one) {
            a.push(A::TargetFree);
        }
        a
    }
    #[allow(
        clippy::too_many_lines,
        reason = "One branch per explicit protocol transition"
    )]
    fn apply(&mut self, a: Self::Action) {
        use SharedAction as A;
        match a {
            A::Admit(g) => self.units[g].receipt = Receipt::Waiting,
            A::Begin(g) => self.units[g].reg = Reg::Begun,
            A::Retain(g) => {
                self.units[g].reg = Reg::Retained;
                self.units[g].retained += 1;
            }
            A::Cas(g) => {
                let u = &mut self.units[g];
                if u.slot == Slot::Empty {
                    u.slot = Slot::Target;
                    u.sdk = Sdk::Slot;
                    u.transferred += 1;
                    u.reg = Reg::Won;
                } else {
                    u.reg = if u.slot == Slot::Done {
                        Reg::LostDone
                    } else {
                        Reg::LostTarget
                    };
                }
            }
            A::CasObserved(g) => self.units[g].cas_seen = true,
            A::Rollback(g) => {
                self.units[g].reg = Reg::RolledBack;
                self.units[g].rollback += 1;
                if self.fault == SharedFault::CandidateDropConsumesOtherProducer
                    && g > 0
                    && self.units[0].sdk.holds()
                {
                    self.units[0].sdk = Sdk::Released;
                }
            }
            A::Sample(g) => self.units[g].sample = Some(self.references()),
            A::Return(g) => {
                self.units[g].reg = Reg::Returned;
                self.units[g].sample = None;
            }
            A::PublishBegin(g) => self.units[g].publishing = 1,
            A::Prefix(g) => self.units[g].prefix = true,
            A::Exchange(g) => {
                let u = &mut self.units[g];
                if u.slot == Slot::Target {
                    u.sdk = Sdk::Captured;
                }
                u.slot = Slot::Done;
            }
            A::ExchangeObserved(g) => self.units[g].exchanged = true,
            A::PublishEnd(g) => self.units[g].publishing = 2,
            A::Copy(g) => self.units[g].receipt = Receipt::Copied,
            A::Physical(g) => self.units[g].receipt = Receipt::Physical,
            A::Settle(g) => {
                self.units[g].receipt = Receipt::Settled;
                if self.topology == SharedTopology::Coalesced
                    && self.physical()
                    && self.units[1 - g].receipt < Receipt::Settled
                {
                    self.first_settled = Some(g);
                }
            }
            A::ReleaseRecord(g) => self.units[g].receipt = Receipt::Released,
            A::Close(g) => self.units[g].closed = true,
            A::Retire(g) => self.units[g].retired = true,
            A::Reuse => {
                let next = self.topology.replacement().unwrap();
                self.units[next].active = true;
                if self.fault == SharedFault::ForgetReferenceOnReuse && self.units[0].sdk.holds() {
                    self.units[0].sdk = Sdk::Released;
                    self.units[0].released += 1;
                }
            }
            A::WakeBegin(g) => self.worker = Some(g),
            A::WakeEnd(g) => {
                self.worker = None;
                self.units[g].woke = true;
            }
            A::NotifyBegin(g) => {
                self.units[g].sdk = Sdk::Notify;
                self.units[g].context = self.attached && self.binding != Binding::RustChannel;
                if self.fault == SharedFault::LateNotifyTouchesCarrier
                    && g == 0
                    && let Some(next) = self.topology.replacement()
                    && self.units[next].active
                {
                    self.units[next].slot = Slot::Done;
                }
            }
            A::NotifyReady(g) => self.units[g].sdk = Sdk::Ready,
            A::NotifyEnd(g) => {
                self.units[g].sdk = Sdk::Ended;
                self.units[g].context = false;
            }
            A::MergeWake => {
                let leader = self.topology.coalesced_leader().unwrap();
                self.units[leader + 1].sdk = Sdk::Merged;
                if self.fault == SharedFault::MergeConsumesLeader {
                    self.units[leader].sdk = Sdk::Released;
                }
                if self.fault == SharedFault::MergeConsumesEarlierBatch
                    && leader > 0
                    && self.units[0].sdk == Sdk::Captured
                {
                    self.units[0].sdk = Sdk::Released;
                    self.units[0].released += 1;
                }
            }
            A::ReleaseBegin(g) => self.units[g].sdk = Sdk::ReleaseOpen,
            A::Decrement(g) => {
                self.units[g].sdk = Sdk::Decremented;
                self.units[g].released += 1;
            }
            A::ReleaseEnd(g) => self.units[g].sdk = Sdk::Released,
            A::ControlBegin(g) => self.control = Some(g),
            A::ControlEnd(g) => {
                self.control = None;
                if g == 0 && self.topology.replacement().is_some() {
                    self.old_control = true;
                    if self.fault == SharedFault::OldControlTouchesReplacement {
                        self.units[self.topology.replacement().unwrap()].cancelled = true;
                    }
                } else {
                    self.units[g].cancelled = true;
                    if self.fault == SharedFault::FreeEnvironmentOnCancel {
                        self.environment = false;
                    }
                }
            }
            A::AliasRead => {
                self.alias = Some(if self.fault == SharedFault::ReadReplacementValue {
                    self.topology.replacement().unwrap()
                } else {
                    0
                });
            }
            A::Shutdown => self.shutdown = true,
            A::Quiescent => self.helper = 2,
            A::Join => self.helper = 3,
            A::Detach => self.attached = false,
            A::RuntimeRelease => self.runtime_reference = false,
            A::EnvironmentFree => self.environment = false,
            A::TargetFree => self.target = false,
        }
    }
    fn check(&self) -> Result<(), Violation> {
        require(
            self.target || self.references() == 0,
            "NL-I7",
            "shared target freed before the last aggregate reference",
        )?;
        require(
            self.attached || (self.physical() && !self.context_busy()),
            "NL-I5",
            "shared context detached before all generations drained",
        )?;
        require(
            self.environment || (!self.attached && !self.context_busy() && self.worker.is_none()),
            "NL-I5",
            "shared environment freed with a live reader",
        )?;
        require(
            self.alias.is_none_or(|g| g == 0),
            "NL-I8",
            "old alias read the replacement generation",
        )?;
        require(
            self.units.iter().filter(|u| u.registering()).count() <= 1,
            "NL-I7",
            "registrars unexpectedly overlap",
        )?;
        if self.binding != Binding::RustChannel {
            require(
                self.units.iter().filter(|u| u.sdk.notifying()).count() <= 1,
                "NL-I7",
                "shared target gate admitted two notifications",
            )?;
        }
        for u in &self.units {
            require(
                u.retained == u8::from(u.temporary()) + u.rollback + u.transferred,
                "NL-I7",
                "candidate reference conservation failed",
            )?;
            require(
                u.transferred <= 1
                    && u.released <= 1
                    && u.transferred == u8::from(u.sdk.holds()) + u.released,
                "NL-I7",
                "generation lost or duplicated its SDK reference",
            )?;
            require(
                !u.published() || u.prefix,
                "NL-I2",
                "publication preceded its required helper prefix",
            )?;
            require(
                u.receipt < Receipt::Copied || (u.published() && u.reg == Reg::Returned),
                "NL-I2",
                "receipt copy bypassed publication/registration",
            )?;
            require(
                !u.retired
                    || (u.published()
                        && u.closed
                        && u.receipt >= Receipt::Physical
                        && u.reg == Reg::Returned),
                "NL-I4",
                "carrier retired before consumer access ended",
            )?;
            require(
                !u.context || (self.environment && self.attached),
                "NL-I7",
                "native notification used a detached context",
            )?;
        }
        Ok(())
    }
    fn check_step(&self, a: Self::Action, after: &Self) -> Result<(), Violation> {
        use SharedAction as A;
        if a == A::MergeWake {
            let leader = self.topology.coalesced_leader().unwrap();
            require(
                self.units[..=leader] == after.units[..=leader],
                "NL-I7",
                "merge consumed a leader or a reference from an earlier batch",
            )?;
        }
        if let A::ReleaseBegin(g) | A::Decrement(g) | A::ReleaseEnd(g) = a {
            require(
                self.units
                    .iter()
                    .zip(&after.units)
                    .enumerate()
                    .all(|(index, (before, next))| index == g || before == next),
                "NL-I7",
                "SDK return changed another generation",
            )?;
        }
        if let A::NotifyBegin(g) = a
            && let Some(leader) = self.topology.coalesced_leader()
            && g >= leader
        {
            require(
                g == leader
                    && self.units[leader + 1].exchanged
                    && self.units[leader + 1].sdk == Sdk::Released,
                "NL-I7",
                "batch notified before the covered exchange and duplicate release",
            )?;
        }
        if a == A::Reuse {
            require(
                self.units[0] == after.units[0],
                "NL-I7",
                "storage reuse consumed the old SDK reference",
            )?;
        }
        if let A::NotifyBegin(_)
        | A::NotifyReady(_)
        | A::NotifyEnd(_)
        | A::ReleaseBegin(_)
        | A::Decrement(_)
        | A::ReleaseEnd(_)
        | A::MergeWake = a
        {
            require(
                self.units.iter().zip(&after.units).all(|(x, y)| {
                    x.slot == y.slot && x.receipt == y.receipt && x.cancelled == y.cancelled
                }),
                "NL-I4",
                "late producer accessed a reused carrier",
            )?;
        }
        if let A::ControlEnd(g) = a {
            require(
                self.environment == after.environment
                    && self
                        .units
                        .iter()
                        .zip(&after.units)
                        .all(|(x, y)| x.sdk == y.sdk && x.receipt == y.receipt),
                "NL-I3",
                "control changed shared physical/reference lifetime",
            )?;
            if g == 0 && self.topology.replacement().is_some() {
                require(
                    self.units == after.units,
                    "NL-I8",
                    "old control signalled the replacement carrier",
                )?;
            }
        }
        Ok(())
    }
    fn terminal(&self) -> bool {
        !self.target
            && !self.environment
            && self.units.iter().all(|u| {
                u.active
                    && u.retired
                    && u.receipt == Receipt::Released
                    && u.publishing == 2
                    && matches!(u.sdk, Sdk::None | Sdk::Released)
            })
            && (self.binding == Binding::CDirect || self.helper == 3)
    }
    fn witnesses(&self) -> Vec<&'static str> {
        let mut w = Vec::new();
        if let Some(leader) = self.topology.coalesced_leader()
            && self.units[leader + 1].sdk == Sdk::Released
            && self.units[leader].sdk == Sdk::Captured
        {
            w.push("duplicate-released-while-batch-target-retained");
        }
        if self.topology == SharedTopology::Coalesced
            && self.units.iter().all(|u| u.receipt >= Receipt::Settled)
        {
            match self.first_settled {
                Some(0) => w.push("leader-settled-first"),
                Some(1) => w.push("covered-receipt-settled-first"),
                _ => {}
            }
        }
        if self.references() >= 3 {
            w.push("more-than-two-target-references");
        }
        if self.references() == 4 {
            w.push("four-target-references");
        }
        if !self.runtime_reference && self.units.iter().filter(|u| u.sdk.holds()).count() >= 2 {
            w.push("two-producers-outlive-runtime");
        }
        if !self.environment && self.units.iter().any(|u| u.sdk.holds()) {
            w.push("sdk-reference-outlives-shared-environment");
        }
        if let Some(next) = self.topology.replacement() {
            if self.units[next].active && self.units[0].sdk.holds() {
                w.push("old-sdk-reference-survives-storage-reuse");
            }
            if self.units[next].active && self.units[0].publishing == 1 {
                w.push("reuse-before-old-publisher-return");
            }
        }
        w
    }
}
