use super::joined::Aux;
use super::replay::Event;
use super::{
    ComposedAction as C, Joined, JoinedAction as A, Protocol, RegistrationAction as R, ReplayReport,
};
use std::collections::{HashSet, VecDeque};
fn advance(s: &Joined, a: A) -> Option<Joined> {
    if !s.enabled(a) {
        return None;
    }
    let mut n = s.clone();
    n.apply(a);
    (s.check_step(a, &n).is_ok() && n.check().is_ok()).then_some(n)
}
fn closure(
    input: HashSet<Joined>,
    seen: &HashSet<(String, u64)>,
    ids: &[u64],
    limit: usize,
) -> Result<HashSet<Joined>, String> {
    let mut queue: VecDeque<_> = input.iter().cloned().collect();
    let mut visited = input;
    while let Some(s) = queue.pop_front() {
        for a in s.actions() {
            let hidden = matches!(
                a,
                A::Primary(
                    R::CompareExchange(_)
                        | R::Product(
                            C::Write | C::BeginCopy(_) | C::Claim(_) | C::ResolveControl(_)
                        )
                )
            ) || matches!(a,A::Primary(R::Product(C::Physical(i))) if ids.get(i).is_some_and(|id|seen.contains(&("completion_claim".to_owned(),*id))))
                || (seen.contains(&("publish_begin".to_owned(), 0))
                    && matches!(a, A::Primary(R::Product(C::Prefix))))
                || (seen.contains(&("sdk_prefix".to_owned(), 0))
                    && matches!(a, A::Primary(R::Product(C::Publish))));
            if hidden {
                let n = advance(&s, a).ok_or("unsafe joined hidden transition")?;
                if visited.insert(n.clone()) {
                    if visited.len() > limit {
                        return Err(format!("joined replay state limit {limit}; incomplete"));
                    }
                    queue.push_back(n);
                }
            }
        }
    }
    Ok(visited)
}
fn observe(e: &Event<'_>, s: &Joined, i: usize, payload: i64) -> Option<Joined> {
    let a = match e.kind {
        "aux_admit" => Some(A::AdmitAux),
        "aux_bind" if e.arg == 1 => Some(A::BindAux),
        "aux_publish" => Some(A::PublishAux),
        "aux_copied" if e.arg == 0 => Some(A::CopyAux),
        "aux_claim" if e.arg == 1 => Some(A::ClaimAux),
        "aux_physical" => Some(A::PhysicalAux),
        "aux_settle" if e.arg == 0 => Some(A::SettleAux),
        "aux_release" => Some(A::ReleaseAux),
        "aux_retire" => Some(A::RetireAux),
        "helper_wake_begin" => Some(A::HelperWakeBegin),
        "helper_wake_end" => Some(A::HelperWakeEnd),
        "helper_quiescent" => Some(A::Primary(R::Product(C::Quiescent))),
        "helper_joined" => Some(A::Primary(R::Product(C::Join))),
        "register_begin" => Some(A::Primary(R::Begin(i))),
        "register_retain" => Some(A::Primary(R::Retain(i))),
        "register_drop" => Some(A::Primary(R::Rollback(i))),
        "registered" if s.inner.return_matches(i, e.arg == 1, e.extra) => {
            Some(A::Primary(R::Return(i)))
        }
        "notify_ready" => Some(A::Primary(R::NotifyReady)),
        "register_cas" | "registered" | "sdk_prefix" | "sdk_exchange" => None,
        _ => e
            .action(&s.inner.inner, i, payload)
            .map(|a| A::Primary(R::Product(a))),
    };
    if let Some(a) = a {
        return advance(s, a);
    }
    let stutter = match e.kind {
        "register_cas" => s.inner.snapshot(i) == Some((e.arg == 1, e.extra)),
        "registered" => false,
        "sdk_prefix" => s.inner.inner.completion.prefix(),
        "sdk_exchange" => s.inner.inner.completion.published() && s.inner.captured == (e.arg == 1),
        "helper_join_wait" => {
            s.inner.inner.shutdown && s.physical() && s.auxiliary >= Aux::Physical
        }
        "helper_context" => s.helper == 1 && s.inner.inner.environment,
        "native_context" => {
            s.inner.inner.wake.active_context && s.inner.notify_ready && s.inner.inner.environment
        }
        _ => e.stutters(&s.inner.inner, i),
    };
    stutter.then(|| s.clone())
}
/// Interpret a complete production trace with notified v3 receipts and a real
/// v1 polling receipt that starts the shared C reactor. One scan/wake batch is
/// modeled; every auxiliary lifetime boundary remains mandatory.
/// # Errors
/// Rejects malformed, incomplete, unmatched and resource-limited traces.
#[allow(
    clippy::too_many_lines,
    reason = "Single ordered event loop validates each boundary before advancement"
)]
pub fn replay_joined(
    count: usize,
    payload: i64,
    lines: &[String],
    limit: usize,
) -> Result<ReplayReport, String> {
    if !(1..=2).contains(&count) || limit == 0 {
        return Err("invalid joined replay bound".into());
    }
    let mut frontier = HashSet::from([Joined::new(count)]);
    let mut ids = Vec::new();
    let mut seen = HashSet::new();
    let mut captured = false;
    let mut report = ReplayReport {
        events: lines.len(),
        maximum_frontier: 1,
    };
    for (offset, line) in lines.iter().enumerate() {
        let f: Vec<_> = line.split(',').collect();
        if f.len() != 4 {
            return Err("invalid joined event".into());
        }
        let e = Event {
            kind: f[0],
            id: f[1].parse().map_err(|_| "invalid id")?,
            arg: f[2].parse().map_err(|_| "invalid argument")?,
            extra: f[3].parse().map_err(|_| "invalid argument")?,
        };
        let custom = matches!(
            e.kind,
            "aux_admit"
                | "aux_bind"
                | "aux_publish"
                | "aux_copied"
                | "aux_claim"
                | "aux_physical"
                | "aux_settle"
                | "aux_release"
                | "aux_retire"
                | "helper_wake_begin"
                | "helper_wake_end"
                | "helper_quiescent"
                | "helper_join_wait"
                | "helper_joined"
                | "helper_context"
                | "native_context"
        );
        if custom {
            let expected = i64::from(matches!(e.kind, "aux_bind" | "aux_claim"));
            if e.id != 0 || e.arg != expected || e.extra != 0 {
                return Err("invalid auxiliary/helper schema".into());
            }
        } else {
            Event::parse(line)?;
        }
        if e.kind == "admit" {
            if ids.contains(&e.id) || ids.len() >= count {
                return Err("duplicate/excess notified receipt".into());
            }
            ids.push(e.id);
        }
        let i = ids.iter().position(|id| *id == e.id);
        if e.indexed() && i.is_none() {
            return Err("unknown notified receipt".into());
        }
        if e.kind == "sdk_prefix" && !seen.contains(&("publish_begin".to_owned(), 0)) {
            return Err("SDK prefix outside invocation".into());
        }
        if e.kind == "sdk_exchange" && !seen.contains(&("sdk_prefix".to_owned(), 0)) {
            return Err("exchange before prefix".into());
        }
        if e.kind == "sdk_exchange" {
            captured = e.arg == 1;
        }
        if e.kind == "publish_end" && !seen.contains(&("sdk_exchange".to_owned(), 0)) {
            return Err("SDK return before exchange".into());
        }
        frontier = closure(frontier, &seen, &ids, limit)?;
        report.maximum_frontier = report.maximum_frontier.max(frontier.len());
        if !matches!(e.kind, "helper_context" | "native_context" | "detach_wait")
            && !seen.insert((e.kind.to_owned(), e.id))
        {
            return Err("duplicate joined boundary".into());
        }
        frontier = frontier
            .iter()
            .filter_map(|s| observe(&e, s, i.unwrap_or(0), payload))
            .collect();
        if frontier.is_empty() {
            return Err(format!(
                "event {offset} does not refine joined model: {line}"
            ));
        }
    }
    for id in ids.iter().copied() {
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
            if !seen.contains(&(kind.to_owned(), id)) {
                return Err(format!("missing {kind}/{id}"));
            }
        }
    }
    for kind in [
        "aux_admit",
        "aux_bind",
        "aux_publish",
        "aux_copied",
        "aux_claim",
        "aux_physical",
        "aux_settle",
        "aux_release",
        "aux_retire",
        "helper_wake_begin",
        "helper_wake_end",
        "helper_quiescent",
        "helper_join_wait",
        "helper_joined",
        "publish_begin",
        "sdk_prefix",
        "sdk_exchange",
        "publish_end",
        "close_admission",
        "carrier_free",
        "shutdown",
        "detach",
        "runtime_reference_release",
        "env_free",
        "target_free",
    ] {
        if !seen.contains(&(kind.to_owned(), 0)) {
            return Err(format!("missing {kind}"));
        }
    }
    if captured
        && (!seen.contains(&("producer_release".to_owned(), 0))
            || !seen.contains(&("producer_released".to_owned(), 0)))
    {
        return Err("missing SDK reference return".into());
    }
    if ids.len() != count || !frontier.iter().any(Protocol::terminal) {
        return Err("incomplete joined retirement".into());
    }
    Ok(report)
}
