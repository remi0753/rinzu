use super::replay::Event;
use super::{
    Binding, ComposedAction as C, Protocol, Registration, RegistrationAction as A, ReplayReport,
};
use std::collections::{HashSet, VecDeque};

fn checked(state: &Registration, action: A) -> Option<Registration> {
    if !state.enabled(action) {
        return None;
    }
    let mut next = state.clone();
    next.apply(action);
    if state.check_step(action, &next).is_err() || next.check().is_err() {
        return None;
    }
    Some(next)
}
fn closure(
    input: HashSet<Registration>,
    publishing: bool,
    prefix_seen: bool,
    claims_seen: &[bool],
    limit: usize,
) -> Result<HashSet<Registration>, String> {
    let mut queue: VecDeque<_> = input.iter().cloned().collect();
    let mut seen = input;
    while let Some(state) = queue.pop_front() {
        for action in state.actions() {
            let hidden = matches!(
                action,
                A::CompareExchange(_)
                    | A::Product(C::Write | C::BeginCopy(_) | C::Claim(_) | C::ResolveControl(_))
            ) || (publishing && action == A::Product(C::Prefix))
                || (publishing && prefix_seen && action == A::Product(C::Publish));
            let hidden = hidden
                || matches!(action,A::Product(C::Physical(i)) if claims_seen.get(i)==Some(&true));
            if !hidden {
                continue;
            }
            let next = checked(&state, action)
                .ok_or("hidden transition violated the registration product")?;
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
pub(super) fn observe(
    event: &Event<'_>,
    state: &Registration,
    i: usize,
    payload: i64,
) -> Option<Registration> {
    let action = match event.kind {
        "register_begin" => Some(A::Begin(i)),
        "register_retain" => Some(A::Retain(i)),
        "register_drop" => Some(A::Rollback(i)),
        "registered" if state.return_matches(i, event.arg == 1, event.extra) => Some(A::Return(i)),
        "notify_ready" => Some(A::NotifyReady),
        "register_cas" | "registered" | "sdk_prefix" | "sdk_exchange" | "context_touch" => None,
        _ => event.action(&state.inner, i, payload).map(A::Product),
    };
    if let Some(action) = action {
        return checked(state, action);
    }
    let stutter = match event.kind {
        "register_cas" => state.snapshot(i) == Some((event.arg == 1, event.extra)),
        "registered" => false,
        "sdk_prefix" => state.inner.completion.prefix(),
        "sdk_exchange" => state.inner.completion.published() && state.captured == (event.arg == 1),
        "context_touch" => {
            state.inner.environment
                && ((state.inner.wake.active_context && state.notify_ready)
                    || state.inner.worker_active.is_some())
        }
        _ => event.stutters(&state.inner, i),
    };
    stutter.then(|| state.clone())
}

/// Replay a complete production trace from an initially unregistered carrier.
/// Registration CAS and SDK exchange are internal linearization steps; their
/// observed results remain mandatory and must match the same execution.
/// # Errors
/// Rejects malformed, incomplete, unmatched, or resource-limited traces.
pub fn replay(
    binding: Binding,
    count: usize,
    payload: i64,
    lines: &[String],
    limit: usize,
) -> Result<ReplayReport, String> {
    if !(1..=2).contains(&count) || limit == 0 {
        return Err("invalid replay bound".into());
    }
    let mut ids = Vec::new();
    let mut frontier = HashSet::from([Registration::new(count, binding)]);
    let mut seen = HashSet::new();
    let mut publishing = false;
    let mut started = false;
    let mut ended = false;
    let mut report = ReplayReport {
        events: lines.len(),
        maximum_frontier: 1,
    };
    for (offset, line) in lines.iter().enumerate() {
        let event = Event::parse(line).map_err(|e| format!("event {offset}: {e}"))?;
        if event.kind == "admit" {
            if ids.contains(&event.id) || ids.len() >= count {
                return Err(format!("event {offset}: duplicate/excess receipt"));
            }
            ids.push(event.id);
        }
        let receipt = ids.iter().position(|id| *id == event.id);
        if event.indexed() && receipt.is_none() {
            return Err(format!("event {offset}: unknown receipt {}", event.id));
        }
        if event.kind == "publish_begin" {
            if started {
                return Err("duplicate SDK publication".into());
            }
            publishing = true;
            started = true;
        }
        if matches!(event.kind, "sdk_prefix" | "sdk_exchange") && !publishing {
            return Err("SDK observation outside publication invocation".into());
        }
        if event.kind == "sdk_exchange" && !seen.contains(&("sdk_prefix".to_owned(), 0)) {
            return Err("SDK exchange observation preceded prefix".into());
        }
        if event.kind == "publish_end" && !seen.contains(&("sdk_exchange".to_owned(), 0)) {
            return Err("publication returned before exchange observation".into());
        }
        frontier = closure(
            frontier,
            publishing,
            seen.contains(&("sdk_prefix".to_owned(), 0)),
            &ids.iter()
                .map(|id| seen.contains(&("completion_claim".to_owned(), *id)))
                .collect::<Vec<_>>(),
            limit,
        )?;
        report.maximum_frontier = report.maximum_frontier.max(frontier.len());
        if event.one_shot() && !seen.insert((event.kind.to_owned(), event.id)) {
            return Err(format!("event {offset}: duplicate {}", event.kind));
        }
        let next: HashSet<_> = frontier
            .iter()
            .filter_map(|s| observe(&event, s, receipt.unwrap_or(0), payload))
            .collect();
        if next.is_empty() {
            return Err(format!(
                "event {offset} does not refine registration product: {line}"
            ));
        }
        report.maximum_frontier = report.maximum_frontier.max(next.len());
        frontier = next;
        if event.kind == "publish_end" {
            if !publishing {
                return Err("unmatched publication return".into());
            }
            publishing = false;
            ended = true;
        }
    }
    if !ended || publishing || ids.len() != count {
        return Err("truncated publication/admission trace".into());
    }
    require_observations(&ids, &seen)?;
    if !frontier.iter().any(Protocol::terminal) {
        return Err("trace ended before registration/context/target retirement".into());
    }
    Ok(report)
}

fn require_observations(ids: &[u64], seen: &HashSet<(String, u64)>) -> Result<(), String> {
    for id in ids {
        for kind in [
            "register_begin",
            "register_retain",
            "register_cas",
            "registered",
            "copied",
            "completion_claim",
            "physical",
            "owner_settle",
            "record_release",
        ] {
            if !seen.contains(&(kind.to_owned(), *id)) {
                return Err(format!("missing {kind} for {id}"));
            }
        }
    }
    for kind in ["sdk_prefix", "sdk_exchange"].into_iter().chain(
        seen.contains(&("producer_release".to_owned(), 0))
            .then_some("producer_released"),
    ) {
        if !seen.contains(&(kind.to_owned(), 0)) {
            return Err(format!("missing {kind}"));
        }
    }
    Ok(())
}
