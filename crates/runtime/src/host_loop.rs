//! Generation-scoped Host Loop controller shared by queue-post adapters.

use core::ffi::c_void;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering, fence};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, ThreadId};

use rinzu_abi::{
    HOST_NESTED_ALL_MODES, HOST_NESTED_DECLARED_MODES, HOST_NESTED_UNSUPPORTED, HOST_POST_ACCEPTED,
    HOST_POST_CLOSED, HOST_POST_FAILURE, HOST_RETURN_RESUME_MANAGED, HostLoopProfileV1,
    HostOwnerTurnCallback, STATUS_OK, is_compatible,
};

use crate::diagnostic::{RuntimeDiagnostic, SourceLocation};

const PHASE_BITS: u32 = 3;
const LEASE_BITS: u32 = 20;
const LEASE_SHIFT: u32 = PHASE_BITS;
const GENERATION_SHIFT: u32 = PHASE_BITS + LEASE_BITS;
const LEASE_MASK: u64 = (1_u64 << LEASE_BITS) - 1;
const MAX_GENERATION: u64 = (1_u64 << (u64::BITS - GENERATION_SHIFT)) - 1;

struct HostWork {
    label: String,
    source: Option<SourceLocation>,
    body: Box<dyn FnOnce() + Send + 'static>,
}

pub const STANDARD_HOST_QUEUE_CAPACITY: usize = 1024;
pub const STANDARD_HOST_TURN_BUDGET: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum HostPhase {
    Managed = 0,
    Entering = 1,
    Active = 2,
    Leaving = 3,
    Closing = 4,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostStateSnapshot {
    pub phase: HostPhase,
    pub generation: u64,
    pub in_flight_posts: u32,
    pub work_pending: bool,
    pub owner_post_outstanding: bool,
    pub queued_work: usize,
}

#[derive(Clone, Copy)]
pub struct OwnerTurnTarget {
    pub callback: HostOwnerTurnCallback,
    pub context: *mut c_void,
}

// SAFETY: a profile receives this target only under the process-lifetime
// callback contract. The target itself is never dereferenced by runtime code.
unsafe impl Send for OwnerTurnTarget {}
// SAFETY: the same C ABI contract permits the target to be copied to a
// thread-safe post mechanism.
unsafe impl Sync for OwnerTurnTarget {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostPostResult {
    Accepted,
    Closed,
    Failure,
}

pub trait HostLoopAdapter: Send + Sync {
    fn prepare(&self, owner: OwnerTurnTarget, generation: u64) -> Result<usize, String>;
    fn post_owner_turn(&self, ready_token: usize, generation: u64) -> HostPostResult;
    fn deactivate(&self, ready_token: usize, generation: u64);
}

type OwnerPostPrimitive = dyn Fn(OwnerTurnTarget, u64) -> HostPostResult + Send + Sync + 'static;

#[derive(Clone, Copy)]
struct PreparedStandardAdapter {
    owner: OwnerTurnTarget,
    generation: u64,
    active: bool,
}

/// Standard queue-post profile. Platform code supplies only a nonblocking
/// owner-thread post primitive; the runtime owns preparation, generation and
/// deactivation state.
pub struct StandardQueuePostAdapter {
    post: Arc<OwnerPostPrimitive>,
    prepared: Mutex<Option<PreparedStandardAdapter>>,
}

impl StandardQueuePostAdapter {
    #[must_use]
    pub fn new(
        post: impl Fn(OwnerTurnTarget, u64) -> HostPostResult + Send + Sync + 'static,
    ) -> Self {
        Self {
            post: Arc::new(post),
            prepared: Mutex::new(None),
        }
    }
}

impl HostLoopAdapter for StandardQueuePostAdapter {
    fn prepare(&self, owner: OwnerTurnTarget, generation: u64) -> Result<usize, String> {
        let mut prepared = self
            .prepared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if prepared.as_ref().is_some_and(|state| state.active) {
            return Err("standard queue-post adapter is already active".into());
        }
        *prepared = Some(PreparedStandardAdapter {
            owner,
            generation,
            active: true,
        });
        usize::try_from(generation).map_err(|_| "generation does not fit the ready token".into())
    }

    fn post_owner_turn(&self, ready_token: usize, generation: u64) -> HostPostResult {
        let owner = {
            let prepared = self
                .prepared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(prepared) = *prepared else {
                return HostPostResult::Closed;
            };
            if !prepared.active
                || prepared.generation != generation
                || ready_token != generation as usize
            {
                return HostPostResult::Closed;
            }
            prepared.owner
        };
        (self.post)(owner, generation)
    }

    fn deactivate(&self, ready_token: usize, generation: u64) {
        let mut prepared = self
            .prepared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(state) = prepared.as_mut()
            && state.generation == generation
            && ready_token == generation as usize
        {
            state.active = false;
        }
    }
}

#[must_use]
pub fn standard_queue_post_controller(
    post: impl Fn(OwnerTurnTarget, u64) -> HostPostResult + Send + Sync + 'static,
) -> Arc<HostLoopController> {
    HostLoopController::new(
        Arc::new(StandardQueuePostAdapter::new(post)),
        STANDARD_HOST_QUEUE_CAPACITY,
        STANDARD_HOST_TURN_BUDGET,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostOutcome {
    Accepted,
    Coalesced,
    Inactive,
    Closed,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DrainReport {
    pub executed: usize,
    pub follow_up: PostOutcome,
    pub stale: bool,
    pub work_panicked: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeaveReport {
    pub generation: u64,
    pub integration_failed: bool,
    pub needs_runtime_pump: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostLoopError {
    WrongThread {
        expected: ThreadId,
        actual: ThreadId,
    },
    AlreadyEntered,
    GenerationExhausted,
    PrepareFailed(String),
    InvalidProfile(String),
    QueueFull,
    QueueClosed,
    StaleGeneration,
}

impl fmt::Display for HostLoopError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongThread { expected, actual } => {
                write!(
                    formatter,
                    "Host Loop owner is {expected:?}, called from {actual:?}"
                )
            }
            Self::AlreadyEntered => formatter.write_str("root Host Loop entry is single-use"),
            Self::GenerationExhausted => formatter.write_str("Host Loop generation exhausted"),
            Self::PrepareFailed(message) => {
                write!(formatter, "Host Loop prepare failed: {message}")
            }
            Self::InvalidProfile(message) => {
                write!(formatter, "invalid HostLoopProfileV1: {message}")
            }
            Self::QueueFull => formatter.write_str("Host Loop managed queue is full"),
            Self::QueueClosed => formatter.write_str("Host Loop managed queue is closed"),
            Self::StaleGeneration => formatter.write_str("Host Loop generation is stale"),
        }
    }
}

impl std::error::Error for HostLoopError {}

pub struct HostActivation {
    controller: Arc<HostLoopController>,
    generation: u64,
    left: bool,
}

impl HostActivation {
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn leave(mut self) -> Result<LeaveReport, HostLoopError> {
        let report = self.controller.leave_generation(self.generation)?;
        self.left = true;
        Ok(report)
    }
}

impl Drop for HostActivation {
    fn drop(&mut self) {
        if !self.left && thread::current().id() == self.controller.owner_thread {
            let _ = self.controller.leave_generation(self.generation);
        }
    }
}

pub struct HostLoopController {
    owner_thread: ThreadId,
    adapter: Arc<dyn HostLoopAdapter>,
    state: AtomicU64,
    next_generation: AtomicU64,
    entered: AtomicBool,
    ready_token: AtomicUsize,
    work_pending: AtomicBool,
    owner_post_outstanding: AtomicBool,
    queued_work: AtomicUsize,
    sender: SyncSender<HostWork>,
    receiver: Mutex<Receiver<HostWork>>,
    turn_budget: usize,
    lease_wait_lock: Mutex<()>,
    lease_wait: Condvar,
    deactivated: AtomicBool,
    diagnostics: Mutex<Vec<RuntimeDiagnostic>>,
}

impl HostLoopController {
    #[must_use]
    pub fn new(
        adapter: Arc<dyn HostLoopAdapter>,
        queue_capacity: usize,
        turn_budget: usize,
    ) -> Arc<Self> {
        assert!(
            queue_capacity > 0,
            "Host Loop queue capacity must be non-zero"
        );
        assert!(turn_budget > 0, "Host Loop turn budget must be non-zero");
        let (sender, receiver) = sync_channel(queue_capacity);
        Arc::new(Self {
            owner_thread: thread::current().id(),
            adapter,
            state: AtomicU64::new(pack(HostPhase::Managed, 0, 0)),
            next_generation: AtomicU64::new(0),
            entered: AtomicBool::new(false),
            ready_token: AtomicUsize::new(0),
            work_pending: AtomicBool::new(false),
            owner_post_outstanding: AtomicBool::new(false),
            queued_work: AtomicUsize::new(0),
            sender,
            receiver: Mutex::new(receiver),
            turn_budget,
            lease_wait_lock: Mutex::new(()),
            lease_wait: Condvar::new(),
            deactivated: AtomicBool::new(false),
            diagnostics: Mutex::new(Vec::new()),
        })
    }

    pub fn enter(self: &Arc<Self>) -> Result<HostActivation, HostLoopError> {
        self.require_owner()?;
        if self
            .entered
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(HostLoopError::AlreadyEntered);
        }
        let generation = self.next_generation.fetch_add(1, Ordering::AcqRel) + 1;
        if generation > MAX_GENERATION {
            return Err(HostLoopError::GenerationExhausted);
        }
        let managed = pack(HostPhase::Managed, generation - 1, 0);
        let entering = pack(HostPhase::Entering, generation, 0);
        if self
            .state
            .compare_exchange(managed, entering, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            self.entered.store(false, Ordering::Release);
            return Err(HostLoopError::AlreadyEntered);
        }
        let target = OwnerTurnTarget {
            callback: owner_turn_trampoline,
            context: Arc::as_ptr(self).cast_mut().cast(),
        };
        let token = match self.adapter.prepare(target, generation) {
            Ok(token) => token,
            Err(message) => {
                self.state.store(managed, Ordering::Release);
                self.entered.store(false, Ordering::Release);
                return Err(HostLoopError::PrepareFailed(message));
            }
        };
        self.ready_token.store(token, Ordering::Release);
        self.deactivated.store(false, Ordering::Release);
        self.state
            .store(pack(HostPhase::Active, generation, 0), Ordering::Release);
        if self.work_pending.load(Ordering::Acquire) || self.queued_work.load(Ordering::Acquire) > 0
        {
            self.work_pending.store(true, Ordering::Release);
            let _ = self.try_post(generation);
        }
        Ok(HostActivation {
            controller: Arc::clone(self),
            generation,
            left: false,
        })
    }

    pub fn enqueue(
        &self,
        work: impl FnOnce() + Send + 'static,
    ) -> Result<PostOutcome, HostLoopError> {
        self.enqueue_at("managed host work", None, work)
    }

    pub fn enqueue_at(
        &self,
        label: impl Into<String>,
        source: Option<SourceLocation>,
        work: impl FnOnce() + Send + 'static,
    ) -> Result<PostOutcome, HostLoopError> {
        self.queued_work.fetch_add(1, Ordering::AcqRel);
        match self.sender.try_send(HostWork {
            label: label.into(),
            source,
            body: Box::new(work),
        }) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.queued_work.fetch_sub(1, Ordering::AcqRel);
                return Err(HostLoopError::QueueFull);
            }
            Err(TrySendError::Disconnected(_)) => {
                self.queued_work.fetch_sub(1, Ordering::AcqRel);
                return Err(HostLoopError::QueueClosed);
            }
        }
        self.work_pending.store(true, Ordering::Release);
        let state = unpack(self.state.load(Ordering::Acquire));
        if state.phase == HostPhase::Active {
            Ok(self.try_post(state.generation))
        } else {
            Ok(PostOutcome::Inactive)
        }
    }

    pub fn owner_turn(&self, generation: u64) -> Result<DrainReport, HostLoopError> {
        self.require_owner()?;
        let state = unpack(self.state.load(Ordering::Acquire));
        if state.phase != HostPhase::Active || state.generation != generation {
            return Ok(DrainReport {
                executed: 0,
                follow_up: PostOutcome::Inactive,
                stale: true,
                work_panicked: false,
            });
        }
        if self
            .owner_post_outstanding
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Ok(DrainReport {
                executed: 0,
                follow_up: PostOutcome::Coalesced,
                stale: false,
                work_panicked: false,
            });
        }
        self.work_pending.store(false, Ordering::Release);
        let mut executed = 0;
        let mut work_panicked = false;
        while executed < self.turn_budget {
            let work = self
                .receiver
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .try_recv();
            match work {
                Ok(work) => {
                    self.queued_work.fetch_sub(1, Ordering::AcqRel);
                    executed += 1;
                    if catch_unwind(AssertUnwindSafe(work.body)).is_err() {
                        work_panicked = true;
                        self.diagnostics
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push(RuntimeDiagnostic::host_callback_failure(
                                work.label,
                                work.source,
                            ));
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Err(HostLoopError::QueueClosed),
            }
        }
        fence(Ordering::Acquire);
        let follow_up = if self.queued_work.load(Ordering::Acquire) > 0 {
            self.work_pending.store(true, Ordering::Release);
            self.try_post(generation)
        } else {
            PostOutcome::Inactive
        };
        Ok(DrainReport {
            executed,
            follow_up,
            stale: false,
            work_panicked,
        })
    }

    #[must_use]
    pub fn snapshot(&self) -> HostStateSnapshot {
        let state = unpack(self.state.load(Ordering::Acquire));
        HostStateSnapshot {
            phase: state.phase,
            generation: state.generation,
            in_flight_posts: state.leases,
            work_pending: self.work_pending.load(Ordering::Acquire),
            owner_post_outstanding: self.owner_post_outstanding.load(Ordering::Acquire),
            queued_work: self.queued_work.load(Ordering::Acquire),
        }
    }

    #[must_use]
    pub fn diagnostics(&self) -> Vec<RuntimeDiagnostic> {
        self.diagnostics
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    #[must_use]
    pub fn owner_thread(&self) -> ThreadId {
        self.owner_thread
    }

    fn require_owner(&self) -> Result<(), HostLoopError> {
        let actual = thread::current().id();
        if actual == self.owner_thread {
            Ok(())
        } else {
            self.diagnostics
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(RuntimeDiagnostic::wrong_thread(
                    "Host Loop operation",
                    self.owner_thread,
                    actual,
                ));
            Err(HostLoopError::WrongThread {
                expected: self.owner_thread,
                actual,
            })
        }
    }

    fn try_post(&self, generation: u64) -> PostOutcome {
        let Some(lease) = PostLease::acquire(self, generation) else {
            return PostOutcome::Inactive;
        };
        let state = unpack(self.state.load(Ordering::Acquire));
        if state.phase != HostPhase::Active || state.generation != generation {
            return PostOutcome::Inactive;
        }
        if self
            .owner_post_outstanding
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return PostOutcome::Coalesced;
        }
        let token = self.ready_token.load(Ordering::Acquire);
        let result = self.adapter.post_owner_turn(token, generation);
        let outcome = match result {
            HostPostResult::Accepted => PostOutcome::Accepted,
            HostPostResult::Closed => {
                self.owner_post_outstanding.store(false, Ordering::Release);
                self.transition_from_active(generation, HostPhase::Leaving);
                self.rebuild_work_pending();
                PostOutcome::Closed
            }
            HostPostResult::Failure => {
                self.owner_post_outstanding.store(false, Ordering::Release);
                self.transition_from_active(generation, HostPhase::Closing);
                self.rebuild_work_pending();
                PostOutcome::Failed
            }
        };
        drop(lease);
        outcome
    }

    fn transition_from_active(&self, generation: u64, target: HostPhase) {
        loop {
            let current_raw = self.state.load(Ordering::Acquire);
            let current = unpack(current_raw);
            if current.generation != generation || current.phase != HostPhase::Active {
                return;
            }
            let next = pack(target, generation, current.leases);
            if self
                .state
                .compare_exchange(current_raw, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                if current.leases == 0 {
                    self.lease_wait.notify_all();
                }
                return;
            }
        }
    }

    fn leave_generation(&self, generation: u64) -> Result<LeaveReport, HostLoopError> {
        self.require_owner()?;
        let integration_failed = loop {
            let current_raw = self.state.load(Ordering::Acquire);
            let current = unpack(current_raw);
            if current.generation != generation {
                return Err(HostLoopError::StaleGeneration);
            }
            match current.phase {
                HostPhase::Active => {
                    let next = pack(HostPhase::Leaving, generation, current.leases);
                    if self
                        .state
                        .compare_exchange(current_raw, next, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                    {
                        break false;
                    }
                }
                HostPhase::Leaving => break false,
                HostPhase::Closing => break true,
                HostPhase::Managed | HostPhase::Entering => {
                    return Err(HostLoopError::StaleGeneration);
                }
            }
        };
        let mut guard = self
            .lease_wait_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while unpack(self.state.load(Ordering::Acquire)).leases != 0 {
            guard = self
                .lease_wait
                .wait(guard)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        drop(guard);
        let token = self.ready_token.load(Ordering::Acquire);
        if !self.deactivated.swap(true, Ordering::AcqRel) {
            self.adapter.deactivate(token, generation);
        }
        self.ready_token.store(0, Ordering::Release);
        self.owner_post_outstanding.store(false, Ordering::Release);
        self.rebuild_work_pending();
        self.state
            .store(pack(HostPhase::Managed, generation, 0), Ordering::Release);
        Ok(LeaveReport {
            generation,
            integration_failed,
            needs_runtime_pump: self.queued_work.load(Ordering::Acquire) > 0,
        })
    }

    fn rebuild_work_pending(&self) {
        self.work_pending.store(
            self.queued_work.load(Ordering::Acquire) > 0,
            Ordering::Release,
        );
    }

    fn release_lease(&self, generation: u64) {
        loop {
            let current_raw = self.state.load(Ordering::Acquire);
            let current = unpack(current_raw);
            assert_eq!(
                current.generation, generation,
                "PostLease generation changed"
            );
            assert!(current.leases > 0, "PostLease underflow");
            let next = pack(current.phase, generation, current.leases - 1);
            if self
                .state
                .compare_exchange(current_raw, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                if current.leases == 1 && current.phase != HostPhase::Active {
                    let _guard = self
                        .lease_wait_lock
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    self.lease_wait.notify_all();
                }
                return;
            }
        }
    }
}

struct PostLease<'a> {
    controller: &'a HostLoopController,
    generation: u64,
}

impl<'a> PostLease<'a> {
    fn acquire(controller: &'a HostLoopController, generation: u64) -> Option<Self> {
        loop {
            let current_raw = controller.state.load(Ordering::Acquire);
            let current = unpack(current_raw);
            if current.phase != HostPhase::Active || current.generation != generation {
                return None;
            }
            if u64::from(current.leases) == LEASE_MASK {
                return None;
            }
            let next = pack(HostPhase::Active, generation, current.leases + 1);
            if controller
                .state
                .compare_exchange(current_raw, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Some(Self {
                    controller,
                    generation,
                });
            }
        }
    }
}

impl Drop for PostLease<'_> {
    fn drop(&mut self) {
        self.controller.release_lease(self.generation);
    }
}

#[derive(Clone, Copy)]
struct PackedState {
    phase: HostPhase,
    generation: u64,
    leases: u32,
}

fn pack(phase: HostPhase, generation: u64, leases: u32) -> u64 {
    assert!(generation <= MAX_GENERATION);
    assert!(u64::from(leases) <= LEASE_MASK);
    (generation << GENERATION_SHIFT) | (u64::from(leases) << LEASE_SHIFT) | phase as u64
}

fn unpack(value: u64) -> PackedState {
    let phase = match value & ((1_u64 << PHASE_BITS) - 1) {
        0 => HostPhase::Managed,
        1 => HostPhase::Entering,
        2 => HostPhase::Active,
        3 => HostPhase::Leaving,
        4 => HostPhase::Closing,
        _ => unreachable!("host phase uses a closed tag set"),
    };
    PackedState {
        phase,
        generation: value >> GENERATION_SHIFT,
        leases: ((value >> LEASE_SHIFT) & LEASE_MASK) as u32,
    }
}

unsafe extern "C" fn owner_turn_trampoline(context: *mut c_void, generation: u64) {
    if context.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: enter publishes a process-lifetime controller address to the
        // adapter, which must stop new delivery before runtime destruction.
        let controller = unsafe { &*context.cast::<HostLoopController>() };
        let _ = controller.owner_turn(generation);
    }));
}

pub struct CAbiHostLoopAdapter {
    profile: HostLoopProfileV1,
}

// SAFETY: validation applies the HostLoopProfileV1 contract: callbacks and
// context are process-lifetime and post_owner_turn is thread-safe.
unsafe impl Send for CAbiHostLoopAdapter {}
// SAFETY: callbacks may be invoked concurrently exactly as declared by the ABI.
unsafe impl Sync for CAbiHostLoopAdapter {}

impl CAbiHostLoopAdapter {
    pub fn new(profile: HostLoopProfileV1) -> Result<Self, HostLoopError> {
        if profile.struct_size < size_of::<HostLoopProfileV1>() as u32 {
            return Err(HostLoopError::InvalidProfile(
                "struct_size is too small".into(),
            ));
        }
        if !is_compatible(profile.abi_version) {
            return Err(HostLoopError::InvalidProfile(
                "ABI major version mismatch".into(),
            ));
        }
        if profile.prepare.is_none()
            || profile.post_owner_turn.is_none()
            || profile.deactivate.is_none()
        {
            return Err(HostLoopError::InvalidProfile(
                "prepare, postOwnerTurn and deactivate are required".into(),
            ));
        }
        if !matches!(
            profile.nested_delivery,
            HOST_NESTED_ALL_MODES | HOST_NESTED_DECLARED_MODES | HOST_NESTED_UNSUPPORTED
        ) {
            return Err(HostLoopError::InvalidProfile(
                "nested_delivery is unknown".into(),
            ));
        }
        if profile.return_policy != HOST_RETURN_RESUME_MANAGED {
            return Err(HostLoopError::InvalidProfile(
                "only resume-managed return policy is supported".into(),
            ));
        }
        if profile.turn_budget == 0 || profile.reserved != 0 {
            return Err(HostLoopError::InvalidProfile(
                "turn_budget must be non-zero and reserved must be zero".into(),
            ));
        }
        Ok(Self { profile })
    }

    #[must_use]
    pub fn turn_budget(&self) -> usize {
        self.profile.turn_budget as usize
    }
}

impl HostLoopAdapter for CAbiHostLoopAdapter {
    fn prepare(&self, owner: OwnerTurnTarget, generation: u64) -> Result<usize, String> {
        let mut token = std::ptr::null_mut();
        // SAFETY: constructor validation requires a callback, and the profile
        // contract owns all pointers for the duration of this call.
        let status = unsafe {
            self.profile.prepare.unwrap_unchecked()(
                self.profile.context,
                owner.callback,
                owner.context,
                generation,
                &mut token,
            )
        };
        if status == STATUS_OK && !token.is_null() {
            Ok(token as usize)
        } else {
            Err(format!("prepare returned status {status} or a null token"))
        }
    }

    fn post_owner_turn(&self, ready_token: usize, generation: u64) -> HostPostResult {
        // SAFETY: constructor validation requires this callback and the token is
        // the live value returned by prepare for this generation.
        match unsafe {
            self.profile.post_owner_turn.unwrap_unchecked()(
                self.profile.context,
                ready_token as *mut c_void,
                generation,
            )
        } {
            HOST_POST_ACCEPTED => HostPostResult::Accepted,
            HOST_POST_CLOSED => HostPostResult::Closed,
            HOST_POST_FAILURE => HostPostResult::Failure,
            _ => HostPostResult::Failure,
        }
    }

    fn deactivate(&self, ready_token: usize, generation: u64) {
        // SAFETY: constructor validation requires this callback. leave waits for
        // every PostLease before passing the generation-scoped token here.
        unsafe {
            self.profile.deactivate.unwrap_unchecked()(
                self.profile.context,
                ready_token as *mut c_void,
                generation,
            );
        }
    }
}
