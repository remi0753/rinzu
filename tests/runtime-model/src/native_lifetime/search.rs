use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fmt::{self, Debug, Display, Formatter};
use std::hash::Hash;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Violation {
    pub invariant: &'static str,
    pub detail: &'static str,
}

pub(super) fn require(
    condition: bool,
    invariant: &'static str,
    detail: &'static str,
) -> Result<(), Violation> {
    if condition {
        Ok(())
    } else {
        Err(Violation { invariant, detail })
    }
}

pub trait Protocol: Clone + Debug + Eq + Hash {
    type Action: Copy + Debug;
    fn name(&self) -> String;
    fn actions(&self) -> Vec<Self::Action>;
    /// Apply an enabled action. Vector adapters must honor the same preconditions.
    fn apply(&mut self, action: Self::Action);
    /// # Errors
    /// Returns a violated state invariant.
    fn check(&self) -> Result<(), Violation>;
    /// # Errors
    /// Returns a violated transition frame condition.
    fn check_step(&self, _action: Self::Action, _after: &Self) -> Result<(), Violation> {
        Ok(())
    }
    fn terminal(&self) -> bool;
    fn allowed_stall(&self) -> bool {
        false
    }
    fn witnesses(&self) -> Vec<&'static str> {
        Vec::new()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Report {
    pub profile: String,
    pub states: usize,
    pub transitions: usize,
    pub terminals: usize,
    pub allowed_stalls: usize,
    pub maximum_shortest_depth: usize,
    pub witnesses: BTreeSet<&'static str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureKind {
    Invariant,
    Deadlock,
    StateLimit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Counterexample {
    pub kind: FailureKind,
    pub profile: String,
    pub reason: String,
    /// BFS parent links give a shortest action trace in this abstraction.
    pub trace: Vec<String>,
    pub state: String,
}
impl Display for Counterexample {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}: {:?}: {}", self.profile, self.kind, self.reason)?;
        for (i, action) in self.trace.iter().enumerate() {
            writeln!(f, "{i:03}: {action}")?;
        }
        write!(f, "{}", self.state)
    }
}
impl std::error::Error for Counterexample {}

struct Parent<A> {
    previous: Option<usize>,
    action: Option<A>,
    depth: usize,
}
fn trace<A: Debug>(parents: &[Parent<A>], mut index: usize) -> Vec<String> {
    let mut result = Vec::new();
    while let Some(parent) = parents[index].previous {
        result.push(format!("{:?}", parents[index].action.as_ref().unwrap()));
        index = parent;
    }
    result.reverse();
    result
}

/// Visit every reachable state and check every enabled transition, including
/// stutters. No depth cutoff or sampled scheduling is used.
///
/// # Errors
/// A resource limit is an explicit incomplete result, not a passed verification.
pub fn explore<S: Protocol>(initial: S, limit: usize) -> Result<Report, Counterexample> {
    let profile = initial.name();
    let failure = |kind, reason, trace, state: &S| Counterexample {
        kind,
        profile: profile.clone(),
        reason,
        trace,
        state: format!("{state:#?}"),
    };
    initial.check().map_err(|v| {
        failure(
            FailureKind::Invariant,
            format!("{}: {}", v.invariant, v.detail),
            Vec::new(),
            &initial,
        )
    })?;
    if limit == 0 {
        return Err(failure(
            FailureKind::StateLimit,
            "state limit 0".into(),
            Vec::new(),
            &initial,
        ));
    }
    let mut visited = HashMap::from([(initial.clone(), 0)]);
    let mut queue = VecDeque::from([(initial, 0)]);
    let mut parents = vec![Parent {
        previous: None,
        action: None,
        depth: 0,
    }];
    let mut report = Report {
        profile: profile.clone(),
        states: 1,
        transitions: 0,
        terminals: 0,
        allowed_stalls: 0,
        maximum_shortest_depth: 0,
        witnesses: BTreeSet::new(),
    };
    while let Some((state, index)) = queue.pop_front() {
        report.maximum_shortest_depth = report.maximum_shortest_depth.max(parents[index].depth);
        report.witnesses.extend(state.witnesses());
        if state.terminal() {
            report.terminals += 1;
            continue;
        }
        let mut progress = false;
        for action in state.actions() {
            let mut next = state.clone();
            next.apply(action);
            report.transitions += 1;
            let checked = state.check_step(action, &next).and_then(|()| next.check());
            if let Err(v) = checked {
                let mut actions = trace(&parents, index);
                actions.push(format!("{action:?}"));
                return Err(failure(
                    FailureKind::Invariant,
                    format!("{}: {}", v.invariant, v.detail),
                    actions,
                    &next,
                ));
            }
            progress |= next != state;
            if !visited.contains_key(&next) {
                if visited.len() >= limit {
                    let mut actions = trace(&parents, index);
                    actions.push(format!("{action:?}"));
                    return Err(failure(
                        FailureKind::StateLimit,
                        format!("state limit {limit}; search incomplete"),
                        actions,
                        &next,
                    ));
                }
                let next_index = parents.len();
                parents.push(Parent {
                    previous: Some(index),
                    action: Some(action),
                    depth: parents[index].depth + 1,
                });
                visited.insert(next.clone(), next_index);
                queue.push_back((next, next_index));
            }
        }
        if !progress {
            if state.allowed_stall() {
                report.allowed_stalls += 1;
            } else {
                return Err(failure(
                    FailureKind::Deadlock,
                    "nonterminal state has no nonstuttering action".into(),
                    trace(&parents, index),
                    &state,
                ));
            }
        }
    }
    report.states = visited.len();
    Ok(report)
}
