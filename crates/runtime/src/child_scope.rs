//! Parent-task ownership and observation of immutable child completions.
//! Lexical scope exit does not alter this ledger. Close seals admission and
//! waits for all children, including children whose Future binding was dropped.

use std::collections::{BTreeMap, BTreeSet};
use std::thread::{self, ThreadId};

use crate::failure::{Diagnostic, ErrorKind, Failure, FailureCause, SourcePosition};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChildScopeError {
    ForeignOwner,
    Closed,
    DuplicateChild,
    MissingChild,
    AlreadyCompleted,
    PendingChildren,
    NotClosing,
    NonRecoverable,
}
impl std::fmt::Display for ChildScopeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "child scope protocol error: {self:?}")
    }
}
impl std::error::Error for ChildScopeError {}

#[derive(Debug)]
struct Child {
    completion: Option<Option<Failure>>,
    observed: bool,
    expected_control: bool,
}

#[derive(Debug)]
pub struct ChildScope {
    owner: ThreadId,
    children: BTreeMap<u64, Child>,
    closing: bool,
    finished: bool,
    outcome: Option<Failure>,
    collect_all: bool,
    errors: Vec<Failure>,
    origins: BTreeSet<u64>,
}

impl Default for ChildScope {
    fn default() -> Self {
        Self::new()
    }
}

impl ChildScope {
    pub fn new() -> Self {
        Self {
            owner: thread::current().id(),
            children: BTreeMap::new(),
            closing: false,
            finished: false,
            outcome: None,
            collect_all: false,
            errors: Vec::new(),
            origins: BTreeSet::new(),
        }
    }
    pub fn all() -> Self {
        Self {
            collect_all: true,
            ..Self::new()
        }
    }
    pub fn cancels_on_failure(&self, failure: &Failure) -> bool {
        !self.collect_all || !failure.recoverable()
    }
    /// A child origin is stable across repeated await and the physical join.
    /// Statements without a child origin are independent failures even at one source span.
    pub fn record_error(
        &mut self,
        origin: Option<u64>,
        failure: Failure,
    ) -> Result<(), ChildScopeError> {
        self.owner()?;
        if self.finished || !self.collect_all {
            return Err(ChildScopeError::Closed);
        }
        if !failure.recoverable() {
            return Err(ChildScopeError::NonRecoverable);
        }
        if let Some(origin) = origin
            && !self.origins.insert(origin)
        {
            return Ok(());
        }
        self.errors.push(failure);
        Ok(())
    }
    pub fn errors(&self) -> Vec<Failure> {
        self.errors.clone()
    }
    fn owner(&self) -> Result<(), ChildScopeError> {
        if thread::current().id() == self.owner {
            Ok(())
        } else {
            Err(ChildScopeError::ForeignOwner)
        }
    }
    pub fn admit(&mut self, identity: u64) -> Result<(), ChildScopeError> {
        self.admit_with_lease(identity, false)
    }
    /// Only an already admitted native callback may extend a sealing ledger.
    pub fn admit_callback(&mut self, identity: u64) -> Result<(), ChildScopeError> {
        self.admit_with_lease(identity, true)
    }
    pub fn is_finished(&self) -> bool {
        self.finished
    }
    /// Transfer owned records to the runtime supervisor without observing them.
    /// The supervisor may already be draining after Main's normal return.
    pub fn handoff(&mut self, supervisor: &mut Self) -> Result<(), ChildScopeError> {
        self.owner()?;
        supervisor.owner()?;
        if self.closing || self.finished || supervisor.finished {
            return Err(ChildScopeError::Closed);
        }
        if self
            .children
            .keys()
            .any(|identity| supervisor.children.contains_key(identity))
        {
            return Err(ChildScopeError::DuplicateChild);
        }
        supervisor.children.append(&mut self.children);
        Ok(())
    }
    fn admit_with_lease(&mut self, identity: u64, callback: bool) -> Result<(), ChildScopeError> {
        self.owner()?;
        if self.finished || (self.closing && !callback) {
            return Err(ChildScopeError::Closed);
        }
        if self.children.contains_key(&identity) {
            return Err(ChildScopeError::DuplicateChild);
        }
        self.children.insert(
            identity,
            Child {
                completion: None,
                observed: false,
                expected_control: false,
            },
        );
        Ok(())
    }
    pub fn complete(
        &mut self,
        identity: u64,
        failure: Option<Failure>,
    ) -> Result<(), ChildScopeError> {
        self.owner()?;
        let child = self
            .children
            .get_mut(&identity)
            .ok_or(ChildScopeError::MissingChild)?;
        if child.completion.is_some() {
            return Err(ChildScopeError::AlreadyCompleted);
        }
        child.completion = Some(failure);
        Ok(())
    }
    /// Roll back a reservation before a Future is returned or work submitted.
    pub fn withdraw_admission(&mut self, identity: u64) -> Result<(), ChildScopeError> {
        self.owner()?;
        let child = self
            .children
            .get(&identity)
            .ok_or(ChildScopeError::MissingChild)?;
        if child.completion.is_some() {
            return Err(ChildScopeError::AlreadyCompleted);
        }
        self.children.remove(&identity);
        Ok(())
    }
    /// Observing a Failed Future never changes its immutable completion.
    pub fn observe(&mut self, identity: u64) -> Result<(), ChildScopeError> {
        self.owner()?;
        let child = self
            .children
            .get_mut(&identity)
            .ok_or(ChildScopeError::MissingChild)?;
        if child.completion.is_none() {
            return Err(ChildScopeError::PendingChildren);
        }
        child.observed = true;
        Ok(())
    }
    /// Retire only the completed, observed parent ledger. Observation has
    /// transferred any failure to the awaiting task's routes. Callers must
    /// independently retain the Future, immutable result/failure and physical
    /// resources until their own consume/GC/release rules permit retirement.
    pub fn retire_observed(&mut self, identity: u64) -> Result<(), ChildScopeError> {
        self.owner()?;
        let child = self
            .children
            .get(&identity)
            .ok_or(ChildScopeError::MissingChild)?;
        if child.completion.is_none() || !child.observed {
            return Err(ChildScopeError::PendingChildren);
        }
        self.children.remove(&identity);
        Ok(())
    }
    /// GC may retire an unreachable completed Future without turning an
    /// unobserved failure into a successful observation.
    pub fn retire_unreferenced(&mut self, identity: u64) -> Result<bool, ChildScopeError> {
        self.owner()?;
        let child = self
            .children
            .get(&identity)
            .ok_or(ChildScopeError::MissingChild)?;
        let Some(completion) = &child.completion else {
            return Ok(false);
        };
        let safe = completion.as_ref().is_none_or(|failure| {
            child.observed
                || (child.expected_control
                    && failure.cleanup.is_empty()
                    && matches!(
                        failure.primary.cause,
                        FailureCause::Cancelled | FailureCause::Shutdown
                    ))
        });
        if safe {
            self.children.remove(&identity);
        }
        Ok(safe)
    }
    /// A requested cancellation suppresses only its control primary, never cleanup.
    pub fn expect_control(&mut self, identity: u64) -> Result<(), ChildScopeError> {
        self.owner()?;
        self.children
            .get_mut(&identity)
            .ok_or(ChildScopeError::MissingChild)?
            .expected_control = true;
        Ok(())
    }
    pub fn begin_close(&mut self, outcome: Option<Failure>) -> Result<(), ChildScopeError> {
        self.owner()?;
        if self.closing {
            return Err(ChildScopeError::Closed);
        }
        self.closing = true;
        self.outcome = outcome;
        Ok(())
    }
    pub fn pending(&self) -> Result<Vec<u64>, ChildScopeError> {
        self.owner()?;
        Ok(self
            .children
            .iter()
            .filter_map(|(identity, child)| child.completion.is_none().then_some(*identity))
            .collect())
    }
    /// A task can be cancelled while its successful body is joining children.
    /// Preserve an earlier failure; a new cancellation only replaces success.
    pub fn cancel_close(&mut self, source: Option<SourcePosition>) -> Result<(), ChildScopeError> {
        self.interrupt_close(FailureCause::Cancelled, source)
    }
    pub fn shutdown_close(
        &mut self,
        source: Option<SourcePosition>,
    ) -> Result<(), ChildScopeError> {
        self.interrupt_close(FailureCause::Shutdown, source)
    }
    fn interrupt_close(
        &mut self,
        cause: FailureCause,
        source: Option<SourcePosition>,
    ) -> Result<(), ChildScopeError> {
        self.owner()?;
        if !self.closing {
            return Err(ChildScopeError::NotClosing);
        }
        if self.finished {
            return Err(ChildScopeError::Closed);
        }
        if self.outcome.is_none() {
            self.outcome = Some(Failure::new(Diagnostic { cause, source }));
        }
        Ok(())
    }
    /// Deterministic creation identities order reports at the close boundary;
    /// a Worker's completion race cannot replace another child's diagnostics.
    pub fn finish_close(&mut self) -> Result<Option<Failure>, ChildScopeError> {
        self.owner()?;
        if !self.closing {
            return Err(ChildScopeError::NotClosing);
        }
        if self.finished {
            return Ok(self.outcome.clone());
        }
        if self
            .children
            .values()
            .any(|child| child.completion.is_none())
        {
            return Err(ChildScopeError::PendingChildren);
        }
        if self.collect_all {
            return self.finish_all();
        }
        let parent_failed = self.outcome.is_some();
        for child in self.children.values() {
            if child.observed {
                continue;
            }
            let Some(failure) = child.completion.as_ref().unwrap() else {
                continue;
            };
            if child.expected_control && failure.primary.cause == FailureCause::Cancelled {
                if failure.cleanup.is_empty() {
                    continue;
                }
                if self.outcome.is_none() {
                    self.outcome = Some(Failure::new(Diagnostic {
                        cause: FailureCause::Error(ErrorKind::UnobservedChildFailure),
                        source: failure.cleanup[0].source,
                    }));
                }
                self.outcome
                    .as_mut()
                    .unwrap()
                    .append_cleanup(failure.clone().without_primary().unwrap());
                continue;
            }
            if !parent_failed && self.outcome.is_none() && !failure.recoverable() {
                self.outcome = Some(failure.clone());
                continue;
            }
            if !parent_failed && self.outcome.is_none() {
                self.outcome = Some(Failure::new(Diagnostic {
                    cause: FailureCause::Error(ErrorKind::UnobservedChildFailure),
                    source: failure.primary.source,
                }));
            }
            let outcome = self.outcome.as_mut().unwrap();
            // Cancellation requested by a closing parent is expected. Keep its
            // new cleanup failures without reporting a second cancellation.
            if !parent_failed
                || !(failure.primary.cause == FailureCause::Cancelled
                    || (outcome.primary.cause == FailureCause::Shutdown
                        && failure.primary.cause == FailureCause::Shutdown))
            {
                outcome.append_cleanup(failure.clone());
            } else if let Some(cleanup) = failure.clone().without_primary() {
                outcome.append_cleanup(cleanup);
            }
        }
        self.children.clear();
        self.finished = true;
        Ok(self.outcome.clone())
    }
    fn finish_all(&mut self) -> Result<Option<Failure>, ChildScopeError> {
        if self.outcome.as_ref().is_some_and(Failure::recoverable) {
            let failure = self.outcome.take().unwrap();
            self.record_error(None, failure)?;
        }
        for (identity, child) in std::mem::take(&mut self.children) {
            if child.observed {
                continue;
            }
            let Some(mut failure) = child.completion.unwrap() else {
                continue;
            };
            let expected_control = (child.expected_control
                && failure.primary.cause == FailureCause::Cancelled)
                || self.outcome.as_ref().is_some_and(|outcome| {
                    failure.primary.cause == FailureCause::Cancelled
                        || (outcome.primary.cause == FailureCause::Shutdown
                            && failure.primary.cause == FailureCause::Shutdown)
                });
            if expected_control {
                if failure.cleanup.is_empty() {
                    continue;
                }
                failure = failure.without_primary().unwrap();
            }
            if failure.recoverable() {
                self.record_error(Some(identity), failure)?;
            } else if let Some(outcome) = &mut self.outcome {
                outcome.append_cleanup(failure);
            } else {
                self.outcome = Some(failure);
            }
        }
        for failure in &self.errors {
            if let Some(outcome) = &mut self.outcome {
                outcome.append_cleanup(failure.clone());
            } else {
                self.outcome = Some(failure.clone());
            }
        }
        self.finished = true;
        Ok(self.outcome.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::failure::SourcePosition;

    #[test]
    fn only_unsubmitted_admissions_can_be_withdrawn() {
        let mut scope = ChildScope::new();
        scope.admit(1).unwrap();
        scope.withdraw_admission(1).unwrap();
        assert_eq!(scope.complete(1, None), Err(ChildScopeError::MissingChild));
        scope.admit(2).unwrap();
        scope
            .complete(2, Some(Failure::new(FailureCause::ArithmeticTrap)))
            .unwrap();
        assert_eq!(
            scope.withdraw_admission(2),
            Err(ChildScopeError::AlreadyCompleted)
        );
        scope.begin_close(None).unwrap();
        assert_eq!(
            scope.finish_close().unwrap().unwrap().primary.cause,
            FailureCause::ArithmeticTrap
        );
    }

    #[test]
    fn unreachable_handles_do_not_observe_unhandled_failures_or_cancel_cleanup() {
        let mut scope = ChildScope::new();
        scope.admit(0).unwrap();
        scope.complete(0, None).unwrap();
        assert!(scope.retire_unreferenced(0).unwrap());
        scope.admit(1).unwrap();
        scope
            .complete(
                1,
                Some(Failure::new(FailureCause::Error(ErrorKind::Conversion))),
            )
            .unwrap();
        assert!(!scope.retire_unreferenced(1).unwrap());
        scope.observe(1).unwrap();
        assert!(scope.retire_unreferenced(1).unwrap());
        scope.admit(2).unwrap();
        scope
            .complete(2, Some(Failure::new(FailureCause::Cancelled)))
            .unwrap();
        assert!(!scope.retire_unreferenced(2).unwrap());
        scope.expect_control(2).unwrap();
        assert!(scope.retire_unreferenced(2).unwrap());
        scope.admit(3).unwrap();
        let mut cancelled = Failure::new(FailureCause::Cancelled);
        cancelled
            .cleanup
            .push(FailureCause::Error(ErrorKind::Conversion).into());
        scope.complete(3, Some(cancelled)).unwrap();
        scope.expect_control(3).unwrap();
        assert!(!scope.retire_unreferenced(3).unwrap());
        scope.observe(3).unwrap();
        assert!(scope.retire_unreferenced(3).unwrap());
        scope.admit(4).unwrap();
        assert!(!scope.retire_unreferenced(4).unwrap());
        scope.complete(4, None).unwrap();
        scope.begin_close(None).unwrap();
        assert_eq!(scope.finish_close().unwrap(), None);
    }

    #[test]
    fn handoff_preserves_observation_and_control_while_the_supervisor_drains() {
        let mut source = ChildScope::new();
        let mut supervisor = ChildScope::new();
        source.admit(1).unwrap();
        source.admit(2).unwrap();
        source
            .complete(
                2,
                Some(Failure::new(FailureCause::Error(ErrorKind::Conversion))),
            )
            .unwrap();
        source.observe(2).unwrap();
        source.admit(3).unwrap();
        source.expect_control(3).unwrap();
        supervisor.begin_close(None).unwrap();
        source.handoff(&mut supervisor).unwrap();
        source.begin_close(None).unwrap();
        assert_eq!(source.finish_close().unwrap(), None);
        assert_eq!(supervisor.pending().unwrap(), vec![1, 3]);
        supervisor.complete(1, None).unwrap();
        supervisor
            .complete(3, Some(Failure::new(FailureCause::Cancelled)))
            .unwrap();
        assert_eq!(supervisor.finish_close().unwrap(), None);
        assert_eq!(
            ChildScope::new().handoff(&mut supervisor),
            Err(ChildScopeError::Closed)
        );
    }

    #[test]
    fn close_joins_dropped_children_and_reports_only_unobserved_immutable_failures() {
        let failed = Failure {
            managed: Default::default(),
            traces: Default::default(),
            primary: Diagnostic {
                cause: FailureCause::Error(ErrorKind::Conversion),
                source: Some(SourcePosition {
                    file: 9,
                    start_byte: 20,
                    end_byte: 30,
                }),
            },
            cleanup: vec![FailureCause::Error(ErrorKind::NativeRecordBusy).into()],
        };
        let mut scope = ChildScope::new();
        scope.admit(0).unwrap();
        scope.admit(1).unwrap();
        scope.admit(2).unwrap();
        scope.complete(0, Some(failed.clone())).unwrap();
        scope.observe(0).unwrap();
        scope.observe(0).unwrap();
        scope.begin_close(None).unwrap();
        assert_eq!(scope.admit(3), Err(ChildScopeError::Closed));
        assert_eq!(scope.pending().unwrap(), vec![1, 2]);
        assert_eq!(scope.finish_close(), Err(ChildScopeError::PendingChildren));
        scope.complete(2, None).unwrap();
        scope.complete(1, Some(failed.clone())).unwrap();
        let result = scope.finish_close().unwrap().unwrap();
        assert_eq!(
            result.primary.cause,
            FailureCause::Error(ErrorKind::UnobservedChildFailure)
        );
        assert_eq!(result.primary.source, failed.primary.source);
        assert_eq!(result.cleanup, [failed.primary, failed.cleanup[0]]);
        assert_eq!(scope.finish_close().unwrap(), Some(result));
        assert!(scope.pending().unwrap().is_empty());
        assert_eq!(failed.cleanup.len(), 1);
    }

    #[test]
    fn parent_failure_keeps_its_primary_and_waits_for_child_cancellation_cleanup() {
        let parent = Failure::new(FailureCause::Cancelled);
        let mut scope = ChildScope::new();
        scope.admit(0).unwrap();
        scope.begin_close(Some(parent.clone())).unwrap();
        scope
            .complete(
                0,
                Some(Failure {
                    managed: Default::default(),
                    traces: Default::default(),
                    primary: FailureCause::Cancelled.into(),
                    cleanup: vec![FailureCause::Error(ErrorKind::Conversion).into()],
                }),
            )
            .unwrap();
        let result = scope.finish_close().unwrap().unwrap();
        assert_eq!(result.primary, parent.primary);
        assert_eq!(
            result.cleanup,
            [FailureCause::Error(ErrorKind::Conversion).into()]
        );
        assert!(!result.recoverable());
    }
}
