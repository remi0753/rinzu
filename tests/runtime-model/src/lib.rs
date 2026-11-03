//! Deterministic executable models for Rinzu runtime protocols.
//!
//! The first model covers the Host Loop lifecycle. Every method in this file
//! represents one atomic step from the normative protocol; no OS threads are
//! used. The explorer enumerates every enabled ordering for a bounded number of
//! producers and preserves the exact operation trace for a counterexample.

use std::collections::{HashSet, VecDeque};
use std::error::Error;
use std::fmt::{self, Display, Formatter};

pub mod binding;
pub mod blocking_pool;
pub mod callback_registration;
pub mod cancellation;
pub mod diagnostics;
pub mod gc_transition;
pub mod host_adapters;
pub mod host_stress;
pub mod lifecycle;
pub mod reload;
pub mod settlement;
pub mod transport;

/// Result returned by a Host Loop profile's `postOwnerTurn` operation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PostResult {
    Accepted,
    Closed,
    Failure,
}

/// The single atomic lifecycle word described by `DESIGN_V2_DETAIL.md`.
///
/// `Active` has open post admission. `Leaving` and `Closing` have closed post
/// admission. A zero `in_flight` count is the model's `PostClosed` state.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HostState {
    Managed,
    Active { generation: u32, in_flight: u8 },
    Leaving { generation: u32, in_flight: u8 },
    Closing { generation: u32, in_flight: u8 },
}

impl HostState {
    #[must_use]
    pub const fn in_flight(self) -> u8 {
        match self {
            Self::Managed => 0,
            Self::Active { in_flight, .. }
            | Self::Leaving { in_flight, .. }
            | Self::Closing { in_flight, .. } => in_flight,
        }
    }

    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Active { .. })
    }
}

/// One producer's progress through `enqueue -> tryPost -> lease release`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ProducerStage {
    New,
    Enqueued,
    LeaseHeld,
    ActiveVerified,
    OutstandingClaimed,
    PostReturned,
    SkipPost,
    Done,
}

impl ProducerStage {
    const fn holds_lease(self) -> bool {
        matches!(
            self,
            Self::LeaseHeld
                | Self::ActiveVerified
                | Self::OutstandingClaimed
                | Self::PostReturned
                | Self::SkipPost
        )
    }
}

/// Progress of the process-main-thread owner around the synchronous Host Loop
/// entry call.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OwnerStage {
    InHostLoop,
    Returned,
    LeaveStarted,
    Deactivated,
}

/// An atomic action selected by the deterministic scheduler.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Action {
    Enqueue(usize),
    AcquirePostLease(usize),
    VerifyActive(usize),
    ClaimOutstanding(usize),
    PostReturns(usize, PostResult),
    ReleasePostLease(usize),
    ObserveClosedAdmission(usize),
    HostEntryReturns,
    OwnerContinuesLeave,
    DeliverOwnerCallback,
    Deactivate,
}

impl Display for Action {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Enqueue(id) => write!(formatter, "p{id}: enqueue"),
            Self::AcquirePostLease(id) => write!(formatter, "p{id}: acquire PostLease"),
            Self::VerifyActive(id) => write!(formatter, "p{id}: verify Active"),
            Self::ClaimOutstanding(id) => write!(formatter, "p{id}: claim outstanding"),
            Self::PostReturns(id, result) => {
                write!(formatter, "p{id}: postOwnerTurn -> {result:?}")
            }
            Self::ReleasePostLease(id) => write!(formatter, "p{id}: release PostLease"),
            Self::ObserveClosedAdmission(id) => {
                write!(formatter, "p{id}: observe closed post admission")
            }
            Self::HostEntryReturns => formatter.write_str("owner: Host Loop entry returns"),
            Self::OwnerContinuesLeave => formatter.write_str("owner: continue leave"),
            Self::DeliverOwnerCallback => formatter.write_str("host: deliver owner callback"),
            Self::Deactivate => formatter.write_str("owner: deactivate adapter"),
        }
    }
}

/// Complete state explored by the reference scheduler.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Model {
    pub host: HostState,
    pub producers: Vec<ProducerStage>,
    pub outcomes: Vec<PostResult>,
    pub work_pending: bool,
    pub owner_post_outstanding: bool,
    pub native_callbacks_pending: u8,
    pub owner: OwnerStage,
    pub runtime_pump_scheduled: bool,
    pub fail_fast_count: u8,
    pub managed_turns: u32,
    pub late_callback_noops: u32,
}

impl Model {
    /// Create a Host Loop already in `Active(g, PostOpen(0))`.
    #[must_use]
    pub fn active(outcomes: Vec<PostResult>) -> Self {
        let producer_count = outcomes.len();
        Self {
            host: HostState::Active {
                generation: 1,
                in_flight: 0,
            },
            producers: vec![ProducerStage::New; producer_count],
            outcomes,
            work_pending: false,
            owner_post_outstanding: false,
            native_callbacks_pending: 0,
            owner: OwnerStage::InHostLoop,
            runtime_pump_scheduled: false,
            fail_fast_count: 0,
            managed_turns: 0,
            late_callback_noops: 0,
        }
    }

    /// Enumerate all actions enabled in this state.
    #[must_use]
    pub fn enabled_actions(&self) -> Vec<Action> {
        // A fail-fast process does not run managed cleanup. OS process teardown
        // owns the remaining native allocations from this point.
        if self.fail_fast_count > 0 {
            return Vec::new();
        }

        let mut actions = Vec::new();

        for (id, stage) in self.producers.iter().copied().enumerate() {
            match stage {
                ProducerStage::New => actions.push(Action::Enqueue(id)),
                ProducerStage::Enqueued => {
                    if self.host.is_active() {
                        actions.push(Action::AcquirePostLease(id));
                    } else {
                        actions.push(Action::ObserveClosedAdmission(id));
                    }
                }
                ProducerStage::LeaseHeld => actions.push(Action::VerifyActive(id)),
                ProducerStage::ActiveVerified => actions.push(Action::ClaimOutstanding(id)),
                ProducerStage::OutstandingClaimed => {
                    actions.push(Action::PostReturns(id, self.outcomes[id]));
                }
                ProducerStage::PostReturned | ProducerStage::SkipPost => {
                    actions.push(Action::ReleasePostLease(id));
                }
                ProducerStage::Done => {}
            }
        }

        if self.owner == OwnerStage::InHostLoop {
            actions.push(Action::HostEntryReturns);
        }

        if self.owner == OwnerStage::Returned
            && matches!(
                self.host,
                HostState::Active { .. } | HostState::Leaving { .. }
            )
        {
            actions.push(Action::OwnerContinuesLeave);
        }

        if self.native_callbacks_pending > 0 {
            actions.push(Action::DeliverOwnerCallback);
        }

        if self.owner == OwnerStage::LeaveStarted
            && matches!(self.host, HostState::Leaving { in_flight: 0, .. })
        {
            actions.push(Action::Deactivate);
        }

        actions
    }

    /// Apply one enabled atomic action.
    ///
    /// # Errors
    ///
    /// Returns an error when the requested action is not enabled or would
    /// violate a checked arithmetic bound.
    #[allow(clippy::too_many_lines)]
    pub fn apply(&mut self, action: Action) -> Result<(), String> {
        match action {
            Action::Enqueue(id) => {
                self.require_stage(id, ProducerStage::New)?;
                self.producers[id] = ProducerStage::Enqueued;
                self.work_pending = true;
                if matches!(self.host, HostState::Managed) {
                    self.runtime_pump_scheduled = true;
                }
            }
            Action::AcquirePostLease(id) => {
                self.require_stage(id, ProducerStage::Enqueued)?;
                self.host = match self.host {
                    HostState::Active {
                        generation,
                        in_flight,
                    } => HostState::Active {
                        generation,
                        in_flight: in_flight
                            .checked_add(1)
                            .ok_or_else(|| "PostLease counter overflow".to_owned())?,
                    },
                    state => return Err(format!("cannot acquire PostLease in {state:?}")),
                };
                self.producers[id] = ProducerStage::LeaseHeld;
            }
            Action::VerifyActive(id) => {
                self.require_stage(id, ProducerStage::LeaseHeld)?;
                self.producers[id] = if self.host.is_active() {
                    ProducerStage::ActiveVerified
                } else {
                    ProducerStage::SkipPost
                };
            }
            Action::ClaimOutstanding(id) => {
                self.require_stage(id, ProducerStage::ActiveVerified)?;
                if self.owner_post_outstanding {
                    self.producers[id] = ProducerStage::SkipPost;
                } else {
                    self.owner_post_outstanding = true;
                    self.producers[id] = ProducerStage::OutstandingClaimed;
                }
            }
            Action::PostReturns(id, result) => {
                self.require_stage(id, ProducerStage::OutstandingClaimed)?;
                if self.outcomes[id] != result {
                    return Err(format!(
                        "producer {id} expected {:?}, received {result:?}",
                        self.outcomes[id]
                    ));
                }

                match result {
                    PostResult::Accepted => {
                        self.native_callbacks_pending = self
                            .native_callbacks_pending
                            .checked_add(1)
                            .ok_or_else(|| "native callback counter overflow".to_owned())?;
                    }
                    PostResult::Closed => {
                        self.owner_post_outstanding = false;
                        if let HostState::Active {
                            generation,
                            in_flight,
                        } = self.host
                        {
                            self.host = HostState::Leaving {
                                generation,
                                in_flight,
                            };
                        }
                    }
                    PostResult::Failure => {
                        self.owner_post_outstanding = false;
                        if let HostState::Active {
                            generation,
                            in_flight,
                        } = self.host
                        {
                            self.host = HostState::Closing {
                                generation,
                                in_flight,
                            };
                            self.fail_fast_count = self
                                .fail_fast_count
                                .checked_add(1)
                                .ok_or_else(|| "fail-fast counter overflow".to_owned())?;
                        }
                    }
                }
                self.producers[id] = ProducerStage::PostReturned;
            }
            Action::ReleasePostLease(id) => {
                let stage = self.producer_stage(id)?;
                if !matches!(stage, ProducerStage::PostReturned | ProducerStage::SkipPost) {
                    return Err(format!("producer {id} cannot release lease from {stage:?}"));
                }

                self.host = decrement_lease(self.host)?;
                self.producers[id] = ProducerStage::Done;
            }
            Action::ObserveClosedAdmission(id) => {
                self.require_stage(id, ProducerStage::Enqueued)?;
                if self.host.is_active() {
                    return Err("post admission is still open".to_owned());
                }
                if matches!(self.host, HostState::Managed) {
                    self.runtime_pump_scheduled = true;
                }
                self.producers[id] = ProducerStage::Done;
            }
            Action::HostEntryReturns => {
                if self.owner != OwnerStage::InHostLoop {
                    return Err("Host Loop entry returned twice".to_owned());
                }
                self.owner = OwnerStage::Returned;
            }
            Action::OwnerContinuesLeave => {
                if self.owner != OwnerStage::Returned {
                    return Err("owner cannot continue leave in this state".to_owned());
                }
                self.host = match self.host {
                    HostState::Active {
                        generation,
                        in_flight,
                    } => HostState::Leaving {
                        generation,
                        in_flight,
                    },
                    state @ HostState::Leaving { .. } => state,
                    state => return Err(format!("owner cannot continue leave from {state:?}")),
                };
                self.owner = OwnerStage::LeaveStarted;
            }
            Action::DeliverOwnerCallback => {
                if self.native_callbacks_pending == 0 {
                    return Err("no owner callback is pending".to_owned());
                }
                self.native_callbacks_pending -= 1;
                if self.host.is_active() {
                    self.owner_post_outstanding = false;
                    self.work_pending = false;
                    self.managed_turns = self
                        .managed_turns
                        .checked_add(1)
                        .ok_or_else(|| "managed turn counter overflow".to_owned())?;
                } else {
                    self.late_callback_noops = self
                        .late_callback_noops
                        .checked_add(1)
                        .ok_or_else(|| "late callback counter overflow".to_owned())?;
                }
            }
            Action::Deactivate => {
                if self.owner != OwnerStage::LeaveStarted {
                    return Err("adapter cannot deactivate in this state".to_owned());
                }
                if !matches!(self.host, HostState::Leaving { in_flight: 0, .. }) {
                    return Err(format!(
                        "deactivate requires Leaving(PostClosed), found {:?}",
                        self.host
                    ));
                }
                self.host = HostState::Managed;
                self.owner = OwnerStage::Deactivated;
                self.owner_post_outstanding = false;
                if self.work_pending {
                    self.runtime_pump_scheduled = true;
                }
            }
        }

        Ok(())
    }

    /// Validate safety invariants that must hold after every atomic action.
    ///
    /// # Errors
    ///
    /// Returns a human-readable invariant violation.
    pub fn check_invariants(&self) -> Result<(), String> {
        if self.producers.len() != self.outcomes.len() {
            return Err("producer/outcome length mismatch".to_owned());
        }

        let lease_holders = self
            .producers
            .iter()
            .filter(|stage| stage.holds_lease())
            .count();
        if usize::from(self.host.in_flight()) != lease_holders {
            return Err(format!(
                "hostState tracks {} leases but {lease_holders} producers hold one",
                self.host.in_flight()
            ));
        }

        if self.fail_fast_count > 1 {
            return Err(format!(
                "fail-fast must have one winner, observed {}",
                self.fail_fast_count
            ));
        }

        match self.host {
            HostState::Managed => {
                if self.owner != OwnerStage::Deactivated {
                    return Err("Managed state requires completed deactivate".to_owned());
                }
            }
            HostState::Active { .. } => {
                if matches!(
                    self.owner,
                    OwnerStage::LeaveStarted | OwnerStage::Deactivated
                ) {
                    return Err("owner leave started while hostState is Active".to_owned());
                }
            }
            HostState::Leaving { .. } => {
                if self.fail_fast_count != 0 {
                    return Err("Leaving and fail-fast ownership cannot coexist".to_owned());
                }
            }
            HostState::Closing { .. } => {
                if self.fail_fast_count != 1 {
                    return Err("Closing must have exactly one fail-fast owner".to_owned());
                }
                if self.owner == OwnerStage::Deactivated {
                    return Err("failure path must not run normal deactivate".to_owned());
                }
            }
        }

        if self.owner == OwnerStage::Deactivated && self.fail_fast_count != 0 {
            return Err("normal deactivate and fail-fast both occurred".to_owned());
        }

        if self.owner == OwnerStage::Deactivated
            && self.work_pending
            && !self.runtime_pump_scheduled
        {
            return Err("pending work was not handed to the runtime pump".to_owned());
        }

        if self.owner_post_outstanding && self.host.is_active() {
            let post_call_in_progress = self
                .producers
                .iter()
                .any(|stage| matches!(stage, ProducerStage::OutstandingClaimed));
            if !post_call_in_progress && self.native_callbacks_pending == 0 {
                return Err("outstanding flag has no post call or native callback".to_owned());
            }
        }

        Ok(())
    }

    #[must_use]
    pub fn is_terminal(&self) -> bool {
        if self.fail_fast_count == 1 {
            return true;
        }

        self.owner == OwnerStage::Deactivated
            && matches!(self.host, HostState::Managed)
            && self
                .producers
                .iter()
                .all(|stage| matches!(stage, ProducerStage::Done))
            && self.native_callbacks_pending == 0
            && (!self.work_pending || self.runtime_pump_scheduled)
    }

    fn producer_stage(&self, id: usize) -> Result<ProducerStage, String> {
        self.producers
            .get(id)
            .copied()
            .ok_or_else(|| format!("unknown producer {id}"))
    }

    fn require_stage(&self, id: usize, expected: ProducerStage) -> Result<(), String> {
        let actual = self.producer_stage(id)?;
        if actual == expected {
            Ok(())
        } else {
            Err(format!(
                "producer {id} expected stage {expected:?}, found {actual:?}"
            ))
        }
    }
}

fn decrement_lease(state: HostState) -> Result<HostState, String> {
    match state {
        HostState::Active {
            generation,
            in_flight,
        } => Ok(HostState::Active {
            generation,
            in_flight: in_flight
                .checked_sub(1)
                .ok_or_else(|| "Active lease counter underflow".to_owned())?,
        }),
        HostState::Leaving {
            generation,
            in_flight,
        } => Ok(HostState::Leaving {
            generation,
            in_flight: in_flight
                .checked_sub(1)
                .ok_or_else(|| "Leaving lease counter underflow".to_owned())?,
        }),
        HostState::Closing {
            generation,
            in_flight,
        } => Ok(HostState::Closing {
            generation,
            in_flight: in_flight
                .checked_sub(1)
                .ok_or_else(|| "Closing lease counter underflow".to_owned())?,
        }),
        HostState::Managed => Err("cannot release PostLease in Managed state".to_owned()),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExplorationReport {
    pub visited_states: usize,
    pub graceful_terminals: usize,
    pub fail_fast_terminals: usize,
    pub maximum_depth: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Counterexample {
    pub reason: String,
    pub trace: Vec<Action>,
    pub state: Model,
}

impl Display for Counterexample {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        writeln!(formatter, "{}", self.reason)?;
        for (index, action) in self.trace.iter().enumerate() {
            writeln!(formatter, "{index:04}: {action}")?;
        }
        write!(formatter, "final state: {:#?}", self.state)
    }
}

impl Error for Counterexample {}

#[derive(Clone)]
struct SearchNode {
    state: Model,
    trace: Vec<Action>,
}

/// Exhaustively explore every enabled operation ordering.
///
/// # Errors
///
/// Returns the shortest discovered invariant violation, deadlock, invalid
/// transition, or an explicit state-space limit failure.
pub fn explore(initial: Model, state_limit: usize) -> Result<ExplorationReport, Counterexample> {
    if let Err(reason) = initial.check_invariants() {
        return Err(Counterexample {
            reason,
            trace: Vec::new(),
            state: initial,
        });
    }

    let mut visited = HashSet::new();
    visited.insert(initial.clone());

    let mut queue = VecDeque::new();
    queue.push_back(SearchNode {
        state: initial,
        trace: Vec::new(),
    });

    let mut graceful_terminals = 0;
    let mut fail_fast_terminals = 0;
    let mut maximum_depth = 0;

    while let Some(node) = queue.pop_front() {
        maximum_depth = maximum_depth.max(node.trace.len());
        let actions = node.state.enabled_actions();

        if actions.is_empty() {
            if !node.state.is_terminal() {
                return Err(Counterexample {
                    reason: "reachable non-terminal state has no enabled action".to_owned(),
                    trace: node.trace,
                    state: node.state,
                });
            }
            if node.state.fail_fast_count == 1 {
                fail_fast_terminals += 1;
            } else {
                graceful_terminals += 1;
            }
            continue;
        }

        for action in actions {
            let mut next = node.state.clone();
            let mut trace = node.trace.clone();
            trace.push(action);

            if let Err(reason) = next.apply(action) {
                return Err(Counterexample {
                    reason,
                    trace,
                    state: next,
                });
            }
            if let Err(reason) = next.check_invariants() {
                return Err(Counterexample {
                    reason,
                    trace,
                    state: next,
                });
            }

            if visited.insert(next.clone()) {
                if visited.len() > state_limit {
                    return Err(Counterexample {
                        reason: format!("state-space limit {state_limit} exceeded"),
                        trace,
                        state: next,
                    });
                }
                queue.push_back(SearchNode { state: next, trace });
            }
        }
    }

    Ok(ExplorationReport {
        visited_states: visited.len(),
        graceful_terminals,
        fail_fast_terminals,
        maximum_depth,
    })
}
