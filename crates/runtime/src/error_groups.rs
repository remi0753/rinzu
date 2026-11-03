//! Lexical normal-Error collection. It owns no tasks and never joins a Future.
use crate::failure::Failure;
use std::collections::BTreeSet;

#[derive(Debug, Default)]
pub struct ErrorGroups {
    stack: Vec<Group>,
}

#[derive(Debug)]
struct Group {
    scope: u64,
    failures: Vec<Failure>,
    observed: BTreeSet<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorGroupError {
    UnbalancedScope,
    NotNormalError,
}

impl ErrorGroups {
    pub fn enter(&mut self, scope: u64) {
        self.stack.push(Group {
            scope,
            failures: Vec::new(),
            observed: BTreeSet::new(),
        });
    }

    /// A repeated await of the same Future records its entire failure only once
    /// per group. Explicit throws have no origin and remain separate events.
    pub fn record(
        &mut self,
        scope: u64,
        origin: Option<u64>,
        failure: Failure,
    ) -> Result<bool, ErrorGroupError> {
        let group = self
            .stack
            .last_mut()
            .filter(|group| group.scope == scope)
            .ok_or(ErrorGroupError::UnbalancedScope)?;
        if !failure.recoverable() {
            return Err(ErrorGroupError::NotNormalError);
        }
        if let Some(origin) = origin
            && !group.observed.insert(origin)
        {
            return Ok(false);
        }
        group.failures.push(failure);
        Ok(true)
    }

    /// Removing a lexical group transfers its Error leases to the caller. It
    /// does not mark outstanding tasks observed or change their lifetime.
    pub fn finish(&mut self, scope: u64) -> Result<Vec<Failure>, ErrorGroupError> {
        if !self.stack.last().is_some_and(|group| group.scope == scope) {
            return Err(ErrorGroupError::UnbalancedScope);
        }
        Ok(self.stack.pop().unwrap().failures)
    }

    pub fn depth(&self) -> usize {
        self.stack.len()
    }
}

/// Keep each statement's failure intact until dispatch; flatten only when a
/// single outward failure is needed. Nested cleanup order and leases survive.
pub fn aggregate(failures: impl IntoIterator<Item = Failure>) -> Option<Failure> {
    let mut failures = failures.into_iter();
    let mut result = failures.next()?;
    for failure in failures {
        result.append_cleanup(failure);
    }
    Some(result)
}

/// Prepare dispatch while a finally region suspends its incoming outcome. A
/// control failure bypasses catch, retaining collected Errors as diagnostics.
pub fn settle(
    mut failures: Vec<Failure>,
    incoming: Option<Failure>,
    handles: bool,
) -> (Vec<Failure>, Option<Failure>) {
    if let Some(mut incoming) = incoming {
        if !incoming.recoverable() {
            if let Some(failure) = aggregate(failures) {
                incoming.append_cleanup(failure);
            }
            return (Vec::new(), Some(incoming));
        }
        failures.push(incoming);
    }
    if handles {
        (failures, None)
    } else {
        (Vec::new(), aggregate(failures))
    }
}
