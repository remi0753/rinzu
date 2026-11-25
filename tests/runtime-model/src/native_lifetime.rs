//! Finite, sequentially consistent abstractions of NLS01–NLS09.
//!
//! These are protocol models, not an interpretation of production C or Rust.
//! Components have explicit interface assumptions; their separate searches are
//! not a proof of their unbounded asynchronous product or of weak memory.

mod admission;
mod batch;
mod call;
mod cohort;
mod cohort_replay;
mod completion;
mod composed;
mod deferred_poll;
mod domain_chain;
mod domain_reuse;
mod domains;
mod domains_replay;
mod extended_plan;
mod handoff;
mod handoff_replay;
mod joined;
mod joined_handoff;
mod joined_replay;
mod notification_batches;
mod parallel_registration;
mod polling;
mod ready_selection;
mod recovery;
mod registration;
mod registration_replay;
mod replay;
mod search;
mod series;
mod shared_replay;
mod shared_target;
mod wake;

pub use admission::{Admission, AdmissionFault};
pub use batch::BatchSchedule;
pub use call::{Call, CallFault};
pub use cohort::{Cohort, CohortAction, CohortFault, CohortLayout, CohortMode};
pub use cohort_replay::{
    replay_batched, replay_cohort, replay_combination, replay_extended, replay_mixed_batches,
    replay_unordered,
};
pub use completion::{Completion, CompletionAction, CompletionFault, Decision, ReceiptStage};
pub use composed::{Binding, Composed, ComposedAction, ComposedFault};
pub use deferred_poll::{DeferredPoll, DeferredPollAction};
pub use domain_chain::{ChainAction, ChainFault, DomainChain, chain_orders, consecutive_orders};
pub use domain_reuse::{
    DomainReuse, DomainReuseAction, DomainReuseFault, StaleControl, reuse_orders,
};
pub use domains::{Domain, DomainAction, DomainBatches, DomainFault, DomainReport};
pub use domains_replay::{
    DomainChainReplayReport, DomainReplayReport, DomainReuseReplayReport, JoinedDomainReplayReport,
    replay_consecutive_domains, replay_domain_chain, replay_domains, replay_joined_handoff,
    replay_reused_domains,
};
pub use handoff::{Handoff, HandoffAction, HandoffFault};
pub use handoff_replay::replay_handoff;
pub use joined::{Joined, JoinedAction, JoinedFault};
pub use joined_handoff::{JoinedHandoff, JoinedHandoffAction};
pub use joined_replay::replay_joined;
pub use notification_batches::{NotificationAction, NotificationBatches};
pub use parallel_registration::{ParallelRegistration, ParallelRegistrationAction};
pub use ready_selection::{ReadyAction, ReadyInput, ReadySelection};
pub use registration::{Registration, RegistrationAction, RegistrationFault};
pub use registration_replay::replay;
pub use replay::{ReplayReport, replay_registered};
pub use search::{Counterexample, FailureKind, Protocol, Report, Violation, explore};
pub use series::MixedBatchSchedule;
pub use shared_replay::replay_shared;
pub use shared_target::{SharedAction, SharedFault, SharedTarget, SharedTopology};
pub use wake::{Wake, WakeFault};

/// Every profile is exhausted, or an error is returned. Limits never mean success.
///
/// # Errors
/// Returns an invariant violation, deadlock, or incomplete search.
pub fn verify(state_limit: usize) -> Result<Vec<Report>, Counterexample> {
    let mut reports = Vec::new();
    for version in [1, 2, 3] {
        for helper in [false, true] {
            reports.push(explore(
                Completion::new(version, 2, 1, helper, true),
                state_limit,
            )?);
        }
    }
    reports.push(explore(Completion::new(3, 1, 2, true, true), state_limit)?);
    reports.push(explore(Completion::new(3, 1, 1, true, false), state_limit)?);
    for callbacks in [1, 2] {
        for closed_consumer in [false, true] {
            reports.push(explore(Call::new(callbacks, closed_consumer), state_limit)?);
        }
    }
    reports.push(explore(Wake::new(), state_limit)?);
    for full in [false, true] {
        reports.push(explore(Admission::new(full), state_limit)?);
    }
    Ok(reports)
}
