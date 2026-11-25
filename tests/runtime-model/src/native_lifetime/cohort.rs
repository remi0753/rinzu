//! Script-guided Owner product; SDK and the single reactor remain asynchronous.
use super::admission::{Action as A, Owner as Drain};
use super::batch::{self, Batch, BatchSchedule};
use super::call::Action as K;
use super::polling::{Polling, PollingAction as P};
use super::recovery::{self, Recovery, Turn};
use super::search::{Violation, require};
use super::series::{self, MixedBatchSchedule, Series};
use super::{
    Admission, Binding, Call, ComposedAction as C, Decision, Protocol, ReceiptStage as S,
    Registration, RegistrationAction as R,
};
use std::sync::LazyLock;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CohortLayout {
    Carriers,
    Reuse,
    Series,
}
impl CohortLayout {
    #[must_use]
    pub const fn count(self) -> usize {
        match self {
            Self::Carriers => 2,
            Self::Reuse => 3,
            Self::Series => 4,
        }
    }
    #[must_use]
    pub const fn storage(self, g: usize) -> usize {
        match g {
            2 => 0,
            3 => 2,
            _ => g,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum CohortMode {
    #[default]
    Registered,
    Early,
    Deferred,
    Mixed,
}
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum CohortFault {
    #[default]
    None,
    FreeWithOtherProducer,
    ReuseBeforeAllCopies,
    ReuseConsumesOldReference,
    CandidateConsumesProducer,
    DetachAfterOneGeneration,
    CallReleaseDropsPin,
    OldControlChangesReplacement,
    OldAliasReadsReplacement,
    RetryDropsDecision,
    SecondReceiptTransfersAgain,
    OverlapDetachedNotifications,
    DirectLosesDecision,
    DirectInventsReceipt,
    DetachDuringHelperWake,
    JoinDuringHelperWake,
    PollRetireBeforeCopy,
    PollRecordBeforeDrain,
    SdkDoneDropsCallEnvironment,
    PollRecordDuringWorkerWake,
    RetryWithoutCredit,
    PollSettleBeforePhysical,
    BatchNotifyBeforeDuplicateRelease,
    BatchReleaseConsumesOldReference,
    LateArrivalReplacesSelection,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Owner {
    Call(usize, K),
    Start(usize),
    Handoff(usize),
    Admit(usize, usize),
    Control(usize, usize, Decision),
    Wait(usize),
    Publish(usize),
    Poll,
    BindPoll,
    DrainAll,
    Wave(usize),
    Retire(usize),
    Registered(usize),
    Late(usize, Decision),
    Alias,
    Status,
    DropAliases,
    End,
}
pub(super) fn append_call(out: &mut Vec<Owner>, g: usize, receipts: usize, early: bool) {
    out.extend([
        Owner::Call(g, K::Enter),
        Owner::Start(g),
        Owner::Call(g, K::Admit(0)),
        Owner::Call(g, K::Begin(0)),
    ]);
    if early {
        out.push(Owner::Publish(g));
    }
    for k in [
        K::ReturnCallback(0),
        K::SettleCallback(0),
        K::ReturnNative,
        K::CloseGate,
        K::SettleGate,
        K::ReadResult,
    ] {
        out.push(Owner::Call(g, k));
    }
    out.extend([
        Owner::Handoff(g),
        Owner::Call(g, K::FinishContinuation),
        Owner::Call(g, K::ReleaseEnvironment),
    ]);
    out.extend((0..receipts).map(|i| Owner::Admit(g, i)));
}
fn plan(layout: CohortLayout, controlled: bool) -> Vec<Owner> {
    let mut out = Vec::new();
    append_call(&mut out, 0, 2, false);
    if controlled {
        out.push(Owner::Control(0, 0, Decision::Cancelled));
    }
    append_call(&mut out, 1, 2, false);
    if controlled {
        out.push(Owner::Control(1, 1, Decision::TimedOut));
    }
    out.extend([Owner::Wait(0), Owner::Retire(0), Owner::Registered(1)]);
    if layout == CohortLayout::Reuse {
        append_call(&mut out, 2, 2, false);
    }
    out.extend([
        Owner::Late(0, Decision::Cancelled),
        Owner::Late(1, Decision::TimedOut),
        Owner::Alias,
    ]);
    if layout == CohortLayout::Reuse {
        out.push(Owner::Status);
    }
    out.extend([Owner::Wait(1), Owner::Retire(1)]);
    if layout == CohortLayout::Reuse {
        out.extend([Owner::Registered(2), Owner::Wait(2), Owner::Retire(2)]);
    }
    out.extend([Owner::DropAliases, Owner::End]);
    out
}
fn combination_plan(mode: CohortMode, controlled: bool, polling: bool) -> Vec<Owner> {
    let mut p = plan(CohortLayout::Carriers, controlled);
    if mode == CohortMode::Early {
        p.retain(|o| !matches!(o, Owner::Control(1, _, _)));
        let i = p.iter().position(|o| *o == Owner::Admit(1, 0)).unwrap();
        p.insert(i, Owner::Publish(1));
    } else {
        let i = p.iter().position(|o| *o == Owner::Wait(0)).unwrap();
        p.splice(i..i, [Owner::Publish(1), Owner::Wait(1), Owner::Retire(1)]);
        let i = p.iter().position(|o| *o == Owner::Registered(1)).unwrap();
        p.remove(i);
        let i = p.iter().rposition(|o| *o == Owner::Wait(1)).unwrap();
        p.drain(i..i + 2);
    }
    if mode == CohortMode::Deferred && controlled {
        let i = p
            .iter()
            .position(|o| matches!(o, Owner::Control(1, 1, _)))
            .unwrap();
        p.insert(i, Owner::Control(1, 0, Decision::Cancelled));
    }
    if polling {
        let i = p.iter().position(|o| *o == Owner::Wait(0)).unwrap();
        p.insert(i, Owner::Poll);
    }
    p
}
static COMBINATION_PLANS: LazyLock<[Vec<Owner>; 6]> = LazyLock::new(|| {
    std::array::from_fn(|i| {
        combination_plan(
            if i < 4 {
                CohortMode::Early
            } else {
                CohortMode::Deferred
            },
            i % 2 == 1,
            (2..4).contains(&i),
        )
    })
});
static PLANS: LazyLock<[Vec<Owner>; 4]> = LazyLock::new(|| {
    [
        plan(CohortLayout::Carriers, false),
        plan(CohortLayout::Carriers, true),
        plan(CohortLayout::Reuse, false),
        plan(CohortLayout::Reuse, true),
    ]
});
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Independent Call, carrier, receipt and publisher lifetimes"
)]
pub(super) struct Unit {
    pub call: Call,
    pub runtime: Registration,
    pub drains: [Admission; 2],
    pub remembered: [Decision; 2],
    pub started: bool,
    pub handed: bool,
    pub pin: bool,
    pub sdk: u8,
    pub release: u8,
    pub cas_seen: [bool; 2],
    pub samples: [Option<u8>; 2],
    pub sdk_cancelled: bool,
    pub direct_settled: [bool; 2],
    pub invented_receipt: bool,
    pub direct_closed: bool,
    pub direct_retired: bool,
}
impl Unit {
    fn new(binding: Binding) -> Self {
        Self::with_version(binding, 2, 3)
    }
    fn with_version(binding: Binding, receipts: usize, version: u8) -> Self {
        Self {
            call: Call::new(1, false),
            runtime: Registration::with_version(receipts, binding, version),
            drains: [Admission::new(false), Admission::new(false)],
            remembered: [Decision::Pending; 2],
            started: false,
            handed: false,
            pin: false,
            sdk: 0,
            release: 0,
            cas_seen: [false; 2],
            samples: [None; 2],
            sdk_cancelled: false,
            direct_settled: [false; 2],
            invented_receipt: false,
            direct_closed: false,
            direct_retired: false,
        }
    }
}
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "One shared target has separate Owner, environment and native reference retirement"
)]
pub struct Cohort {
    pub(super) units: Vec<Unit>,
    pub(super) binding: Binding,
    pub(super) layout: CohortLayout,
    pub(super) controlled: bool,
    pub(super) pc: usize,
    pub(super) mode: CohortMode,
    pub(super) polling: Option<Polling>,
    pub(super) helper_active: bool,
    pub(super) helper_seen: u16,
    pub(super) shutdown: bool,
    pub(super) helper: u8,
    pub(super) attached: bool,
    pub(super) environment: bool,
    pub(super) runtime_reference: bool,
    pub(super) target: bool,
    pub(super) disposed: bool,
    pub(super) aliases: bool,
    pub(super) alias_value: Option<usize>,
    pub(super) storage: [Option<usize>; 3],
    pub(super) deferred: u8,
    pub(super) late: u8,
    pub(super) extra_producer: u8,
    pub(super) recovery: Option<Recovery>,
    pub(super) batch: Option<Batch>,
    pub(super) series: Option<Series>,
    pub fault: CohortFault,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CohortAction {
    Call(usize, K),
    Start(usize),
    Handoff(usize),
    Admit(usize, usize),
    Retry(usize, usize),
    InspectDeferred,
    Runtime(usize, R),
    Polling(P),
    DirectCopy(usize, usize),
    DirectSettle(usize, usize),
    HelperWakeBegin,
    HelperWakeEnd,
    CasSeen(usize, usize),
    Sample(usize, usize),
    PublishBegin(usize),
    PrefixSeen(usize),
    ExchangeSeen(usize),
    MergeWake(usize),
    ReadySelection,
    PublishEnd(usize),
    ReleaseBegin(usize),
    Decrement(usize),
    ReleaseEnd(usize),
    DrainRelease(usize, usize),
    DeferredBegin,
    DeferredCommit,
    DeferredFlag,
    DeferredEnd,
    Advance,
    Retire(usize),
    LateBegin,
    LateEnd,
    Alias,
    Status,
    DropAliases,
    Shutdown,
    Quiescent,
    Join,
    Detach,
    RuntimeRelease,
    EnvironmentFree,
    TargetFree,
    Dispose,
}
impl Cohort {
    /// # Panics
    /// Panics for a joined-C binding or a mixed-batch layout. Each requires
    /// its own profile constructor.
    #[must_use]
    pub fn new(binding: Binding, layout: CohortLayout, controlled: bool) -> Self {
        assert_ne!(binding, Binding::CJoined);
        assert_ne!(layout, CohortLayout::Series);
        Self::for_layout(binding, layout, controlled)
    }
    fn for_layout(binding: Binding, layout: CohortLayout, controlled: bool) -> Self {
        Self {
            units: (0..layout.count())
                .map(|g| {
                    if layout == CohortLayout::Series {
                        Unit::with_version(binding, series::RECEIPTS[g], series::VERSIONS[g])
                    } else {
                        Unit::new(binding)
                    }
                })
                .collect(),
            binding,
            layout,
            controlled,
            pc: 0,
            mode: CohortMode::Registered,
            polling: None,
            helper_active: false,
            helper_seen: 0,
            shutdown: false,
            helper: if binding == Binding::CDirect { 3 } else { 0 },
            attached: true,
            environment: true,
            runtime_reference: true,
            target: true,
            disposed: false,
            aliases: true,
            alias_value: None,
            storage: [None; 3],
            deferred: 0,
            late: 0,
            extra_producer: 0,
            recovery: None,
            batch: None,
            series: None,
            fault: CohortFault::None,
        }
    }
    /// Two notified Calls, optionally a third polling Call, sharing one Owner.
    /// # Panics
    /// Rejects a registered-only mode, deferred C, or polling without `CJoined`.
    #[must_use]
    pub fn combination(binding: Binding, mode: CohortMode, controlled: bool) -> Self {
        assert_ne!(mode, CohortMode::Mixed);
        assert_ne!(mode, CohortMode::Registered);
        assert!(mode != CohortMode::Deferred || binding == Binding::RustChannel);
        let mut s = Self::new(
            if binding == Binding::CJoined {
                Binding::CDirect
            } else {
                binding
            },
            CohortLayout::Carriers,
            controlled,
        );
        s.binding = binding;
        s.units = (0..2).map(|_| Unit::new(binding)).collect();
        s.helper = if binding == Binding::CDirect { 3 } else { 0 };
        s.mode = mode;
        s.polling = (binding == Binding::CJoined).then(Polling::new);
        s.global();
        s
    }
    /// Three notified Calls with address reuse, plus an optional polling Call.
    /// # Panics
    /// Rejects unsupported binding/mode/polling combinations.
    #[must_use]
    pub fn extended(binding: Binding, mode: CohortMode, controlled: bool, polling: bool) -> Self {
        assert!(
            mode != CohortMode::Registered
                && (binding == Binding::RustChannel || mode == CohortMode::Early)
        );
        assert!(binding != Binding::CDirect || !polling);
        assert!(binding != Binding::CJoined || polling);
        let mut s = Self::new(
            if binding == Binding::CJoined {
                Binding::CDirect
            } else {
                binding
            },
            CohortLayout::Reuse,
            controlled,
        );
        s.binding = binding;
        s.mode = mode;
        s.helper = if binding == Binding::CDirect { 3 } else { 0 };
        s.units = (0..3)
            .map(|g| {
                let mut u = Unit::new(binding);
                if mode == CohortMode::Mixed && g == 1 {
                    u.runtime = Registration::new(1, binding);
                }
                u
            })
            .collect();
        s.polling = polling.then(|| Polling::with_binding(binding));
        s.global();
        s
    }
    pub(super) fn extended_profile(&self) -> bool {
        self.mode != CohortMode::Registered && self.layout == CohortLayout::Reuse
    }
    /// Two v3 Calls and one v1 polling Call, with no prescribed recovery order.
    /// The Rust binding has two credits, so the second v3 Call starts Deferred.
    /// # Panics
    /// Rejects bindings or controls outside this profile.
    #[must_use]
    pub fn unordered(binding: Binding, controlled: bool) -> Self {
        assert!(binding == Binding::RustChannel || binding == Binding::CJoined && !controlled);
        let mut s = Self::new(
            if binding == Binding::CJoined {
                Binding::CDirect
            } else {
                binding
            },
            CohortLayout::Carriers,
            controlled,
        );
        s.binding = binding;
        s.helper = 0;
        for unit in &mut s.units {
            unit.runtime = Registration::new(1, binding);
        }
        s.aliases = false;
        s.polling = Some(Polling::with_binding(binding));
        s.recovery = Some(Recovery::new());
        s.global();
        s
    }
    pub(super) const fn receipt_count(&self) -> usize {
        if self.recovery.is_some() { 1 } else { 2 }
    }
    pub(super) fn receipts(&self, g: usize) -> usize {
        if self.series.is_some() {
            series::RECEIPTS[g]
        } else {
            self.receipt_count()
        }
    }
    /// Four notified generations in two batches and a v1 polling Call.
    /// # Panics
    /// Accepts joined C and Rust.
    #[must_use]
    pub fn mixed_batches(binding: Binding, schedule: MixedBatchSchedule, controlled: bool) -> Self {
        assert!(matches!(binding, Binding::CJoined | Binding::RustChannel));
        let mut state = Self::for_layout(binding, CohortLayout::Series, controlled);
        state.polling = Some(Polling::with_binding(binding));
        state.series = Some(Series::new(schedule));
        state.global();
        state
    }
    /// Two pairs of receipts share one notification after a third Call reuses
    /// the address of a retired generation with its own producer reference.
    /// # Panics
    /// Accepts joined C and Rust. Split admission requires the Rust binding.
    #[must_use]
    pub fn batched(binding: Binding, schedule: BatchSchedule, controlled: bool) -> Self {
        assert!(matches!(binding, Binding::CJoined | Binding::RustChannel));
        assert!(!schedule.split_admission() || binding == Binding::RustChannel);
        let mut state = Self::new(
            if binding == Binding::CJoined {
                Binding::CDirect
            } else {
                binding
            },
            CohortLayout::Reuse,
            controlled,
        );
        state.binding = binding;
        state.helper = 0;
        state.units = (0..3).map(|_| Unit::new(binding)).collect();
        if schedule == BatchSchedule::TogetherDeferred {
            state.units[2].runtime = Registration::new(1, binding);
        }
        state.polling = Some(Polling::with_binding(binding));
        state.batch = Some(Batch::new(schedule));
        state.global();
        state
    }
    pub(super) fn direct_receipt(&self, g: usize, i: usize) -> bool {
        if g == 2
            && i == 1
            && self
                .batch
                .as_ref()
                .is_some_and(|b| b.schedule == BatchSchedule::TogetherDeferred)
        {
            return true;
        }
        g == 1
            && (self.mode == CohortMode::Deferred
                || self.mode == CohortMode::Mixed && i == 1
                || self.recovery.is_some() && i == 0 && self.has_deferred())
    }
    pub(super) fn runtime_stage(&self, g: usize, i: usize) -> S {
        let u = &self.units[g];
        if i < u.runtime.inner.count {
            u.runtime.inner.completion.stage(i)
        } else {
            S::Absent
        }
    }
    fn capacity(&self) -> usize {
        if self.series.is_some() {
            return 6;
        }
        if let Some(batch) = &self.batch {
            return if batch.schedule.split_admission() {
                4
            } else {
                5
            };
        }
        match self.mode {
            CohortMode::Early => 4,
            CohortMode::Mixed => 3,
            _ => 2,
        }
    }
    fn tail_turn(&self, g: usize) -> bool {
        if let Some(series) = &self.series {
            return series.duplicate(g) || !self.environment;
        }
        if let Some(batch) = &self.batch {
            return batch.duplicate() == Some(g)
                || (!self.environment && (g == 0 || self.units[0].sdk == 4));
        }
        !(self.extended_profile() || self.recovery.is_some())
            || (!self.environment && (0..g).all(|j| self.units[j].sdk == 4))
    }
    pub(super) fn owner(&self) -> Owner {
        if let Some(series) = &self.series {
            return series::owner(series.schedule, self.controlled, self.pc);
        }
        if let Some(batch) = &self.batch {
            return batch::owner(batch.schedule, self.controlled, self.pc);
        }
        if self.recovery.is_some() {
            return recovery::owner(self.binding, self.controlled, self.pc);
        }
        if self.extended_profile() {
            return super::extended_plan::owner(
                self.binding,
                self.mode,
                self.polling.is_some(),
                self.controlled,
                self.pc,
            );
        }
        if self.mode != CohortMode::Registered {
            let index = if self.mode == CohortMode::Deferred {
                4
            } else if self.polling.is_some() {
                2
            } else {
                0
            };
            return COMBINATION_PLANS[index + usize::from(self.controlled)][self.pc];
        }
        PLANS[usize::from(self.layout == CohortLayout::Reuse) * 2 + usize::from(self.controlled)]
            [self.pc]
    }
    #[must_use]
    pub fn references(&self) -> u8 {
        u8::from(self.runtime_reference)
            + self.extra_producer
            + self
                .units
                .iter()
                .map(|u| {
                    u.runtime.temporary_count() + u8::from(u.runtime.inner.wake.producer_reference)
                })
                .sum::<u8>()
    }
    fn used(&self) -> usize {
        // Owner drain return is observed before mailbox settlement in Rust.
        // Count the operation through that interval until the credit returns.
        (0..self.units.len())
            .map(|g| {
                (0..self.receipts(g))
                    .filter(|&i| (S::Waiting..S::OwnerSettled).contains(&self.runtime_stage(g, i)))
                    .count()
            })
            .sum::<usize>()
            + self.polling.as_ref().map_or(0, |p| {
                use super::joined::Aux;
                usize::from((Aux::Waiting..Aux::Settled).contains(&p.stage))
            })
    }
    pub(super) fn credit(&self) -> bool {
        self.binding != Binding::RustChannel || self.used() < self.capacity()
    }
    fn physical(&self, g: usize) -> bool {
        (0..self.receipts(g)).all(|i| self.stage(g, i) >= S::Acquired)
    }
    fn all_physical(&self) -> bool {
        (0..self.units.len()).all(|g| self.physical(g))
            && self.polling.as_ref().is_none_or(Polling::physical)
    }
    fn registering(&self) -> bool {
        self.units.iter().any(|u| u.runtime.registering())
    }
    fn copying(&self) -> bool {
        self.units.iter().any(|u| {
            (0..u.runtime.inner.count).any(|i| u.runtime.inner.completion.stage(i) == S::Copying)
        })
    }
    fn working(&self) -> bool {
        self.units
            .iter()
            .any(|u| u.runtime.inner.worker_active.is_some())
            || self.polling.as_ref().is_some_and(|p| p.worker == 1)
    }
    fn polling_copying(&self) -> bool {
        self.polling.as_ref().is_some_and(|p| {
            matches!(
                p.stage,
                super::joined::Aux::Copied | super::joined::Aux::Claimed
            )
        })
    }
    fn controlling(&self) -> bool {
        self.units
            .iter()
            .any(|u| u.runtime.inner.control_in_progress())
            || self.deferred != 0
            || self.late != 0
    }
    fn context(&self) -> bool {
        self.units
            .iter()
            .any(|u| u.runtime.inner.wake.active_context)
    }
    fn records_released(&self) -> bool {
        (0..self.units.len())
            .all(|g| (0..self.receipts(g)).all(|i| self.stage(g, i) == S::Released))
            && self.polling.as_ref().is_none_or(Polling::released)
    }
    fn batch_drained(&self) -> bool {
        self.units[1..].iter().all(|unit| !unit.pin)
    }
    fn series_notified(&self, wave: usize) -> bool {
        self.series.as_ref().unwrap().rounds[wave]
            .leader
            .is_some_and(|leader| self.units[leader].runtime.inner.wake.notification == 2)
    }
    fn series_may_publish(&self, g: usize, wave: usize) -> bool {
        let series = self.series.as_ref().unwrap();
        if wave == 1 && series.schedule == MixedBatchSchedule::LateArrival {
            return (g == 3 && !series.late_selection) || (g == 2 && series.late_selection);
        }
        g / 2 == wave
            && !self.units.iter().any(|u| matches!(u.sdk, 1 | 2))
            && series.rounds[wave].first_exchange.is_none_or(|first| {
                series.schedule == MixedBatchSchedule::Together || !self.units[first].pin
            })
    }
    fn series_may_settle(&self, g: usize, i: usize) -> bool {
        let Owner::Wave(wave) = self.owner() else {
            return false;
        };
        let series = self.series.as_ref().unwrap();
        if g / 2 > wave {
            return false;
        }
        if wave == 1 && series.schedule == MixedBatchSchedule::LateArrival && g >= 2 {
            return if g == 3 {
                series.late_selection && self.series_notified(1) && self.physical(2)
            } else {
                self.stage(3, 0) >= S::OwnerSettled
                    || self.fault == CohortFault::LateArrivalReplacesSelection
                        && series.late_selection
            };
        }
        if g / 2 == wave
            && series.schedule == MixedBatchSchedule::Together
            && (!self.series_notified(wave)
                || !(0..=2 * wave + 1).all(|other| self.physical(other)))
        {
            return false;
        }
        // Within a ready set the Owner takes the lowest Future ID. A carrier
        // that has yet to publish does not hold up a ready peer.
        (0..=g).all(|older| {
            (0..if older == g { i } else { self.receipts(older) })
                .all(|index| !(S::Acquired..S::OwnerSettled).contains(&self.stage(older, index)))
        })
    }
    fn series_witnesses(&self) -> Vec<&'static str> {
        let Some(series) = &self.series else {
            return Vec::new();
        };
        let mut witnesses = Vec::new();
        for (round, names) in series.rounds.iter().zip([
            [
                "first-batch-v3-collected-first",
                "first-batch-v2-collected-first",
            ],
            [
                "second-batch-v3-collected-first",
                "second-batch-v2-collected-first",
            ],
        ]) {
            if let Some(first) = round.first_settle {
                witnesses.push(names[first % 2]);
            }
        }
        if series.rounds[1].leader.is_some() && self.stage(1, 0) < S::OwnerSettled {
            witnesses.push("second-batch-with-previous-receipt-pending");
        }
        if series.late_selection && self.physical(2) && self.stage(3, 0) < S::OwnerSettled {
            witnesses.push("older-receipt-arrived-after-ready-selection");
        }
        if let [Some(first), Some(second)] = series.rounds.map(|round| round.leader)
            && !self.environment
        {
            match (
                self.units[first].release == 3,
                self.units[second].release == 3,
            ) {
                (true, false) => witnesses.push("first-batch-leader-returned-first"),
                (false, true) => witnesses.push("second-batch-leader-returned-first"),
                _ => {}
            }
        }
        witnesses
    }
    fn duplicate(&self, g: usize) -> bool {
        self.series
            .as_ref()
            .is_some_and(|series| series.duplicate(g))
            || self.batch.as_ref().and_then(Batch::duplicate) == Some(g)
    }
    fn batch_notified(&self) -> bool {
        self.batch
            .as_ref()
            .and_then(|b| b.leader)
            .is_some_and(|g| self.units[g].runtime.inner.wake.notification == 2)
    }
    fn batch_may_settle(&self, g: usize, i: usize) -> bool {
        if g == 0 {
            return self.owner() == Owner::Wait(0);
        }
        self.owner() == Owner::DrainAll
            && (self.batch.as_ref().unwrap().schedule == BatchSchedule::Staggered
                || self.batch_notified()
                    && self.physical(1)
                    && self.physical(2)
                    && (1..=g).all(|older| {
                        (0..if older == g { i } else { 2 })
                            .all(|index| self.stage(older, index) >= S::OwnerSettled)
                    }))
    }
    fn batch_may_publish(&self, g: usize) -> bool {
        self.batch.as_ref().is_none_or(|batch| {
            g != 0
                && batch.first_exchange.is_none_or(|first| {
                    batch.schedule != BatchSchedule::Staggered || !self.units[first].pin
                })
                && (batch.schedule == BatchSchedule::Staggered
                    || self.polling.as_ref().is_some_and(Polling::drained))
                && (batch.schedule != BatchSchedule::TogetherRetry
                    || self.units[2].runtime.registered_all())
        })
    }
    fn batch_may_notify(&self, g: usize) -> bool {
        if let Some(series) = &self.series {
            return self.environment
                && series.rounds[g / 2].leader == Some(g)
                && (self.units[g ^ 1].release == 3
                    || self.fault == CohortFault::BatchNotifyBeforeDuplicateRelease);
        }
        self.batch.as_ref().is_none_or(|batch| {
            if g == 0 {
                return self.binding == Binding::RustChannel || !self.environment;
            }
            self.environment
                && batch.leader == Some(g)
                && (self.units[3 - g].release == 3
                    || self.fault == CohortFault::BatchNotifyBeforeDuplicateRelease)
        })
    }
    fn batch_witnesses(&self) -> Vec<&'static str> {
        let Some(batch) = &self.batch else {
            return Vec::new();
        };
        let mut witnesses = Vec::new();
        if batch.leader.is_some()
            && let Some(duplicate) = batch.duplicate()
            && self.units[duplicate].release == 3
            && self.units[0].runtime.inner.wake.producer_reference
            && self.units[3 - duplicate]
                .runtime
                .inner
                .wake
                .producer_reference
        {
            witnesses.push("duplicate-released-with-old-and-batch-references-live");
        }
        if let Some(first) = batch.first_settle {
            witnesses.push(if first == 1 {
                "peer-receipt-settled-first"
            } else {
                "replacement-receipt-settled-first"
            });
            if self.units[3 - first].sdk < 3 && !self.units[first].pin {
                witnesses.push("first-member-retired-before-second-exchange");
            }
        }
        if self.units[2].direct_settled[1] {
            witnesses.push("replacement-settled-deferred-receipt-without-mailbox-credit");
        } else if batch.schedule == BatchSchedule::TogetherRetry
            && self.units[2].runtime.returned(1)
        {
            witnesses.push("replacement-retried-deferred-receipt-before-batch");
        }
        witnesses
    }
    fn c_batch_root_can_return(&self, g: usize, i: usize) -> bool {
        if self.series.is_some() {
            // The poll was admitted between generations 1 and 2. C releases
            // records in reverse admission order at Owner teardown.
            return (g >= 2 || self.polling.as_ref().is_some_and(Polling::released))
                && (g..self.units.len()).all(|later| {
                    (if later == g { i + 1 } else { 0 }..self.receipts(later))
                        .all(|index| self.stage(later, index) == S::Released)
                });
        }
        self.batch.is_none()
            || self.polling.as_ref().is_some_and(Polling::released)
                && (g..self.units.len()).all(|later| {
                    (if later == g { i + 1 } else { 0 }..self.receipt_count())
                        .all(|index| self.stage(later, index) == S::Released)
                })
    }
    fn control_done(&self, g: usize) -> bool {
        !self.controlled
            || !matches!(g, 0 | 1)
            || self.units[g].remembered[usize::from(g != 0)] != Decision::Pending
    }
    fn global(&mut self) {
        for u in &mut self.units {
            let c = &mut u.runtime.inner;
            c.shutdown = self.shutdown;
            c.wake.helper = self.helper;
            c.wake.attached = self.attached;
            c.wake.context = self.attached;
            c.environment = self.environment;
            c.runtime_reference = self.runtime_reference;
            c.wake.target = self.target;
        }
    }
    pub(super) fn all_physical_for_replay(&self) -> bool {
        self.all_physical()
    }
    pub(super) fn enabled(&self, a: CohortAction) -> bool {
        self.actions().contains(&a)
    }
    pub(super) fn stage(&self, g: usize, i: usize) -> S {
        let u = &self.units[g];
        if self.runtime_stage(g, i) == S::Absent && u.drains[i].collected {
            if u.drains[i].owner == Drain::Released {
                S::Released
            } else if u.direct_settled[i] {
                S::OwnerSettled
            } else {
                S::Acquired
            }
        } else {
            self.runtime_stage(g, i)
        }
    }
    pub(super) fn decision(&self, g: usize, i: usize) -> Decision {
        let u = &self.units[g];
        if self.runtime_stage(g, i) == S::Absent && u.drains[i].collected {
            if u.remembered[i] == Decision::Pending {
                Decision::Completed
            } else {
                u.remembered[i]
            }
        } else {
            if i < u.runtime.inner.count {
                u.runtime.inner.completion.decision(i)
            } else {
                u.remembered[i]
            }
        }
    }
    pub(super) fn direct(&self, g: usize) -> bool {
        g == 1
            && (self.mode == CohortMode::Deferred
                || self.recovery.is_some()
                    && self.units[1].drains[0].attempted
                    && self.runtime_stage(1, 0) == S::Absent)
    }
    fn has_deferred(&self) -> bool {
        self.units[1].drains[0].owner == Drain::Deferred
    }
    fn recovery_witnesses(&self) -> Vec<&'static str> {
        let Some(recovery) = &self.recovery else {
            return Vec::new();
        };
        let mut witnesses: Vec<_> = recovery.order_witness().into_iter().collect();
        if self.direct(1) && self.units[1].direct_settled[0] {
            witnesses.push("deferred-collected-directly");
            if self.used() == self.capacity() {
                witnesses.push("deferred-collected-while-full");
            }
        } else if self.binding == Binding::RustChannel && self.units[1].runtime.returned(0) {
            if self.units[1].runtime.captured {
                witnesses.push("retried-with-registered-target");
            } else if self.units[1].sdk >= 3 {
                witnesses.push("retry-rolled-back-against-done");
            }
        }
        witnesses
    }
    fn ready_to_receive(&self) -> bool {
        self.owner() == Owner::DrainAll
            && self.recovery.as_ref().is_some_and(|r| {
                r.turn == Turn::Full || r.turn == Turn::Idle && !self.has_deferred()
            })
    }
    fn polling_recovery_allowed(&self, action: P) -> bool {
        if let Some(series) = &self.series {
            use super::joined::Aux;
            let poll = self.polling.as_ref().unwrap();
            let collecting = match self.owner() {
                Owner::Poll => true,
                Owner::Wave(1) if series.schedule != MixedBatchSchedule::Together => {
                    self.series_notified(1) && self.units.iter().all(|u| !u.pin)
                }
                _ => false,
            };
            return match action {
                P::Publish => collecting,
                P::Settle => {
                    collecting
                        && (self.binding != Binding::RustChannel
                            || series.taking == Some((self.units.len(), 0)))
                }
                P::DrainRelease if self.binding == Binding::RustChannel => {
                    collecting && series.taking.is_none() && poll.worker == 2
                }
                P::DrainRelease => {
                    self.owner() == Owner::End && (2..4).all(|g| self.stage(g, 0) == S::Released)
                }
                P::Close | P::Retire => poll.stage >= Aux::Settled,
                _ => true,
            };
        }
        if let Some(batch) = &self.batch {
            use super::joined::Aux;
            let collecting = self.owner() == Owner::DrainAll;
            let poll = self.polling.as_ref().unwrap();
            return match action {
                P::Publish => {
                    collecting
                        && if batch.schedule == BatchSchedule::Staggered {
                            self.batch_drained()
                        } else {
                            batch.first_exchange.is_none()
                        }
                }
                P::Settle => {
                    collecting
                        && (self.binding != Binding::RustChannel
                            || batch.taking == Some((self.units.len(), 0)))
                }
                P::DrainRelease if self.binding == Binding::RustChannel => {
                    collecting && batch.taking.is_none()
                }
                P::DrainRelease => self.owner() == Owner::End,
                P::Close | P::Retire => poll.stage >= Aux::Settled,
                _ => true,
            };
        }
        let Some(recovery) = &self.recovery else {
            return true;
        };
        let polling = self.polling.as_ref().unwrap();
        let collecting = self.owner() == Owner::DrainAll;
        match action {
            P::Publish => collecting,
            P::Settle => {
                collecting
                    && (self.binding == Binding::CJoined
                        || recovery.turn == Turn::Taking(self.units.len()))
            }
            P::DrainRelease if self.binding == Binding::RustChannel => self.ready_to_receive(),
            P::DrainRelease | P::Close | P::Retire => polling.stage >= super::joined::Aux::Settled,
            _ => true,
        }
    }
    fn worker_pending(&self) -> bool {
        (self.mode != CohortMode::Registered
            || self.recovery.is_some()
            || self.batch.is_some()
            || self.series.is_some())
            && self.binding == Binding::RustChannel
            && (self.units.iter().any(|u| {
                (0..u.runtime.inner.count).any(|i| {
                    u.runtime.inner.completion.stage(i) >= S::Acquired && !u.runtime.inner.woke[i]
                })
            }) || self
                .polling
                .as_ref()
                .is_some_and(|p| p.physical() && p.worker != 2))
    }
    fn worker_pending_other_poll(&self) -> bool {
        self.binding == Binding::RustChannel
            && self.units.iter().any(|u| {
                (0..u.runtime.inner.count).any(|i| {
                    u.runtime.inner.completion.stage(i) >= S::Acquired && !u.runtime.inner.woke[i]
                })
            })
    }
    fn physical_mask(&self) -> u16 {
        let mut m = 0;
        for g in 0..self.units.len() {
            for i in 0..self.receipts(g) {
                if self.stage(g, i) >= S::Acquired {
                    m |= 1 << (g * 2 + i);
                }
            }
        }
        if self.polling.as_ref().is_some_and(Polling::physical) {
            m |= 1 << (self.units.len() * 2);
        }
        m
    }
    fn admit_runtime(&mut self, g: usize, i: usize) {
        self.units[g].runtime.apply(R::Product(C::Admit(i)));
        let d = self.units[g].remembered[i];
        if d != Decision::Pending && self.fault != CohortFault::RetryDropsDecision {
            for c in [
                C::BeginControl(i, d),
                C::ResolveControl(i),
                C::EndControl(i, true),
            ] {
                self.units[g].runtime.apply(R::Product(c));
            }
        }
    }
    #[allow(
        clippy::too_many_lines,
        reason = "Keep the explicit Owner barriers in one transition table"
    )]
    fn owner_actions(&self, actions: &mut Vec<CohortAction>) {
        use CohortAction as H;
        match self.owner() {
            Owner::Call(g, k)
                if self.units[g].call.actions().contains(&k)
                    && (k != K::Enter
                        || g == 0
                        || self.units[g - 1].runtime.registered_all()
                        || self.extended_profile() && g == 2 && self.units[1].direct_retired) =>
            {
                actions.push(H::Call(g, k));
            }
            Owner::Start(g)
                if g != 2
                    || self.units[0].runtime.inner.completion.retired()
                    || self.fault == CohortFault::ReuseBeforeAllCopies =>
            {
                actions.push(H::Start(g));
            }
            Owner::Publish(g)
                if self.units[g].sdk
                    >= if self.extended_profile() && self.mode == CohortMode::Early {
                        4
                    } else {
                        3
                    } =>
            {
                actions.push(H::Advance);
            }
            Owner::Poll
                if self.polling.as_ref().is_some_and(|p| {
                    p.drained()
                        && (!self.extended_profile()
                            || if self.binding == Binding::RustChannel {
                                p.released()
                            } else {
                                self.helper_active
                            })
                }) =>
            {
                actions.push(H::Advance);
            }
            Owner::BindPoll if self.polling.as_ref().is_some_and(|p| p.bound) => {
                actions.push(H::Advance);
            }
            Owner::DrainAll | Owner::Wave(_) => {
                for g in 0..self.units.len() {
                    if !self.units[g].pin
                        || !(0..self.receipts(g)).all(|i| self.stage(g, i) >= S::OwnerSettled)
                    {
                        continue;
                    }
                    let u = &self.units[g];
                    if self.direct(g) {
                        actions.push(if u.direct_closed {
                            H::Retire(g)
                        } else {
                            H::Runtime(g, R::Product(C::CloseAdmission))
                        });
                    } else if u.runtime.enabled(R::Product(C::CloseAdmission)) {
                        actions.push(H::Runtime(g, R::Product(C::CloseAdmission)));
                    } else if u.runtime.enabled(R::Product(C::RetireCarrier)) {
                        actions.push(H::Retire(g));
                    }
                }
                if self.owner() == Owner::Wave(0) {
                    if !self.units[0].pin && self.series_notified(0) {
                        actions.push(H::Advance);
                    }
                } else if self.units.iter().all(|u| !u.pin)
                    && self.polling.as_ref().is_some_and(Polling::drained)
                    && (self.binding != Binding::CJoined
                        || self.helper_active
                        || self.series.as_ref().is_some_and(|series| {
                            series.schedule == MixedBatchSchedule::LateArrival
                        }))
                    && self.recovery.as_ref().is_none_or(|r| r.turn == Turn::Idle)
                    && (self.batch.is_none() || self.batch_notified())
                    && (self.series.is_none() || self.series_notified(1))
                {
                    actions.push(H::Advance);
                }
            }
            Owner::Retire(g) if self.direct(g) && self.physical(g) => {
                actions.push(if self.units[g].direct_closed {
                    H::Retire(g)
                } else {
                    H::Runtime(g, R::Product(C::CloseAdmission))
                });
            }
            Owner::Handoff(g) => actions.push(H::Handoff(g)),
            Owner::Admit(g, i) => actions.push(H::Admit(g, i)),
            Owner::Control(g, i, d) => {
                if self.units[g].drains[i].owner == Drain::Deferred {
                    match self.deferred {
                        0 => actions.push(H::DeferredBegin),
                        1 => actions.push(H::DeferredCommit),
                        2 => actions.push(H::DeferredFlag),
                        3 => actions.push(H::DeferredEnd),
                        _ => {}
                    }
                } else if !self.controlling() {
                    let r = R::Product(C::BeginControl(i, d));
                    if self.units[g].runtime.enabled(r) {
                        actions.push(H::Runtime(g, r));
                    }
                }
            }
            Owner::Wait(g)
                if self.physical(g)
                    && (!self.extended_profile()
                        || self.binding != Binding::CJoined
                        || !self.controlled
                        || g != 0
                        || self.units[0].runtime.inner.wake.notification == 2)
                    && (if self.binding == Binding::RustChannel {
                        (0..2).all(|i| {
                            self.stage(g, i) >= S::OwnerSettled
                                && self.units[g].drains[i].owner == Drain::Released
                        }) && (self.extended_profile()
                            || g == 0
                            || (0..2).all(|i| self.stage(g, i) == S::Released))
                    } else if g == 1 && !self.extended_profile() {
                        self.stage(g, 0) >= S::OwnerSettled
                    } else {
                        (0..2).all(|i| self.stage(g, i) >= S::OwnerSettled)
                    }) =>
            {
                actions.push(H::Advance);
            }
            Owner::Retire(g) if self.units[g].runtime.enabled(R::Product(C::RetireCarrier)) => {
                actions.push(H::Retire(g));
            }
            Owner::Retire(g) if self.units[g].runtime.enabled(R::Product(C::CloseAdmission)) => {
                actions.push(H::Runtime(g, R::Product(C::CloseAdmission)));
            }
            Owner::Registered(g) => {
                let split = g == 2
                    && self
                        .batch
                        .as_ref()
                        .is_some_and(|batch| batch.schedule.split_admission());
                if (self.units[g].runtime.registered_all()
                    || split && self.units[g].runtime.returned(0))
                    && (!self.extended_profile()
                        || g != 1
                        || self.mode != CohortMode::Early
                        || self.physical(g)
                            && (self.binding != Binding::RustChannel || !self.worker_pending()))
                {
                    actions.push(H::Advance);
                }
                for i in 0..2 {
                    if self.units[g].drains[i].owner == Drain::Deferred
                        && !self.direct_receipt(g, i)
                        && self.credit()
                    {
                        actions.push(H::Retry(g, i));
                        break;
                    }
                }
            }
            Owner::Late(_, _) if self.late == 0 => actions.push(H::LateBegin),
            Owner::Late(_, _) => actions.push(H::LateEnd),
            Owner::Alias => actions.push(H::Alias),
            Owner::Status => actions.push(H::Status),
            Owner::DropAliases => actions.push(H::DropAliases),
            _ => {}
        }
    }
    #[allow(
        clippy::too_many_lines,
        reason = "Keep the component guards and SDK intervals in one auditable transition table"
    )]
    fn runtime_actions(&self, g: usize, a: &mut Vec<CohortAction>) {
        use CohortAction as H;
        let u = &self.units[g];
        if !u.started {
            return;
        }
        for r in u.runtime.actions() {
            if self.direct(g)
                && !matches!(
                    r,
                    R::Product(C::Write | C::Prefix | C::Publish | C::NotifyBegin | C::NotifyEnd)
                        | R::NotifyReady
                )
            {
                continue;
            }
            let allow = match r {
                R::Begin(i) => {
                    (0..i).all(|j| u.runtime.returned(j))
                        && !self.registering()
                        && !self.copying()
                        && !self.polling_copying()
                        && !self.working()
                        && !self.worker_pending()
                        && (self.binding == Binding::RustChannel || !self.controlling())
                }
                R::Return(i) => u.samples[i].is_some(),

                R::Rollback(i) => u.cas_seen[i],

                R::Product(C::Write) => u.sdk == 1,
                R::Product(C::Prefix) => u.sdk == 1 && u.runtime.inner.completion.written(),
                R::Product(C::Publish) => u.sdk == 2,
                R::Product(C::BeginCopy(i)) => {
                    (if self.binding == Binding::RustChannel {
                        (0..i).all(|j| self.stage(g, j) >= S::Acquired)
                    } else {
                        (i + 1..self.receipts(g)).all(|j| {
                            self.stage(g, j) >= S::Acquired
                                || self.mode == CohortMode::Early
                                    && g == 1
                                    && self.stage(g, j) == S::Absent
                        })
                    }) && !self.registering()
                        && !self.copying()
                        && !self.polling_copying()
                        && !self.working()
                        && !self.worker_pending()
                }

                R::Product(C::Settle(i)) => {
                    if let Some(series) = &self.series {
                        if self.binding == Binding::RustChannel {
                            // A later packet cannot replace the receipt this turn has taken.
                            series.taking == Some((g, i))
                        } else {
                            self.series_may_settle(g, i)
                        }
                    } else if let Some(batch) = &self.batch {
                        self.batch_may_settle(g, i)
                            && (self.binding != Binding::RustChannel
                                || batch.taking == Some((g, i)))
                    } else if let Some(r) = &self.recovery {
                        self.owner() == Owner::DrainAll
                            && (self.binding != Binding::RustChannel || r.turn == Turn::Taking(g))
                    } else {
                        (self.binding != Binding::RustChannel
                            || self.owner() == Owner::Wait(g)
                            || self.mode == CohortMode::Early
                                && matches!(self.owner(), Owner::Wait(_)))
                            && !matches!(
                                self.owner(),
                                Owner::Call(..)
                                    | Owner::Start(_)
                                    | Owner::Handoff(_)
                                    | Owner::Admit(..)
                                    | Owner::Control(..)
                            )
                    }
                }
                R::Product(C::Release(i)) => {
                    u.drains[i].owner == Drain::Released && (g != 0 || !self.aliases)
                }

                R::Product(C::WorkerWakeBegin(_)) => {
                    !self.working() && !self.registering() && !self.copying()
                }

                R::Product(C::NotifyBegin) => {
                    u.sdk >= 3
                        && self.batch_may_notify(g)
                        && (self.recovery.is_none()
                            || self.binding == Binding::RustChannel
                            || !self.environment)
                        && (!self.extended_profile()
                            || self.binding == Binding::RustChannel
                            || if self.binding == Binding::CJoined && self.controlled && g == 0 {
                                self.helper_active && self.owner() == Owner::Wait(0)
                            } else {
                                !self.environment
                            })
                        && (self.binding == Binding::RustChannel
                            || !self
                                .units
                                .iter()
                                .any(|other| other.runtime.inner.wake.notification == 1)
                            || self.fault == CohortFault::OverlapDetachedNotifications)
                        && (self.fault != CohortFault::OverlapDetachedNotifications
                            || !self.attached)
                }
                R::NotifyReady => !self.registering() && !self.copying() && !self.polling_copying(),

                R::CompareExchange(_)
                | R::Retain(_)
                | R::Product(
                    C::Copy(_)
                    | C::Claim(_)
                    | C::Physical(_)
                    | C::ResolveControl(_)
                    | C::EndControl(..)
                    | C::WorkerWakeEnd(_)
                    | C::NotifyEnd,
                ) => true,
                R::Product(_) => false,
            };
            if allow {
                a.push(H::Runtime(g, r));
            }
        }
        for i in 0..self.receipts(g) {
            let choose_direct = if let Some(batch) = &self.batch {
                self.owner() == Owner::DrainAll
                    && self.batch_notified()
                    && self.physical(1)
                    && self.runtime_stage(2, 0) >= S::Acquired
                    && (batch.taking.is_none() || batch.taking == Some((g, i)))
            } else if let Some(r) = &self.recovery {
                self.owner() == Owner::DrainAll
                    && if u.drains[i].collected {
                        r.turn == Turn::Taking(g)
                    } else {
                        matches!(r.turn, Turn::Idle | Turn::Full)
                    }
            } else {
                self.owner() == Owner::Wait(g)
            };
            if self.direct_receipt(g, i) && choose_direct && u.sdk >= 3 {
                if !u.drains[i].collected
                    && (0..i)
                        .filter(|&j| self.direct_receipt(g, j))
                        .all(|j| u.drains[j].owner == Drain::Released)
                {
                    a.push(H::DirectCopy(g, i));
                } else if u.drains[i].collected && !u.direct_settled[i] {
                    a.push(H::DirectSettle(g, i));
                }
            }
            if i < u.runtime.inner.count && u.runtime.snapshot(i).is_some() && !u.cas_seen[i] {
                a.push(H::CasSeen(g, i));
            }
            if i < u.runtime.inner.count
                && u.runtime.enabled(R::Return(i))
                && u.cas_seen[i]
                && u.samples[i].is_none()
            {
                a.push(H::Sample(g, i));
            }
            let root_can_return = if self.binding == Binding::RustChannel {
                (if let Some(series) = &self.series {
                    self.series_may_settle(g, i)
                        && series.taking.is_none()
                        && u.runtime.inner.woke[i]
                } else if let Some(batch) = &self.batch {
                    if self.direct_receipt(g, i) {
                        batch.taking == Some((g, i)) && u.direct_settled[i]
                    } else {
                        self.batch_may_settle(g, i) && batch.taking.is_none()
                    }
                } else if let Some(r) = &self.recovery {
                    if self.direct(g) {
                        r.turn == Turn::Taking(g) && u.direct_settled[i]
                    } else {
                        self.ready_to_receive()
                    }
                } else {
                    self.owner() == Owner::Wait(g)
                        || self.mode == CohortMode::Early && matches!(self.owner(), Owner::Wait(_))
                }) && self.stage(g, i) >= S::Acquired
            } else {
                self.owner() == Owner::End
                    && self.stage(g, i) >= S::OwnerSettled
                    && (if self.batch.is_some() || self.series.is_some() {
                        self.c_batch_root_can_return(g, i)
                    } else {
                        self.extended_profile()
                            || self.recovery.is_some()
                            || (g..self.units.len()).all(|other| {
                                (0..2).all(|j| {
                                    other < g
                                        || other == g && j <= i
                                        || self.stage(other, j) == S::Released
                                })
                            })
                    })
            };
            if root_can_return
                && (u.drains[i].owner == Drain::Receipt
                    || self.direct_receipt(g, i)
                        && u.direct_settled[i]
                        && u.drains[i].owner == Drain::Deferred)
            {
                a.push(H::DrainRelease(g, i));
            }
        }
        if u.sdk == 0
            && ((self.owner() == Owner::Wait(g)
                && u.runtime.registered_all()
                && self.control_done(g))
                || self.owner() == Owner::Publish(g)
                || matches!(self.owner(), Owner::Wave(wave) if self.series_may_publish(g, wave))
                || self.owner() == Owner::DrainAll
                    && self.batch_may_publish(g)
                    && !self.units.iter().any(|x| matches!(x.sdk, 1 | 2)))
        {
            a.push(H::PublishBegin(g));
        }
        if u.sdk == 1 && u.runtime.inner.completion.prefix() {
            a.push(H::PrefixSeen(g));
        }
        if u.sdk == 2 && u.runtime.inner.completion.published() {
            a.push(H::ExchangeSeen(g));
        }
        if u.sdk == 3 && (!u.runtime.captured || u.release == 3) {
            a.push(H::PublishEnd(g));
        }
        if u.runtime.enabled(R::Product(C::ProducerRelease))
            || (self.duplicate(g) && u.runtime.inner.wake.producer_reference)
        {
            match u.release {
                0 if (self.batch.is_none() && self.series.is_none()) || self.tail_turn(g) => {
                    a.push(H::ReleaseBegin(g));
                }
                1 if self.tail_turn(g) => a.push(H::Decrement(g)),
                _ => {}
            }
        }
        if u.release == 2 && self.tail_turn(g) {
            a.push(H::ReleaseEnd(g));
        }
    }
}
impl Protocol for Cohort {
    type Action = CohortAction;
    fn name(&self) -> String {
        if let Some(series) = &self.series {
            return format!(
                "cohort-{:?}-{:?}-controlled{}",
                self.binding, series.schedule, self.controlled
            );
        }
        format!(
            "cohort-{:?}-{:?}-{:?}-controlled{}",
            self.binding, self.layout, self.mode, self.controlled
        )
    }
    #[allow(
        clippy::too_many_lines,
        reason = "One table connects the global shared-target and helper guards"
    )]
    fn actions(&self) -> Vec<Self::Action> {
        use CohortAction as H;
        let mut a = Vec::new();
        self.owner_actions(&mut a);
        if self.fault == CohortFault::DetachAfterOneGeneration
            && self.attached
            && self.physical(0)
            && !self.all_physical()
        {
            a.push(H::Detach);
        }
        if self.fault == CohortFault::ReuseBeforeAllCopies
            && self.layout != CohortLayout::Carriers
            && !self.units[2].started
            && (0..self.receipts(0)).any(|i| self.stage(0, i) >= S::Acquired)
            && !self.physical(0)
        {
            a.push(H::Start(2));
        }

        if matches!(self.owner(), Owner::Wait(_) | Owner::Registered(_)) {
            for g in 0..self.units.len() {
                for i in 0..2 {
                    if self.units[g].drains[i].owner == Drain::Deferred
                        && !self.direct_receipt(g, i)
                        && self.credit()
                    {
                        a.push(H::Retry(g, i));
                        break;
                    }
                }
            }
        }
        if let Some(r) = &self.recovery
            && self.owner() == Owner::DrainAll
            && self.has_deferred()
        {
            if r.turn == Turn::Idle && !self.units[1].runtime.inner.completion.published() {
                a.push(H::InspectDeferred);
            }
            if r.turn == Turn::Retry
                || r.turn == Turn::Full && self.fault == CohortFault::RetryWithoutCredit
            {
                a.push(H::Retry(1, 0));
            }
        }
        if let Some(batch) = &self.batch
            && batch.leader.is_none()
            && self.units[1..]
                .iter()
                .all(|u| u.sdk == 3 && u.runtime.captured)
        {
            a.push(H::MergeWake(batch.first_exchange.unwrap()));
        }
        if let Owner::Wave(wave) = self.owner()
            && let Some(series) = &self.series
            && series.rounds[wave].leader.is_none()
            && self.units[wave * 2..wave * 2 + 2]
                .iter()
                .all(|u| u.sdk == 3 && u.runtime.captured)
        {
            a.push(H::MergeWake(series.rounds[wave].first_exchange.unwrap()));
        }
        let late_arrival = self
            .series
            .as_ref()
            .is_some_and(|series| series.schedule == MixedBatchSchedule::LateArrival);
        if late_arrival
            && self.owner() == Owner::Wave(1)
            && self.stage(1, 0) >= S::OwnerSettled
            && self.stage(3, 0) == S::Acquired
            && self.units[3].sdk == 3
            && !self.series.as_ref().unwrap().late_selection
            && (self.binding != Binding::RustChannel || self.units[3].runtime.inner.woke[0])
        {
            a.push(H::ReadySelection);
        }
        if self
            .batch
            .as_ref()
            .is_some_and(|b| b.schedule == BatchSchedule::TogetherRetry)
            && self.owner() == Owner::DrainAll
            && self.polling.as_ref().is_some_and(Polling::drained)
            && self.units[2].drains[1].owner == Drain::Deferred
            && self.credit()
        {
            a.push(H::Retry(2, 1));
        }
        if let Some(p) = &self.polling {
            if matches!(self.owner(), Owner::Poll | Owner::BindPoll) || p.started() {
                a.extend(
                    p.actions()
                        .into_iter()
                        .filter(|a| {
                            (!matches!(a, P::Copy | P::Claim | P::Physical | P::WorkerBegin)
                                || !self.copying() && !self.registering() && !self.working())
                                && (*a != P::Admit || self.credit())
                                && (!matches!(a, P::Copy | P::Claim | P::Physical | P::WorkerBegin)
                                    || !self.worker_pending_other_poll())
                                && self.polling_recovery_allowed(*a)
                        })
                        .map(H::Polling),
                );
                if self.fault == CohortFault::PollRecordDuringWorkerWake
                    && p.stage == super::joined::Aux::Settled
                    && p.worker != 2
                    && p.drain.owner == Drain::Released
                {
                    a.push(H::Polling(P::Release));
                }
                if self.fault == CohortFault::PollRecordBeforeDrain
                    && p.stage == super::joined::Aux::Settled
                    && p.drain.owner != Drain::Released
                {
                    a.push(H::Polling(P::Release));
                }
                if self.fault == CohortFault::PollRetireBeforeCopy && p.bound && !p.physical() {
                    a.push(H::Polling(P::Retire));
                }
                if self.fault == CohortFault::PollSettleBeforePhysical
                    && self.owner() == Owner::DrainAll
                    && p.bound
                    && !p.physical()
                {
                    a.push(H::Polling(P::Settle));
                }
            }
            if self.binding == Binding::CJoined
                && p.bound
                && self.helper < 2
                && !self.helper_active
                && self.physical_mask() & !self.helper_seen != 0
                && (!self.extended_profile() || p.physical() && self.helper_seen == 0)
                && (self.recovery.is_none() || self.helper_seen == 0)
                && (self.series.is_none() || self.helper_seen == 0 || late_arrival)
                && (self.batch.is_none()
                    || self.owner() == Owner::DrainAll
                        && self.helper_seen == 0
                        && self.physical_mask() & !3 != 0)
                && !self.copying()
                && !self.registering()
            {
                a.push(H::HelperWakeBegin);
            }
            if self.helper_active
                && (!(self.extended_profile()
                    || self.recovery.is_some()
                    || self.batch.is_some()
                    || self.series.is_some())
                    || self.shutdown
                    || late_arrival)
            {
                a.push(H::HelperWakeEnd);
            }
        }
        for g in 0..self.units.len() {
            self.runtime_actions(g, &mut a);
        }
        if self.owner() == Owner::End && !self.shutdown {
            a.push(H::Shutdown);
        }
        if self.shutdown
            && self.all_physical()
            && !self.working()
            && !self.registering()
            && !self.copying()
            && self.helper == 0
            && !self.helper_active
            && self.polling.as_ref().is_none_or(|p| p.worker == 2)
            && self
                .units
                .iter()
                .enumerate()
                .all(|(g, u)| self.direct(g) || u.runtime.inner.enabled(C::Quiescent))
        {
            a.push(H::Quiescent);
        }
        if self.fault == CohortFault::JoinDuringHelperWake && self.shutdown && self.helper_active {
            a.push(H::Join);
        }
        if self.helper == 2
            && (!self.helper_active || self.fault == CohortFault::JoinDuringHelperWake)
        {
            a.push(H::Join);
        }
        if self.shutdown
            && self.attached
            && (self.all_physical()
                || self.fault == CohortFault::DetachAfterOneGeneration && self.physical(0))
            && ((self.helper >= 2 && (self.binding != Binding::CJoined || self.helper == 3))
                || self.fault == CohortFault::DetachDuringHelperWake && self.all_physical())
            && (!self.helper_active || self.fault == CohortFault::DetachDuringHelperWake)
            && !self.context()
            && !self.working()
        {
            a.push(H::Detach);
        }
        if !self.attached && self.runtime_reference {
            a.push(H::RuntimeRelease);
        }
        if !self.attached
            && self.environment
            && !self.context()
            && !self.working()
            && !self.helper_active
            && self.records_released()
        {
            a.push(H::EnvironmentFree);
        }
        if self.target
            && !self.runtime_reference
            && (self.references() == 0
                || self.fault == CohortFault::FreeWithOtherProducer
                    && self.units.iter().any(|u| u.release >= 2))
        {
            a.push(H::TargetFree);
        }
        if !self.disposed
            && !self.environment
            && self.helper == 3
            && self.records_released()
            && self.units.iter().all(|u| !u.call.environment && !u.pin)
        {
            a.push(H::Dispose);
        }
        a
    }
    #[allow(
        clippy::too_many_lines,
        reason = "Explicit interface transitions keep ownership changes auditable"
    )]
    fn apply(&mut self, action: Self::Action) {
        use CohortAction as H;
        match action {
            H::Call(g, k) => {
                self.units[g].call.apply(k);
                if k == K::ReleaseEnvironment {
                    for k in [
                        K::RetireLedger,
                        K::DropAlias,
                        K::ReleaseBorrower,
                        K::CloseHandle,
                        K::CollectValue,
                    ] {
                        self.units[g].call.apply(k);
                    }
                    if self.fault == CohortFault::CallReleaseDropsPin {
                        self.units[g].pin = false;
                    }
                }
                self.pc += 1;
            }
            H::Start(g) => {
                self.units[g].started = true;
                self.units[g].pin = true;
                self.storage[self.layout.storage(g)] = Some(g);
                if g == 2
                    && self.fault == CohortFault::ReuseConsumesOldReference
                    && self.units[0].runtime.inner.wake.producer_reference
                {
                    self.units[0].runtime.apply(R::Product(C::ProducerRelease));
                }
                self.pc += 1;
            }
            H::Handoff(g) => {
                self.units[g].handed = true;
                self.pc += 1;
            }
            H::Admit(g, i) => {
                self.units[g].drains[i] = Admission::new(!self.credit());
                self.units[g].drains[i].apply(A::TryAdmit);
                if self.units[g].runtime.inner.completion.published() {
                    self.units[g].drains[i].apply(A::Publish);
                }
                if self.units[g].drains[i].owner == Drain::Receipt {
                    self.admit_runtime(g, i);
                }
                self.pc += 1;
            }
            H::Retry(g, i) => {
                self.units[g].drains[i].apply(A::CreditAvailable);
                self.units[g].drains[i].apply(A::RetryAdmit);
                self.admit_runtime(g, i);
                if let Some(r) = &mut self.recovery {
                    r.turn = Turn::Idle;
                }
            }
            H::InspectDeferred => {
                let turn = if self.credit() {
                    Turn::Retry
                } else {
                    Turn::Full
                };
                self.recovery.as_mut().unwrap().turn = turn;
            }
            H::Polling(p) => {
                let poll = self.polling.as_mut().unwrap();
                poll.apply(p);
                if let Some(r) = &mut self.recovery {
                    if p == P::DrainRelease && self.binding == Binding::RustChannel {
                        r.turn = Turn::Taking(self.units.len());
                    }
                    if p == P::Settle {
                        r.received(self.units.len());
                    }
                }
                if let Some(batch) = &mut self.batch {
                    if p == P::DrainRelease && self.binding == Binding::RustChannel {
                        batch.taking = Some((self.units.len(), 0));
                    }
                    if p == P::Settle {
                        batch.taking = None;
                    }
                }
                if let Some(series) = &mut self.series {
                    if p == P::DrainRelease && self.binding == Binding::RustChannel {
                        series.taking = Some((self.units.len(), 0));
                    }
                    if p == P::Settle {
                        series.taking = None;
                    }
                }
            }
            H::HelperWakeBegin => {
                self.helper_active = true;
                self.helper_seen |= self.physical_mask();
            }
            H::HelperWakeEnd => self.helper_active = false,
            H::DirectCopy(g, i) => {
                self.units[g].drains[i].apply(A::CollectDeferred);
                if let Some(r) = &mut self.recovery {
                    r.turn = Turn::Taking(g);
                }
                if let Some(batch) = &mut self.batch {
                    batch.taking = Some((g, i));
                }
                if self.fault == CohortFault::DirectInventsReceipt {
                    self.units[g].invented_receipt = true;
                }
            }
            H::DirectSettle(g, i) => {
                self.units[g].direct_settled[i] = true;
                if let Some(batch) = &mut self.batch {
                    batch.first_settle.get_or_insert(g);
                }
                if self.fault == CohortFault::DirectLosesDecision {
                    self.units[g].remembered[i] = Decision::Completed;
                }
            }
            H::Runtime(g, registration) => {
                if self.direct(g) && registration == R::Product(C::CloseAdmission) {
                    self.units[g].direct_closed = true;
                } else {
                    self.units[g].runtime.apply(registration);
                }
                if let R::Return(i) = registration {
                    self.units[g].samples[i] = None;
                }
                if let R::CompareExchange(1) = registration
                    && self.fault == CohortFault::SecondReceiptTransfersAgain
                {
                    self.extra_producer += 1;
                }
                if matches!(registration, R::Rollback(_))
                    && self.fault == CohortFault::CandidateConsumesProducer
                    && g > 0
                    && self.units[0].runtime.inner.wake.producer_reference
                {
                    self.units[0].runtime.apply(R::Product(C::ProducerRelease));
                }
                if let R::Product(c) = registration {
                    match c {
                        C::Publish => {
                            for d in &mut self.units[g].drains {
                                d.apply(A::Publish);
                            }
                        }
                        C::Physical(i) => self.units[g].drains[i].apply(A::Collect),
                        C::Settle(_) => {
                            if let Some(r) = &mut self.recovery {
                                r.received(g);
                            }
                            if let Some(batch) = &mut self.batch {
                                batch.taking = None;
                                if g != 0 {
                                    batch.first_settle.get_or_insert(g);
                                }
                            }
                            if let Some(series) = &mut self.series {
                                series.taking = None;
                                series.rounds[g / 2].first_settle.get_or_insert(g);
                            }
                        }
                        C::ResolveControl(i) => {
                            let d = self.decision(g, i);
                            if d != Decision::Pending {
                                self.units[g].remembered[i] = d;
                                self.units[g].sdk_cancelled = true;
                            }
                        }
                        C::EndControl(..) => self.pc += 1,
                        _ => {}
                    }
                }
                if let R::Return(i) = registration {
                    self.units[g].samples[i] = None;
                }
            }
            H::CasSeen(g, i) => self.units[g].cas_seen[i] = true,
            H::Sample(g, i) => self.units[g].samples[i] = Some(self.references()),
            H::PublishBegin(g) => self.units[g].sdk = 1,
            H::PrefixSeen(g) => self.units[g].sdk = 2,
            H::ExchangeSeen(g) => {
                self.units[g].sdk = 3;
                if g != 0
                    && let Some(batch) = &mut self.batch
                {
                    batch.first_exchange.get_or_insert(g);
                }
                if let Some(series) = &mut self.series {
                    series.rounds[g / 2].first_exchange.get_or_insert(g);
                }
                if self.fault == CohortFault::SdkDoneDropsCallEnvironment {
                    self.units[g].call.environment = false;
                }
            }
            H::MergeWake(leader) => {
                if let Some(series) = &mut self.series {
                    series.rounds[leader / 2].leader = Some(leader);
                } else {
                    self.batch.as_mut().unwrap().leader = Some(leader);
                }
            }
            H::ReadySelection => self.series.as_mut().unwrap().late_selection = true,
            H::PublishEnd(g) => self.units[g].sdk = 4,
            H::ReleaseBegin(g) => self.units[g].release = 1,
            H::Decrement(g) => {
                let consume = if self.duplicate(g)
                    && self.fault == CohortFault::BatchReleaseConsumesOldReference
                    && (self.series.is_none() || g >= 2)
                {
                    self.series
                        .as_ref()
                        .map_or(0, |series| series.rounds[0].leader.unwrap_or(g ^ 1))
                } else {
                    g
                };
                self.units[consume]
                    .runtime
                    .apply(R::Product(C::ProducerRelease));
                self.units[g].release = 2;
            }
            H::ReleaseEnd(g) => self.units[g].release = 3,
            H::DrainRelease(g, i) => {
                let direct = self.direct(g);
                let direct_receipt = self.direct_receipt(g, i);
                self.units[g].drains[i].apply(A::Release);
                self.units[g].drains[i].apply(A::ReleaseRoot);
                if self.binding == Binding::RustChannel
                    && let Some(r) = &mut self.recovery
                {
                    if direct {
                        r.received(g);
                    } else {
                        r.turn = Turn::Taking(g);
                    }
                }
                if self.binding == Binding::RustChannel
                    && let Some(batch) = &mut self.batch
                {
                    batch.taking = (!direct_receipt).then_some((g, i));
                }
                if self.binding == Binding::RustChannel
                    && let Some(series) = &mut self.series
                {
                    series.taking = Some((g, i));
                }
            }
            H::DeferredBegin => self.deferred = 1,
            H::DeferredCommit => {
                let Owner::Control(g, i, d) = self.owner() else {
                    unreachable!()
                };
                self.units[g].drains[i].apply(A::Cancel);
                self.units[g].remembered[i] = d;
                self.units[g].sdk_cancelled = true;
                self.deferred = 2;
            }
            H::DeferredFlag => self.deferred = 3,
            H::DeferredEnd => {
                self.deferred = 0;
                self.pc += 1;
            }
            H::Advance => self.pc += 1,
            H::Retire(g) => {
                if self.direct(g) {
                    self.units[g].direct_retired = true;
                } else {
                    self.units[g].runtime.apply(R::Product(C::RetireCarrier));
                }
                self.units[g].pin = false;
                if self.recovery.is_none()
                    && self.series.is_none()
                    && (self.batch.is_none() || g == 0)
                {
                    self.pc += 1;
                }
            }
            H::LateBegin => self.late = 1,
            H::LateEnd => {
                self.late = 0;
                if self.fault == CohortFault::OldControlChangesReplacement
                    && self.layout != CohortLayout::Carriers
                {
                    self.units[2].sdk_cancelled = true;
                }
                self.pc += 1;
            }
            H::Alias => {
                self.alias_value = Some(if self.fault == CohortFault::OldAliasReadsReplacement {
                    self.units.len() - 1
                } else {
                    0
                });
                self.pc += 1;
            }
            H::Status | H::DropAliases => {
                if action == H::DropAliases {
                    self.aliases = false;
                }
                self.pc += 1;
            }
            H::Shutdown => self.shutdown = true,
            H::Quiescent => self.helper = 2,
            H::Join => self.helper = 3,
            H::Detach => self.attached = false,
            H::RuntimeRelease => self.runtime_reference = false,
            H::EnvironmentFree => self.environment = false,
            H::TargetFree => self.target = false,
            H::Dispose => self.disposed = true,
        }
        self.global();
    }
    #[allow(
        clippy::too_many_lines,
        reason = "Keep all cross-component lifetime invariants together"
    )]
    fn check(&self) -> Result<(), Violation> {
        if let Some(p) = &self.polling {
            p.check()?;
        }
        require(
            !self.helper_active || self.environment && self.attached && self.helper != 3,
            "NL-I5",
            "helper wake outlived context or join",
        )?;
        require(
            self.binding != Binding::CJoined || self.attached || self.helper == 3,
            "NL-I5",
            "joined C detached without an actual join",
        )?;
        require(
            self.binding == Binding::RustChannel
                || self
                    .units
                    .iter()
                    .filter(|u| u.runtime.inner.wake.notification == 1)
                    .count()
                    <= 1,
            "NL-I5",
            "shared C gate admitted overlapping detached notifications",
        )?;
        require(
            self.target || self.references() == 0,
            "NL-I7",
            "shared target lost a generation or temporary reference",
        )?;
        require(
            self.extra_producer == 0,
            "NL-I7",
            "second receipt acquired a second SDK reference",
        )?;
        require(
            self.attached || self.all_physical(),
            "NL-I5",
            "shared target detached after only one generation copied",
        )?;
        require(
            self.alias_value.is_none_or(|g| g == 0),
            "NL-I8",
            "old receipt read replacement storage",
        )?;
        require(
            !self.registering()
                || self
                    .units
                    .iter()
                    .filter(|u| u.runtime.registering())
                    .count()
                    == 1,
            "NL-I7",
            "registrars overlap across generations",
        )?;
        if self.binding == Binding::RustChannel {
            require(
                self.used() <= self.capacity(),
                "NL-I9",
                "normal admission exceeded shared credit",
            )?;
            if let Some(r) = &self.recovery {
                require(
                    (r.order[0] != 1 || self.direct(1)) && (r.order[2] != 1 || !self.direct(1)),
                    "NL-I9",
                    "Deferred recovery bypassed full-credit or next-turn admission rules",
                )?;
            }
        }
        for (g, u) in self.units.iter().enumerate() {
            u.call.check()?;
            require(
                !u.invented_receipt,
                "NL-I9",
                "direct collection invented a reactor receipt",
            )?;
            if self.direct(g) {
                u.runtime.inner.wake.check()?;
                require(
                    !u.runtime.captured
                        && !u.runtime.inner.wake.producer_reference
                        && u.runtime.temporary_count() == 0,
                    "NL-I7",
                    "direct collection acquired a notification reference",
                )?;
                require(
                    (0..u.runtime.inner.count)
                        .all(|i| u.runtime.inner.completion.stage(i) == S::Absent),
                    "NL-I9",
                    "direct collection invented a reactor receipt",
                )?;
            } else {
                u.runtime.check()?;
            }
            if self.mode == CohortMode::Mixed && g == 1 {
                require(
                    u.runtime.inner.count == 1,
                    "NL-I9",
                    "split admission invented a second reactor receipt",
                )?;
            }
            for d in &u.drains {
                d.check()?;
            }
            require(
                !u.started || u.pin || u.runtime.inner.completion.retired() || u.direct_retired,
                "NL-I5",
                "Call release lost the independent SDK pin",
            )?;
            require(
                !u.runtime.inner.completion.retired() || self.physical(g),
                "NL-I4",
                "carrier retired before every real or direct receipt copied",
            )?;
            for i in 0..self.receipts(g) {
                require(
                    self.stage(g, i) == S::Absent || u.handed,
                    "NL-I9",
                    "receipt admitted before Call handoff",
                )?;
                if matches!(u.remembered[i], Decision::Cancelled | Decision::TimedOut)
                    && self.runtime_stage(g, i) != S::Absent
                {
                    require(
                        self.decision(g, i) == u.remembered[i],
                        "NL-I1",
                        "retry lost a receipt's exact decision",
                    )?;
                }
            }
        }
        if self.layout != CohortLayout::Carriers && self.units[2].started {
            require(
                self.units[0].runtime.inner.completion.retired() && self.physical(0),
                "NL-I4",
                "new Call reused storage before every old receipt copied",
            )?;
        }
        Ok(())
    }
    #[allow(
        clippy::too_many_lines,
        reason = "Keep the shared-reference and receipt frame conditions in one transition check"
    )]
    fn check_step(&self, a: Self::Action, after: &Self) -> Result<(), Violation> {
        if let Some(series) = &self.series {
            match a {
                CohortAction::MergeWake(g) => {
                    let both_captured = [g, g ^ 1]
                        .into_iter()
                        .all(|member| self.units[member].runtime.inner.wake.producer_reference);
                    let prior = g < 2
                        || series.rounds[0].leader.is_some_and(|leader| {
                            self.units[leader].runtime.inner.wake.producer_reference
                                && self.storage[0] == Some(2)
                                && !self.units[0].pin
                        });
                    require(
                        both_captured && prior && usize::from(self.references()) == 3 + g / 2,
                        "NL-I7",
                        "successive batch did not preserve every independent SDK reference",
                    )?;
                }
                CohortAction::Decrement(g) => require(
                    self.references() == after.references() + 1
                        && self.units.iter().zip(&after.units).enumerate().all(
                            |(member, (before, after))| {
                                after.runtime.inner.wake.producer_reference
                                    == (member != g && before.runtime.inner.wake.producer_reference)
                            },
                        ),
                    "NL-I7",
                    "SDK reference return consumed a different batch member's reference",
                )?,
                CohortAction::Runtime(g, R::Product(C::NotifyBegin)) => require(
                    series.rounds[g / 2].leader == Some(g)
                        && self.units[g ^ 1].release == 3
                        && self.units[g ^ 1].sdk >= 3,
                    "NL-I7",
                    "batch notified before returning its own duplicate reference",
                )?,
                CohortAction::Runtime(2, R::Product(C::Settle(0))) if series.late_selection => {
                    require(
                        self.stage(3, 0) >= S::OwnerSettled,
                        "NLS03",
                        "late arrival replaced the already selected receipt",
                    )?;
                }
                _ => {}
            }
        }
        if let Some(batch) = &self.batch {
            match a {
                CohortAction::MergeWake(_) => require(
                    self.storage[0] == Some(2)
                        && self.units[0].runtime.inner.completion.retired()
                        && self.references() == 4
                        && self
                            .units
                            .iter()
                            .all(|u| u.runtime.inner.wake.producer_reference),
                    "NL-I7",
                    "batch did not preserve the old producer through both new captures",
                )?,
                CohortAction::Decrement(g) if batch.duplicate() == Some(g) => require(
                    self.references() == after.references() + 1
                        && self.units.iter().zip(&after.units).enumerate().all(
                            |(i, (before, after))| {
                                after.runtime.inner.wake.producer_reference
                                    == (i != g && before.runtime.inner.wake.producer_reference)
                            },
                        ),
                    "NL-I7",
                    "batch duplicate returned another generation's reference",
                )?,
                CohortAction::Runtime(g, R::Product(C::NotifyBegin)) if g != 0 => require(
                    batch.leader == Some(g)
                        && self.units[3 - g].release == 3
                        && self.units[3 - g].sdk >= 3,
                    "NL-I7",
                    "batch notified before its duplicate reference was returned",
                )?,
                _ => {}
            }
        }
        if a == CohortAction::Polling(P::Settle) {
            require(
                self.polling.as_ref().unwrap().physical(),
                "NL-I2",
                "polling receipt collected before physical completion",
            )?;
        }
        if let CohortAction::Polling(p) = a {
            self.polling
                .as_ref()
                .unwrap()
                .check_step(p, after.polling.as_ref().unwrap())?;
        }
        if let CohortAction::Runtime(g, r) = a {
            self.units[g]
                .runtime
                .check_step(r, &after.units[g].runtime)?;
        }
        if let CohortAction::Call(g, k) = a {
            self.units[g].call.check_step(k, &after.units[g].call)?;
        }
        if matches!(a, CohortAction::Start(2)) {
            require(
                self.units[0].runtime == after.units[0].runtime,
                "NL-I7",
                "new generation consumed an old SDK reference",
            )?;
        }
        if let CohortAction::DirectSettle(g, i) = a {
            require(
                self.units[g].remembered[i] == after.units[g].remembered[i],
                "NL-I1",
                "direct collection lost the exact deferred decision",
            )?;
        }
        if a == CohortAction::LateEnd {
            require(
                self.units
                    .iter()
                    .zip(&after.units)
                    .all(|(x, y)| x.sdk_cancelled == y.sdk_cancelled),
                "NL-I8",
                "old control touched replacement cancellation state",
            )?;
        }
        if matches!(a, CohortAction::Runtime(_, R::Rollback(_))) {
            require(
                self.units.iter().zip(&after.units).all(|(x, y)| {
                    x.runtime.inner.wake.producer_reference
                        == y.runtime.inner.wake.producer_reference
                }),
                "NL-I7",
                "candidate rollback consumed another producer",
            )?;
        }
        Ok(())
    }
    fn terminal(&self) -> bool {
        self.disposed && !self.target && self.units.iter().all(|u| u.sdk == 4)
    }
    fn witnesses(&self) -> Vec<&'static str> {
        let mut w = self.recovery_witnesses();
        w.extend(self.batch_witnesses());
        w.extend(self.series_witnesses());
        if self.mode != CohortMode::Registered {
            if (self.direct(1) || self.mode == CohortMode::Mixed)
                && self.units[1].direct_settled == [true; 2]
                && self.units[1].remembered == [Decision::Cancelled, Decision::TimedOut]
            {
                w.push("direct-cancel-and-timeout-preserved-independently");
            }
            if self.units[1].sdk >= 3 && !self.units[1].drains[0].attempted {
                w.push("sdk-published-before-first-admission");
            }
            if (self.direct(1) || self.mode == CohortMode::Mixed)
                && self.units[1]
                    .drains
                    .iter()
                    .all(|d| d.collected && d.owner == Drain::Released)
                && self.used() == 2
            {
                w.push("two-direct-deferred-copies-without-credit");
            }
            if self.helper_active && self.records_released() {
                w.push("all-records-released-before-helper-wake-return");
            }
            if self.helper_active && self.context() {
                w.push("multiple-call-helper-and-sdk-context-overlap");
            }
            if self.polling.as_ref().is_some_and(Polling::released) {
                w.push("polling-call-drained-with-notified-calls");
            }
        }
        if self.mode == CohortMode::Mixed
            && self.units[1].drains[1].collected
            && self.runtime_stage(1, 1) == S::Absent
        {
            w.push("split-admission-direct-copy-with-real-peer-receipt");
        }
        if self.extended_profile()
            && self.units[1].sdk >= 3
            && self.units[1].call.environment
            && self.units[1].call.callbacks[0] == super::call::Callback::Executing
        {
            w.push("sdk-publishes-with-callback-environment-live");
        }
        if self
            .polling
            .as_ref()
            .is_some_and(|p| p.binding == Binding::RustChannel && p.released())
        {
            w.push("rust-polling-call-drained-in-reuse-product");
        }
        if self.references() >= 4 {
            w.push("shared-references-with-duplicate-receipt-registration");
        }
        if self.references() == 5 {
            w.push("five-shared-target-references");
        }
        if self.units[1].started && !self.physical(0) {
            w.push("second-call-started-before-first-sdk-drain");
        }
        if self.layout != CohortLayout::Carriers
            && self.units[2].started
            && self.series.as_ref().map_or_else(
                || self.units[0].runtime.inner.wake.producer_reference,
                |series| {
                    series.rounds[0].leader.is_some_and(|leader| {
                        self.units[leader].runtime.inner.wake.producer_reference
                    })
                },
            )
        {
            w.push("new-call-reuses-storage-with-old-sdk-reference");
        }
        if !self.environment
            && self
                .units
                .iter()
                .filter(|u| u.runtime.inner.wake.producer_reference)
                .count()
                >= 2
        {
            w.push("multiple-producers-outlive-all-call-and-wake-environments");
        }
        if self
            .units
            .iter()
            .any(|u| u.drains.iter().any(|d| d.owner == Drain::Deferred))
        {
            w.push("multiple-receipts-share-deferred-credit-pressure");
        }
        if self.controlled
            && self.decision(0, 0) == Decision::Cancelled
            && self.decision(0, 1) == Decision::Completed
        {
            w.push("receipt-decisions-independent-on-one-carrier");
        }
        w
    }
}
