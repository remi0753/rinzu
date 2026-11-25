use super::replay::Event;
use super::shared_target::{Receipt, Reg, Sdk};
use super::{Binding, Protocol, ReplayReport, SharedAction as A, SharedTarget, SharedTopology};
use std::collections::{HashMap, HashSet, VecDeque};

fn advance(s: &SharedTarget, a: A) -> Option<SharedTarget> {
    if !s.enabled(a) {
        return None;
    }
    let mut n = s.clone();
    n.apply(a);
    (s.check_step(a, &n).is_ok() && n.check().is_ok()).then_some(n)
}
fn closure(input: HashSet<SharedTarget>, limit: usize) -> Result<HashSet<SharedTarget>, String> {
    let mut queue: VecDeque<_> = input.iter().cloned().collect();
    let mut seen = input;
    while let Some(s) = queue.pop_front() {
        for a in s.actions() {
            let hidden = matches!(
                a,
                A::Cas(_) | A::Sample(_) | A::Exchange(_) | A::Decrement(_)
            ) || matches!(a, A::Copy(g) if unit_acquire(s.binding, s.topology, g));
            if !hidden {
                continue;
            }
            let n = advance(&s, a).ok_or("unsafe hidden shared-target transition")?;
            if seen.insert(n.clone()) {
                if seen.len() > limit {
                    return Err(format!("shared replay state limit {limit}; incomplete"));
                }
                queue.push_back(n);
            }
        }
    }
    Ok(seen)
}

fn unit_acquire(binding: Binding, topology: SharedTopology, generation: usize) -> bool {
    binding == Binding::RustChannel
        && topology
            .coalesced_leader()
            .is_some_and(|leader| generation == leader + 1)
}
fn token(id: u64, count: usize) -> Option<usize> {
    usize::try_from(id.checked_sub(100)?)
        .ok()
        .filter(|g| *g < count)
}
fn observe(
    event: &Event<'_>,
    state: &SharedTarget,
    generation: usize,
    payload: &[i64],
) -> Option<SharedTarget> {
    let unit = &state.units[generation];
    let action = match event.kind {
        "admit" => Some(A::Admit(generation)),
        "register_begin" => Some(A::Begin(generation)),
        "register_retain" => Some(A::Retain(generation)),
        "register_cas" if state.snapshot(generation) == Some((event.arg == 1, event.extra)) => {
            Some(A::CasObserved(generation))
        }
        "register_drop" => Some(A::Rollback(generation)),
        "registered"
            if unit.sample.map(i64::from) == Some(event.extra)
                && (event.arg == 1) == (unit.reg == Reg::Won) =>
        {
            Some(A::Return(generation))
        }
        "publish_begin" => Some(A::PublishBegin(generation)),
        "sdk_prefix" => Some(A::Prefix(generation)),
        "sdk_exchange" if unit.published() && state.captured(generation) == (event.arg == 1) => {
            Some(A::ExchangeObserved(generation))
        }
        "publish_end" => Some(A::PublishEnd(generation)),
        "copied"
            if !unit_acquire(state.binding, state.topology, generation)
                && event.arg == payload[generation] =>
        {
            Some(A::Copy(generation))
        }
        "physical" => Some(A::Physical(generation)),
        "owner_settle" if (event.arg == 1) == unit.cancelled => Some(A::Settle(generation)),
        "record_release" => Some(A::ReleaseRecord(generation)),
        "close_admission" => Some(A::Close(generation)),
        "carrier_free" => Some(A::Retire(generation)),
        "reuse" => Some(A::Reuse),
        "worker_wake_begin" => Some(A::WakeBegin(generation)),
        "worker_wake_end" => Some(A::WakeEnd(generation)),
        "notify_enter"
            if (event.arg == 1) == (state.attached && state.binding != Binding::RustChannel) =>
        {
            Some(A::NotifyBegin(generation))
        }
        "notify_ready" => Some(A::NotifyReady(generation)),
        "notify_leave" => Some(A::NotifyEnd(generation)),
        "merge_wake"
            if state.topology.coalesced_leader().is_some_and(|leader| {
                generation == leader + 1
                    && usize::try_from(event.arg).ok() == Some(100 + leader)
                    && event.extra == 0
            }) =>
        {
            Some(A::MergeWake)
        }
        "producer_release" => Some(A::ReleaseBegin(generation)),
        "producer_released" => Some(A::ReleaseEnd(generation)),
        "control_begin" if event.arg == 1 => Some(A::ControlBegin(generation)),
        "control_end" if (event.arg == 1) == state.accepted_control(generation) => {
            Some(A::ControlEnd(generation))
        }
        "alias_result" if event.arg == payload[0] => Some(A::AliasRead),
        "shutdown" => Some(A::Shutdown),
        "worker_quiescent" => Some(A::Quiescent),
        "joined" if event.arg == 0 => Some(A::Join),
        "detach" => Some(A::Detach),
        "runtime_reference_release" => Some(A::RuntimeRelease),
        "env_free" => Some(A::EnvironmentFree),
        "target_free" => Some(A::TargetFree),
        _ => None,
    };
    if let Some(action) = action {
        return advance(state, action);
    }
    let stutter = match event.kind {
        "completion_claim" => unit.receipt == Receipt::Copied && (event.arg == 1) != unit.cancelled,
        "generation" => unit.active,
        "generation_status" => {
            (event.arg == 1) == unit.cancelled && (event.extra == 1) == unit.published()
        }
        "context_touch" => {
            state.environment
                && if event.id == 0 {
                    state.worker.is_some()
                } else {
                    unit.context && unit.sdk == Sdk::Ready
                }
        }
        "detach_wait" => {
            state.shutdown
                && state.environment
                && state.units.iter().all(|v| v.receipt >= Receipt::Physical)
        }
        _ => false,
    };
    stutter.then(|| state.clone())
}

/// Replay generation-attributed production events against the bounded shared-target model.
/// Each generation has one receipt. The coalesced profile admits two receipts
/// before either publication and allows either settlement order. In the other
/// profiles each admission follows physical retirement of the previous carrier.
/// # Errors
/// Rejects unknown, misattributed, incomplete, unsafe or resource-limited traces.
#[allow(
    clippy::too_many_lines,
    reason = "One ordered event loop preserves identity validation before protocol advancement"
)]
pub fn replay_shared(
    binding: Binding,
    topology: SharedTopology,
    payload: &[i64],
    lines: &[String],
    limit: usize,
) -> Result<ReplayReport, String> {
    let count = topology.count();
    if payload.len() != count || limit == 0 {
        return Err("invalid shared replay bound".into());
    }
    let mut frontier = HashSet::from([SharedTarget::new(binding, topology)]);
    // C creates/adopts a Future before its carrier pointer is assigned. This
    // look-ahead supplies identity only; the binding boundary is still required
    // before registration, and never moves an action earlier in the trace.
    let mut known_ids = HashMap::new();
    for line in lines {
        let f: Vec<_> = line.split(',').collect();
        if f.first() == Some(&"receipt") {
            if f.len() != 4 {
                return Err("invalid receipt binding".into());
            }
            let id = f[1].parse::<u64>().map_err(|_| "invalid receipt id")?;
            let g = token(f[2].parse().map_err(|_| "invalid generation")?, count)
                .ok_or("invalid generation")?;
            if known_ids.insert(id, g).is_some() {
                return Err("duplicate receipt binding".into());
            }
        }
    }
    let mut ids = HashMap::new();
    let mut admitted = HashSet::new();
    let mut bound = HashSet::new();
    let mut declared = HashSet::new();
    let mut seen = HashSet::new();
    let mut captured = HashSet::new();
    let mut report = ReplayReport {
        events: lines.len(),
        maximum_frontier: 1,
    };
    for (offset, line) in lines.iter().enumerate() {
        let f: Vec<_> = line.split(',').collect();
        if f.len() != 4 {
            return Err(format!("invalid shared event {offset}"));
        }
        let e = Event {
            kind: f[0],
            id: f[1].parse().map_err(|_| "invalid object id")?,
            arg: f[2].parse().map_err(|_| "invalid argument")?,
            extra: f[3].parse().map_err(|_| "invalid argument")?,
        };
        let gen_event = matches!(
            e.kind,
            "generation"
                | "reuse"
                | "publish_begin"
                | "publish_end"
                | "sdk_prefix"
                | "sdk_exchange"
                | "notify_enter"
                | "notify_ready"
                | "notify_leave"
                | "merge_wake"
                | "producer_release"
                | "producer_released"
                | "close_admission"
                | "carrier_free"
                | "generation_status"
        ) || (e.kind == "context_touch" && e.id != 0);
        let mut g = if gen_event {
            token(e.id, count)
                .ok_or_else(|| format!("event {offset}: unknown generation {}", e.id))?
        } else {
            0
        };
        let custom = matches!(
            e.kind,
            "generation"
                | "reuse"
                | "receipt"
                | "alias_result"
                | "generation_status"
                | "merge_wake"
        );
        if !custom {
            let normalized = format!(
                "{},{},{},{}",
                e.kind,
                if gen_event { 0 } else { e.id },
                e.arg,
                e.extra
            );
            Event::parse(&normalized)?;
        }
        if e.kind == "generation" || e.kind == "reuse" {
            let replacement = topology.replacement() == Some(g);
            if (e.kind == "reuse") != replacement || !declared.insert(g) {
                return Err("duplicate/incorrect generation declaration".into());
            }
            let expected = if replacement {
                100
            } else {
                i64::try_from(topology.storage(g)).map_err(|_| "invalid storage index")?
            };
            if e.arg != expected || e.extra != i64::from(replacement) {
                return Err("invalid storage generation relation".into());
            }
        } else if gen_event && !declared.contains(&g) {
            return Err("event for undeclared generation".into());
        }
        if e.kind == "receipt" {
            g = token(
                u64::try_from(e.arg).map_err(|_| "invalid generation")?,
                count,
            )
            .ok_or("invalid generation")?;
            if e.extra != 0
                || !declared.contains(&g)
                || ids.insert(e.id, g).is_some()
                || !bound.insert(g)
            {
                return Err("duplicate/invalid receipt binding".into());
            }
            continue;
        } else if e.kind == "admit" {
            if !admitted.insert(e.id) || admitted.len() > count {
                return Err("duplicate/excess receipt".into());
            }
            let &unit = known_ids
                .get(&e.id)
                .ok_or("admission without carrier binding")?;
            g = unit;
            if !declared.contains(&g) {
                return Err("admission for undeclared generation".into());
            }
        } else if e.indexed() || e.kind == "alias_result" {
            g = *ids
                .get(&e.id)
                .filter(|_| admitted.contains(&e.id))
                .ok_or("event for unknown/unadmitted receipt")?;
        }
        if e.kind == "alias_result" && (g != 0 || e.extra != 0 || topology.replacement().is_none())
        {
            return Err("invalid old alias result".into());
        }
        if e.kind == "generation_status"
            && (!(0..=1).contains(&e.arg) || !(0..=1).contains(&e.extra))
        {
            return Err("invalid generation status".into());
        }
        if !matches!(e.kind, "context_touch" | "detach_wait")
            && !seen.insert((e.kind.to_owned(), e.id))
        {
            return Err(format!("duplicate shared event {}", e.kind));
        }
        frontier = closure(frontier, limit)?;
        report.maximum_frontier = report.maximum_frontier.max(frontier.len());
        frontier = frontier
            .iter()
            .filter_map(|s| observe(&e, s, g, payload))
            .collect();
        if frontier.is_empty() {
            return Err(format!(
                "event {offset} does not refine shared target: {line}"
            ));
        }
        if e.kind == "sdk_exchange" && e.arg == 1 {
            captured.insert(g);
        }
        report.maximum_frontier = report.maximum_frontier.max(frontier.len());
    }
    if declared.len() != count || ids.len() != count || admitted.len() != count {
        return Err("incomplete generation/receipt bindings".into());
    }
    for (&id, &g) in &ids {
        for kind in [
            "admit",
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
            if kind == "copied" && unit_acquire(binding, topology, g) {
                continue;
            }
            if !seen.contains(&(kind.to_owned(), id)) {
                return Err(format!("missing {kind}/{id}"));
            }
        }
        for kind in [
            "publish_begin",
            "sdk_prefix",
            "sdk_exchange",
            "publish_end",
            "close_admission",
            "carrier_free",
            "generation_status",
        ] {
            if !seen.contains(&(
                kind.to_owned(),
                100 + u64::try_from(g).map_err(|_| "invalid generation index")?,
            )) {
                return Err(format!("missing {kind}/generation {g}"));
            }
        }
        if captured.contains(&g) {
            let boundaries = if topology
                .coalesced_leader()
                .is_some_and(|leader| g == leader + 1)
            {
                &["merge_wake", "producer_release", "producer_released"][..]
            } else {
                &[
                    "notify_enter",
                    "notify_leave",
                    "producer_release",
                    "producer_released",
                ][..]
            };
            for &kind in boundaries {
                if !seen.contains(&(
                    kind.to_owned(),
                    100 + u64::try_from(g).map_err(|_| "invalid generation index")?,
                )) {
                    return Err(format!("missing {kind}/generation {g}"));
                }
            }
        }
    }
    if !frontier.iter().any(Protocol::terminal) {
        return Err("shared trace ended before complete retirement".into());
    }
    Ok(report)
}
