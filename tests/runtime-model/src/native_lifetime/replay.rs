use super::{Binding, Composed, ComposedAction as A, Decision, Protocol, ReceiptStage};
use std::collections::{HashSet, VecDeque};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayReport {
    pub events: usize,
    pub maximum_frontier: usize,
}

pub(super) struct Event<'a> {
    pub kind: &'a str,
    pub id: u64,
    pub arg: i64,
    pub extra: i64,
}
impl<'a> Event<'a> {
    pub(super) fn parse(line: &'a str) -> Result<Self, String> {
        let f: Vec<_> = line.split(',').collect();
        if f.len() != 4 {
            return Err(format!("invalid trace event {line}"));
        }
        let e = Self {
            kind: f[0],
            id: f[1].parse().map_err(|_| "invalid object id")?,
            arg: f[2].parse().map_err(|_| "invalid argument")?,
            extra: f[3].parse().map_err(|_| "invalid argument")?,
        };
        let args_valid = match e.kind {
            "register_cas" => {
                (e.arg == 1 && e.extra == 0) || (e.arg == 0 && (1..=2).contains(&e.extra))
            }
            "sdk_exchange" => (0..=1).contains(&e.arg) && e.extra == 0,
            "registered" => (0..=1).contains(&e.arg) && e.extra >= 1,
            "copied" => e.extra == 0,
            "control_begin" => (1..=2).contains(&e.arg) && e.extra == 0,
            "completion_claim" | "control_end" | "owner_settle" | "notify_enter" | "joined" => {
                (0..=1).contains(&e.arg) && e.extra == 0
            }
            "admit"
            | "register_begin"
            | "register_retain"
            | "register_drop"
            | "sdk_prefix"
            | "notify_ready"
            | "physical"
            | "record_release"
            | "worker_wake_begin"
            | "worker_wake_end"
            | "close_admission"
            | "carrier_free"
            | "shutdown"
            | "worker_quiescent"
            | "notify_leave"
            | "producer_release"
            | "producer_released"
            | "detach"
            | "runtime_reference_release"
            | "env_free"
            | "target_free"
            | "publish_begin"
            | "publish_end"
            | "context_touch"
            | "detach_wait" => e.arg == 0 && e.extra == 0,
            _ => return Err(format!("unknown trace event {}", e.kind)),
        };
        if !args_valid || (!e.indexed() && e.id != 0) {
            return Err(format!("invalid trace event schema {line}"));
        }
        Ok(e)
    }
    pub(super) fn indexed(&self) -> bool {
        matches!(
            self.kind,
            "admit"
                | "register_begin"
                | "register_retain"
                | "register_cas"
                | "register_drop"
                | "registered"
                | "control_begin"
                | "control_end"
                | "copied"
                | "completion_claim"
                | "physical"
                | "owner_settle"
                | "record_release"
                | "worker_wake_begin"
                | "worker_wake_end"
        )
    }
    pub(super) fn one_shot(&self) -> bool {
        !matches!(
            self.kind,
            "admit"
                | "control_begin"
                | "control_end"
                | "publish_begin"
                | "publish_end"
                | "context_touch"
                | "detach_wait"
        )
    }
    pub(super) fn action(&self, s: &Composed, i: usize, payload: i64) -> Option<A> {
        match self.kind {
            "admit" => Some(A::Admit(i)),
            "copied" if self.arg == payload => Some(A::Copy(i)),
            "owner_settle"
                if (self.arg == 1) == (s.completion.decision(i) != Decision::Completed) =>
            {
                Some(A::Settle(i))
            }
            "record_release" => Some(A::Release(i)),
            "control_begin" => Some(A::BeginControl(
                i,
                if self.arg == 1 {
                    Decision::Cancelled
                } else {
                    Decision::TimedOut
                },
            )),
            "control_end" if s.control_result(i) == Some(self.arg == 1) => {
                Some(A::EndControl(i, self.arg == 1))
            }
            "close_admission" => Some(A::CloseAdmission),
            "carrier_free" => Some(A::RetireCarrier),
            "shutdown" => Some(A::Shutdown),
            "worker_wake_begin" => Some(A::WorkerWakeBegin(i)),
            "worker_wake_end" => Some(A::WorkerWakeEnd(i)),
            "worker_quiescent" => Some(A::Quiescent),
            "joined" if self.arg == 0 => Some(A::Join),
            "notify_enter"
                if (self.arg == 1) == (s.wake.attached && s.binding != Binding::RustChannel) =>
            {
                Some(A::NotifyBegin)
            }
            "notify_leave" => Some(A::NotifyEnd),
            "producer_release" => Some(A::ProducerRelease),
            "detach" => Some(A::Detach),
            "runtime_reference_release" => Some(A::RuntimeReferenceRelease),
            "env_free" => Some(A::EnvironmentFree),
            "target_free" => Some(A::TargetFree),
            _ => None,
        }
    }
    pub(super) fn stutters(&self, s: &Composed, i: usize) -> bool {
        match self.kind {
            "publish_begin" => true,
            "publish_end" => s.completion.published(),
            "producer_released" => !s.wake.producer_reference && s.wake.notification == 2,
            "physical" => s.completion.stage(i) >= ReceiptStage::Acquired,
            "completion_claim" => s.claim_result(i) == Some(self.arg == 1),
            "registered" => {
                i > 0
                    && self.arg == 0
                    && s.completion.stage(i) == ReceiptStage::Waiting
                    && s.wake.attached
            }
            "context_touch" => {
                s.environment && (s.wake.active_context || s.worker_active.is_some())
            }
            "detach_wait" => {
                s.binding != Binding::RustChannel
                    && s.shutdown
                    && s.wake.physical_done
                    && s.environment
            }
            _ => false,
        }
    }
    fn consume(
        &self,
        frontier: &HashSet<Composed>,
        ids: &[u64],
        payload: i64,
    ) -> Result<HashSet<Composed>, String> {
        let receipt = ids.iter().position(|n| *n == self.id);
        if self.indexed() && receipt.is_none() {
            return Err(format!("unknown receipt {}", self.id));
        }
        let i = receipt.unwrap_or(0);
        let mut next = HashSet::new();
        for s in frontier {
            if let Some(a) = self.action(s, i, payload) {
                if let Some(n) = advance(s, a) {
                    next.insert(n);
                }
            } else if self.stutters(s, i) {
                next.insert(s.clone());
            }
        }
        if next.is_empty() {
            return Err("observation does not refine the product".into());
        }
        Ok(next)
    }
}
fn closure(
    input: HashSet<Composed>,
    publishing: bool,
    claims_seen: &[bool],
    limit: usize,
) -> Result<HashSet<Composed>, String> {
    let mut queue: VecDeque<_> = input.iter().cloned().collect();
    let mut seen = input;
    while let Some(s) = queue.pop_front() {
        for a in s.actions() {
            let hidden = matches!(
                a,
                A::Write | A::BeginCopy(_) | A::Claim(_) | A::ResolveControl(_)
            ) || (publishing && matches!(a, A::Prefix | A::Publish));
            let hidden = hidden || matches!(a,A::Physical(i) if claims_seen.get(i)==Some(&true));
            if !hidden {
                continue;
            }
            let mut next = s.clone();
            next.apply(a);
            s.check_step(a, &next)
                .map_err(|e| format!("{}: {}", e.invariant, e.detail))?;
            next.check()
                .map_err(|e| format!("{}: {}", e.invariant, e.detail))?;
            if seen.insert(next.clone()) {
                if seen.len() > limit {
                    return Err(format!("replay state limit {limit}; incomplete"));
                }
                queue.push_back(next);
            }
        }
    }
    Ok(seen)
}
fn advance(s: &Composed, a: A) -> Option<Composed> {
    if !s.enabled(a) {
        return None;
    }
    let mut next = s.clone();
    next.apply(a);
    if s.check_step(a, &next).is_err() || next.check().is_err() {
        return None;
    }
    Some(next)
}
fn first_registration(lines: &[String]) -> Result<u64, String> {
    if lines.len() < 2 {
        return Err("truncated registration prefix".into());
    }
    let first = Event::parse(&lines[0])?;
    let registered = Event::parse(&lines[1])?;
    if first.kind != "admit"
        || registered.kind != "registered"
        || registered.id != first.id
        || registered.arg != 1
        || registered.extra < 2
    {
        return Err("trace must start with a successful pinned notification registration".into());
    }
    Ok(first.id)
}

/// Check a complete observed trace against the synchronized product. Internal
/// steps are allowed only inside their observed operation intervals. Unknown,
/// duplicated, truncated, and resource-limited traces are rejected.
///
/// # Errors
/// Returns the first unmatched observation or an incomplete replay.
pub fn replay_registered(
    binding: Binding,
    count: usize,
    payload: i64,
    lines: &[String],
    limit: usize,
) -> Result<ReplayReport, String> {
    if !(1..=2).contains(&count) || limit == 0 {
        return Err("invalid replay bound".into());
    }
    let first = first_registration(lines)?;
    let mut ids = vec![first];
    let mut publishing = false;
    let mut publish_started = false;
    let mut publish_ended = false;
    let mut frontier = HashSet::from([Composed::new(count, binding)]);
    let mut report = ReplayReport {
        events: lines.len(),
        maximum_frontier: 1,
    };
    let mut seen = HashSet::from([("registered".to_owned(), first)]);
    for (offset, line) in lines.iter().enumerate().skip(2) {
        let e = Event::parse(line).map_err(|error| format!("event {offset}: {error}"))?;
        if e.kind == "admit" {
            if ids.contains(&e.id) || ids.len() >= count {
                return Err(format!("event {offset}: duplicate/excess receipt"));
            }
            ids.push(e.id);
        }
        if e.kind == "publish_begin" {
            if publish_started {
                return Err("duplicate SDK publication".into());
            }
            publish_started = true;
            publishing = true;
        }
        frontier = closure(
            frontier,
            publishing,
            &ids.iter()
                .map(|id| seen.contains(&("completion_claim".to_owned(), *id)))
                .collect::<Vec<_>>(),
            limit,
        )?;
        report.maximum_frontier = report.maximum_frontier.max(frontier.len());
        if e.one_shot() && !seen.insert((e.kind.to_owned(), e.id)) {
            return Err(format!("event {offset}: duplicated {}", e.kind));
        }
        frontier = e
            .consume(&frontier, &ids, payload)
            .map_err(|error| format!("event {offset}: {error}: {line}"))?;
        report.maximum_frontier = report.maximum_frontier.max(frontier.len());
        if e.kind == "publish_end" {
            if !publishing {
                return Err("unmatched publication return".into());
            }
            publishing = false;
            publish_ended = true;
        }
    }
    if !publish_ended || publishing || ids.len() != count {
        return Err("truncated publication/admission trace".into());
    }
    for id in &ids {
        for kind in [
            "registered",
            "copied",
            "completion_claim",
            "physical",
            "owner_settle",
            "record_release",
        ] {
            if !seen.contains(&(kind.into(), *id)) {
                return Err(format!("missing {kind} for {id}"));
            }
        }
    }
    if !frontier.iter().any(Protocol::terminal) {
        return Err("trace ended before physical/context/target retirement".into());
    }
    Ok(report)
}
