use super::admission::Owner as Drain;
use super::call::Action as K;
use super::cohort::Owner;
use super::polling::PollingAction as P;
use super::series;
use super::{
    BatchSchedule, Binding, Cohort, CohortAction as H, CohortLayout, CohortMode,
    ComposedAction as C, Decision, MixedBatchSchedule, Protocol, ReceiptStage as S,
    RegistrationAction as R, ReplayReport,
};
use std::collections::{HashMap, HashSet, VecDeque};
const PAYLOAD: [i64; 4] = [7183, 29, 65537, 0];
fn copied_payload(s: &Cohort, g: usize) -> Option<i64> {
    if s.series.is_some() && series::VERSIONS[g] == 2 {
        (s.binding != Binding::RustChannel).then_some(0)
    } else {
        Some(PAYLOAD[g])
    }
}
fn advance(s: &Cohort, a: H) -> Option<Cohort> {
    if !s.enabled(a) {
        return None;
    }
    let mut n = s.clone();
    n.apply(a);
    (s.check_step(a, &n).is_ok() && n.check().is_ok()).then_some(n)
}
fn closure(
    input: HashSet<Cohort>,
    claims: &HashSet<(usize, usize)>,
    limit: usize,
) -> Result<HashSet<Cohort>, String> {
    let mut queue: VecDeque<_> = input.iter().cloned().collect();
    let mut seen = input;
    while let Some(s) = queue.pop_front() {
        for a in s.actions() {
            let hidden = matches!(
                a,
                H::Advance
                    | H::DropAliases
                    | H::DeferredCommit
                    | H::InspectDeferred
                    | H::Sample(..)
                    | H::Decrement(_)
                    | H::Call(_, K::FinishContinuation | K::SettleCallback(_))
                    | H::Polling(P::Call(K::FinishContinuation | K::SettleCallback(_)))
                    | H::Runtime(
                        _,
                        R::CompareExchange(_)
                            | R::Product(
                                C::Write
                                    | C::Prefix
                                    | C::Publish
                                    | C::BeginCopy(_)
                                    | C::Claim(_)
                                    | C::ResolveControl(_)
                            )
                    )
            ) || matches!(a,H::Runtime(g,R::Product(C::Physical(i))) if claims.contains(&(g,i)));
            let hidden = hidden
                || matches!(a,H::Polling(P::Copy) if s.binding==Binding::RustChannel)
                || matches!(a,H::Runtime(g,R::Product(C::Copy(_))) if copied_payload(&s,g).is_none())
                || matches!(a,H::Polling(P::Physical) if claims.contains(&(s.units.len(),0)));
            if !hidden {
                continue;
            }
            let n = advance(&s, a).ok_or("unsafe hidden cohort transition")?;
            if seen.insert(n.clone()) {
                if seen.len() > limit {
                    return Err("cohort replay state limit; incomplete".into());
                }
                queue.push_back(n);
            }
        }
    }
    Ok(seen)
}
fn token(n: u64, count: usize) -> Option<usize> {
    usize::try_from(n.checked_sub(100)?)
        .ok()
        .filter(|g| *g < count)
}
fn call_event(kind: &str) -> Option<K> {
    Some(match kind {
        "call_enter" => K::Enter,
        "callback_admit" => K::Admit(0),
        "callback_begin" => K::Begin(0),
        "callback_return" => K::ReturnCallback(0),
        "call_return" => K::ReturnNative,
        "call_close" => K::CloseGate,
        "call_settle" => K::SettleGate,
        "call_read" => K::ReadResult,
        "call_env_free" => K::ReleaseEnvironment,
        _ => return None,
    })
}
fn decision_value(d: Decision) -> i64 {
    match d {
        Decision::Completed => 0,
        Decision::Cancelled => 1,
        Decision::TimedOut => 2,
        Decision::Pending => -1,
    }
}
#[allow(
    clippy::too_many_lines,
    clippy::many_single_char_names,
    reason = "One event maps to one guarded product transition"
)]
fn observe(s: &Cohort, kind: &str, g: usize, i: usize, arg: i64, extra: i64) -> Option<Cohort> {
    if g == s.units.len() && s.polling.is_some() {
        let p = s.polling.as_ref()?;
        let a = if let Some(k) = call_event(kind) {
            Some(P::Call(k))
        } else {
            match kind {
                "sdk_start" => Some(P::Start),
                "handoff" => Some(P::Handoff),
                "drain_receipt" => Some(P::Admit),
                "poll_bind" if arg == 1 => Some(P::Bind),
                "poll_publish" => Some(P::Publish),
                "copied" if arg == 0 && s.binding != Binding::RustChannel => Some(P::Copy),
                "completion_claim" if arg == 1 => Some(P::Claim),
                "physical" if !p.physical() => Some(P::Physical),
                "worker_wake_begin" => Some(P::WorkerBegin),
                "worker_wake_end" => Some(P::WorkerEnd),
                "owner_settle" if arg == 0 => Some(P::Settle),
                "drain_release" => Some(P::DrainRelease),
                "record_release" => Some(P::Release),
                "close_admission" => Some(P::Close),
                "carrier_free" => Some(P::Retire),
                _ => None,
            }
        };
        if let Some(a) = a {
            return advance(s, H::Polling(a));
        }
        let stutter = match kind {
            "generation" | "carrier_abi" => p.started(),
            "receipt_decision" if arg == 0 => p.physical(),
            "physical" => p.physical(),
            "admit" => p.bound || p.pc == 13 && p.stage != super::joined::Aux::Absent,
            "sdk_pin_free" => p.retired,
            _ => false,
        };
        return stutter.then(|| s.clone());
    }
    let u = &s.units[g];
    let action = if let Some(k) = call_event(kind) {
        Some(H::Call(g, k))
    } else {
        match kind {
            "sdk_start" => Some(H::Start(g)),
            "handoff" => Some(H::Handoff(g)),
            "drain_deferred" if !u.drains[i].attempted && !s.credit() => Some(H::Admit(g, i)),
            "drain_receipt" if u.drains[i].owner == Drain::Deferred => Some(H::Retry(g, i)),
            "drain_receipt" if s.credit() => Some(H::Admit(g, i)),
            "register_begin" => Some(H::Runtime(g, R::Begin(i))),
            "register_retain" => Some(H::Runtime(g, R::Retain(i))),
            "register_cas" if u.runtime.snapshot(i) == Some((arg == 1, extra)) => {
                Some(H::CasSeen(g, i))
            }
            "register_drop" => Some(H::Runtime(g, R::Rollback(i))),
            "registered"
                if u.runtime.return_value(i) == Some(arg == 1)
                    && u.samples[i].map(i64::from) == Some(extra) =>
            {
                Some(H::Runtime(g, R::Return(i)))
            }
            "publish_begin" => Some(H::PublishBegin(g)),
            "sdk_prefix" if u.runtime.inner.completion.prefix() => Some(H::PrefixSeen(g)),
            "sdk_exchange"
                if u.runtime.inner.completion.published() && u.runtime.captured == (arg == 1) =>
            {
                Some(H::ExchangeSeen(g))
            }
            "merge_wake" if s.batch.is_some() && matches!(g, 1 | 2) => Some(H::MergeWake(3 - g)),
            "merge_wake" if s.series.is_some() => Some(H::MergeWake(g ^ 1)),
            "ready_selection" if g == 3 && i == 0 => Some(H::ReadySelection),
            "publish_end" => Some(H::PublishEnd(g)),
            "copied" if copied_payload(s, g) == Some(arg) => {
                Some(H::Runtime(g, R::Product(C::Copy(i))))
            }
            "owner_settle" if (arg == 1) == (s.decision(g, i) != Decision::Completed) => {
                Some(H::Runtime(g, R::Product(C::Settle(i))))
            }
            "direct_copy" if s.direct_receipt(g, i) && arg == PAYLOAD[g] => {
                Some(H::DirectCopy(g, i))
            }
            "direct_settle"
                if s.direct_receipt(g, i) && arg == decision_value(s.decision(g, i)) =>
            {
                Some(H::DirectSettle(g, i))
            }
            "drain_release" => Some(H::DrainRelease(g, i)),
            "record_release" => Some(H::Runtime(g, R::Product(C::Release(i)))),
            "close_admission" => Some(H::Runtime(g, R::Product(C::CloseAdmission))),
            "carrier_free" => Some(H::Retire(g)),
            "control_begin" if matches!(s.owner(),Owner::Late(j,d) if g==0&&i==j&&decision_value(d)==arg) => {
                Some(H::LateBegin)
            }
            "control_end" if matches!(s.owner(),Owner::Late(j,_) if g==0&&i==j) && arg == 0 => {
                Some(H::LateEnd)
            }
            "control_begin" => Some(H::Runtime(
                g,
                R::Product(C::BeginControl(
                    i,
                    if arg == 1 {
                        Decision::Cancelled
                    } else {
                        Decision::TimedOut
                    },
                )),
            )),
            "control_end" if u.runtime.inner.control_result(i) == Some(arg == 1) => {
                Some(H::Runtime(g, R::Product(C::EndControl(i, arg == 1))))
            }
            "deferred_begin" if matches!(s.owner(),Owner::Control(x,j,d) if x==g&&j==i&&decision_value(d)==arg) => {
                Some(H::DeferredBegin)
            }
            "deferred_flag" => Some(H::DeferredFlag),
            "deferred_end" if arg == 1 => Some(H::DeferredEnd),
            "alias_result" if g == 0 && i == 1 && arg == PAYLOAD[0] => Some(H::Alias),
            "generation_status"
                if g == 2
                    && arg == i64::from(u.sdk_cancelled)
                    && extra == i64::from(u.runtime.inner.completion.published()) =>
            {
                Some(H::Status)
            }
            "worker_wake_begin" => Some(H::Runtime(g, R::Product(C::WorkerWakeBegin(i)))),
            "worker_wake_end" => Some(H::Runtime(g, R::Product(C::WorkerWakeEnd(i)))),
            "notify_enter" if (arg == 1) == (s.attached && s.binding != Binding::RustChannel) => {
                Some(H::Runtime(g, R::Product(C::NotifyBegin)))
            }
            "notify_ready" => Some(H::Runtime(g, R::NotifyReady)),
            "notify_leave" => Some(H::Runtime(g, R::Product(C::NotifyEnd))),
            "producer_release" => Some(H::ReleaseBegin(g)),
            "producer_released" => Some(H::ReleaseEnd(g)),
            "shutdown" => Some(H::Shutdown),
            "worker_quiescent" | "helper_quiescent" => Some(H::Quiescent),
            "helper_joined" => Some(H::Join),
            "helper_wake_begin" => Some(H::HelperWakeBegin),
            "helper_wake_end" => Some(H::HelperWakeEnd),
            "joined" if arg == 0 => Some(H::Join),
            "detach" => Some(H::Detach),
            "runtime_reference_release" => Some(H::RuntimeRelease),
            "env_free" => Some(H::EnvironmentFree),
            "target_free" => Some(H::TargetFree),
            "owner_dispose" => Some(H::Dispose),
            _ => None,
        }
    };
    if let Some(a) = action {
        return advance(s, a);
    }
    let stutter = match kind {
        "generation" | "carrier_abi" => u.started && s.storage[s.layout.storage(g)] == Some(g),
        "batch_target" => {
            (s.batch.is_some() && matches!(g, 1 | 2) || s.series.is_some())
                && u.sdk >= 3
                && u.runtime.captured
        }
        "admit" => s.stage(g, i) == S::Waiting,
        "completion_claim" => u.runtime.inner.claim_result(i) == Some(arg == 1),
        "physical" => s.stage(g, i) >= S::Acquired,
        "receipt_decision" => {
            s.stage(g, i) >= S::Acquired && decision_value(s.decision(g, i)) == arg
        }
        "sdk_pin_free" => !u.pin && (u.runtime.inner.completion.retired() || u.direct_retired),
        "signal_touch" => s.environment,
        "context_touch" => {
            s.environment
                && (u.runtime.inner.worker_active.is_some()
                    || (u.runtime.inner.wake.active_context && u.runtime.notify_ready))
        }
        "helper_join_wait" => s.shutdown && s.all_physical_for_replay(),
        "helper_context" => s.helper_active && s.environment,
        "detach_wait" => s.shutdown && s.environment && s.all_physical_for_replay(),
        _ => false,
    };
    stutter.then(|| s.clone())
}
/// Replay the whole script-guided product, including every receipt's generation
/// and the actual identity and aggregate references of one shared wake target.
/// # Panics
/// Internal bounded identity conversions and complete validated bindings must remain consistent.
/// # Errors
/// Rejects missing, duplicate, misbound, unsafe, truncated or resource-limited traces.
#[allow(
    clippy::too_many_lines,
    reason = "One ordered pass validates identities and ownership before accepting any boundary"
)]
pub fn replay_cohort(
    binding: Binding,
    layout: CohortLayout,
    controlled: bool,
    lines: &[String],
    limit: usize,
) -> Result<ReplayReport, String> {
    if binding == Binding::CJoined {
        return Err("invalid cohort binding".into());
    }
    replay_product(Cohort::new(binding, layout, controlled), lines, limit)
}
/// Replay the extended whole product with early publication, direct deferred
/// collection, or a real mixed v1/v3 joined helper. No component fallback.
/// # Panics
/// Panics for unsupported binding/mode combinations.
/// # Errors
/// Rejects unsafe, incomplete, misbound, or resource-limited traces.
pub fn replay_combination(
    binding: Binding,
    mode: CohortMode,
    controlled: bool,
    lines: &[String],
    limit: usize,
) -> Result<ReplayReport, String> {
    replay_product(Cohort::combination(binding, mode, controlled), lines, limit)
}
/// Replay three notified generations and optional v1 polling in one product.
/// # Panics
/// Panics for unsupported profile parameters.
/// # Errors
/// Rejects unsafe, incomplete, or resource-limited traces.
pub fn replay_extended(
    binding: Binding,
    mode: CohortMode,
    controlled: bool,
    polling: bool,
    lines: &[String],
    limit: usize,
) -> Result<ReplayReport, String> {
    replay_product(
        Cohort::extended(binding, mode, controlled, polling),
        lines,
        limit,
    )
}
/// Replay the two notified Calls and one polling Call without supplying a recovery order.
/// # Panics
/// Rejects bindings or controls outside this profile.
/// # Errors
/// Rejects unsafe, incomplete, or resource-limited traces.
pub fn replay_unordered(
    binding: Binding,
    controlled: bool,
    lines: &[String],
    limit: usize,
) -> Result<ReplayReport, String> {
    replay_product(Cohort::unordered(binding, controlled), lines, limit)
}
/// Check one full Call, Admission, Completion and Wake trace across a reused carrier
/// and a coalesced SDK notification.
/// # Panics
/// Rejects bindings outside the batched profile.
/// # Errors
/// Rejects unsafe, incomplete or resource-limited traces.
pub fn replay_batched(
    binding: Binding,
    schedule: BatchSchedule,
    controlled: bool,
    lines: &[String],
    limit: usize,
) -> Result<ReplayReport, String> {
    replay_product(Cohort::batched(binding, schedule, controlled), lines, limit)
}
/// Check the complete mixed-ABI trace, including the actual identity of each
/// carrier and two notification batches on one target.
/// # Panics
/// Rejects bindings outside the mixed-batch profile.
/// # Errors
/// Rejects unsafe, incomplete or resource-limited traces.
pub fn replay_mixed_batches(
    binding: Binding,
    schedule: MixedBatchSchedule,
    controlled: bool,
    lines: &[String],
    limit: usize,
) -> Result<ReplayReport, String> {
    replay_product(
        Cohort::mixed_batches(binding, schedule, controlled),
        lines,
        limit,
    )
}
#[allow(
    clippy::too_many_lines,
    reason = "One ordered pass validates all product boundaries"
)]
fn replay_product(initial: Cohort, lines: &[String], limit: usize) -> Result<ReplayReport, String> {
    let layout = initial.layout;
    let mode = initial.mode;
    let polling = initial.polling.is_some();
    let poll_index = layout.count();
    let binding = initial.binding;
    let extended = initial.extended_profile();
    let unordered = initial.recovery.is_some();
    let batched = initial.batch.is_some();
    let mixed = initial.series.is_some();
    let receipt_counts: Vec<_> = (0..layout.count()).map(|g| initial.receipts(g)).collect();
    let storage_slot = |g| {
        if polling && g == poll_index {
            if mixed { series::POLL_STORAGE } else { 2 }
        } else {
            layout.storage(g)
        }
    };
    let mut direct_pairs: HashSet<_> = (0..layout.count())
        .flat_map(|g| (0..receipt_counts[g]).map(move |i| (g, i)))
        .filter(|&(g, i)| initial.direct_receipt(g, i))
        .collect();
    if limit == 0 {
        return Err("invalid cohort bound".into());
    }
    let mut states = HashSet::from([initial]);
    let mut futures = HashMap::new();
    let mut operations = HashMap::new();
    let mut receipts = HashMap::new();
    let mut seen = HashSet::new();
    let mut claims = HashSet::new();
    let mut captured = HashSet::new();
    let mut addresses = HashMap::new();
    let mut target = None;
    let mut last_future = None;
    let mut report = ReplayReport {
        events: lines.len(),
        maximum_frontier: 1,
    };
    for (offset, line) in lines.iter().enumerate() {
        let f: Vec<_> = line.split(',').collect();
        if f.len() != 4 {
            return Err("invalid cohort row".into());
        }
        let kind = f[0];
        let id: u64 = f[1].parse().map_err(|_| "invalid id")?;
        let arg: i64 = f[2].parse().map_err(|_| "invalid argument")?;
        let extra: i64 = f[3].parse().map_err(|_| "invalid extra")?;
        let duplicate = !matches!(
            kind,
            "signal_touch"
                | "context_touch"
                | "detach_wait"
                | "control_begin"
                | "control_end"
                | "helper_wake_begin"
                | "helper_wake_end"
                | "helper_context"
        );
        let first = seen.insert((kind.to_owned(), id));
        if duplicate && !first {
            return Err(format!("duplicate {kind}/{id}"));
        }
        states = closure(states, &claims, limit)?;
        let mut next = HashSet::new();
        let gtoken = if polling && id == 200 {
            Some(poll_index)
        } else {
            token(id, layout.count())
        };
        let future_event = matches!(
            kind,
            "direct_copy"
                | "direct_settle"
                | "drain_deferred"
                | "drain_receipt"
                | "drain_release"
                | "deferred_begin"
                | "deferred_flag"
                | "deferred_end"
                | "receipt_decision"
                | "ready_selection"
        );
        let indexed = matches!(
            kind,
            "admit"
                | "register_begin"
                | "register_retain"
                | "register_cas"
                | "register_drop"
                | "registered"
                | "copied"
                | "completion_claim"
                | "physical"
                | "owner_settle"
                | "record_release"
                | "worker_wake_begin"
                | "worker_wake_end"
                | "control_begin"
                | "control_end"
        );
        let pair = if future_event {
            futures.get(&id).copied()
        } else if indexed {
            receipts.get(&id).copied()
        } else {
            None
        };
        if (future_event || indexed) && pair.is_none() {
            return Err(format!("unknown receipt/future {id}"));
        }
        match kind {
            "future_bind" => {
                let g = if polling && arg == 200 {
                    poll_index
                } else {
                    token(
                        u64::try_from(arg).map_err(|_| "invalid generation")?,
                        layout.count(),
                    )
                    .ok_or("unknown generation")?
                };
                let i = usize::try_from(extra).map_err(|_| "invalid receipt index")?;
                if i >= (if g == poll_index && polling {
                    1
                } else {
                    receipt_counts[g]
                }) || futures.values().any(|p| *p == (g, i))
                    || mixed && last_future.is_some_and(|prior| id <= prior)
                    || futures.insert(id, (g, i)).is_some()
                {
                    return Err("duplicate future binding".into());
                }
                last_future = Some(id);
                next = states;
            }
            "receipt_link" => {
                let future = u64::try_from(arg).map_err(|_| "invalid future link")?;
                if extra != 0
                    || !futures.contains_key(&future)
                    || operations.insert(id, future).is_some()
                {
                    return Err("invalid receipt link".into());
                }
                next = states;
            }
            "receipt" => {
                let future = *operations.get(&id).ok_or("receipt lacks future link")?;
                let pair = *futures.get(&future).ok_or("unbound future")?;
                if (if polling && arg == 200 {
                    Some(poll_index)
                } else {
                    token(
                        u64::try_from(arg).map_err(|_| "invalid generation")?,
                        layout.count(),
                    )
                }) != Some(pair.0)
                    || extra != 0
                    || receipts.insert(id, pair).is_some()
                {
                    return Err("receipt generation mismatch".into());
                }
                next = states;
            }
            "wake_bind" => {
                let &(g, _) = receipts.get(&id).ok_or("wake binding lacks receipt")?;
                if polling && g == poll_index || extra != 0 || arg <= 1 || arg % 8 != 0 {
                    return Err("invalid wake binding".into());
                }
                if target.is_some_and(|t| t != arg) {
                    return Err("receipts bound different wake targets".into());
                }
                target = Some(arg);
                next = states;
            }
            _ => {
                let (g, i) = pair
                    .or_else(|| {
                        gtoken.map(|g| {
                            (
                                g,
                                if kind == "alias_result" {
                                    usize::try_from(extra).unwrap_or(2)
                                } else {
                                    0
                                },
                            )
                        })
                    })
                    .unwrap_or((0, 0));
                if i >= 2 {
                    return Err("invalid receipt index".into());
                }
                let global = matches!(
                    kind,
                    "shutdown"
                        | "worker_quiescent"
                        | "joined"
                        | "detach"
                        | "runtime_reference_release"
                        | "env_free"
                        | "target_free"
                        | "owner_dispose"
                        | "signal_touch"
                        | "detach_wait"
                        | "helper_wake_begin"
                        | "helper_wake_end"
                        | "helper_quiescent"
                        | "helper_join_wait"
                        | "helper_joined"
                        | "helper_context"
                );
                if global && id != 0 {
                    return Err("foreign global identity".into());
                }
                if !global && !future_event && !indexed && gtoken.is_none() {
                    return Err(format!("unknown generation {id}"));
                }
                let bool_arg = matches!(
                    kind,
                    "control_end"
                        | "completion_claim"
                        | "owner_settle"
                        | "notify_enter"
                        | "sdk_exchange"
                        | "joined"
                        | "poll_bind"
                );
                let args_ok = if kind == "generation" {
                    let polling_carrier = polling && g == poll_index;
                    let alignment = if polling_carrier { 4 } else { 8 };
                    arg > 1
                        && arg % alignment == 0
                        && (!mixed || !polling_carrier || arg % 8 == 4)
                        && extra == i64::try_from(storage_slot(g)).unwrap()
                } else if kind == "carrier_abi" {
                    let version = if g == poll_index {
                        1
                    } else {
                        series::VERSIONS.get(g).copied().unwrap_or(0)
                    };
                    mixed
                        && arg == i64::from(version)
                        && extra == 8 * (1 + arg)
                        && seen.contains(&("generation".into(), id))
                } else if kind == "generation_status" {
                    (0..=1).contains(&arg) && (0..=1).contains(&extra)
                } else if kind == "alias_result" {
                    arg == PAYLOAD[0] && extra == 1
                } else if kind == "batch_target" {
                    (batched || mixed) && Some(arg) == target && extra == 0
                } else if kind == "merge_wake" {
                    (batched && matches!((id, arg), (101, 102) | (102, 101))
                        || mixed
                            && g < layout.count()
                            && arg == 100 + i64::try_from(g ^ 1).unwrap())
                        && Some(extra) == target
                        && seen.contains(&("batch_target".into(), id))
                        && u64::try_from(arg)
                            .is_ok_and(|leader| seen.contains(&("batch_target".into(), leader)))
                } else if kind == "registered" {
                    (0..=1).contains(&arg) && (1..=5).contains(&extra)
                } else if kind == "register_cas" {
                    (arg == 1 && extra == 0) || (arg == 0 && (1..=2).contains(&extra))
                } else if matches!(kind, "control_begin" | "deferred_begin") {
                    (1..=2).contains(&arg) && extra == 0
                } else if kind == "deferred_end" {
                    arg == 1 && extra == 0
                } else if matches!(kind, "receipt_decision" | "direct_settle") {
                    (0..=2).contains(&arg) && extra == 0
                } else if matches!(kind, "copied" | "direct_copy") {
                    extra == 0
                } else if bool_arg {
                    (0..=1).contains(&arg) && extra == 0
                } else {
                    arg == 0 && extra == 0
                };
                if !args_ok {
                    return Err(format!("invalid schema: {line}"));
                }
                if kind == "generation" {
                    let storage = storage_slot(g);
                    if let Some(old) = addresses.get(&storage) {
                        if *old != arg {
                            return Err("generation changed physical storage".into());
                        }
                    } else {
                        if addresses.values().any(|p| *p == arg) {
                            return Err("distinct storage addresses alias".into());
                        }
                        addresses.insert(storage, arg);
                    }
                }
                for s in &states {
                    if let Some(n) = observe(s, kind, g, i, arg, extra) {
                        next.insert(n);
                    }
                }
                if kind == "completion_claim" {
                    claims.insert((g, i));
                }
                if unordered && kind == "direct_copy" {
                    direct_pairs.insert((g, i));
                }
                if kind == "sdk_exchange" && arg == 1 {
                    captured.insert(g);
                }
            }
        }
        if next.is_empty() {
            return Err(format!("event {offset} does not refine cohort: {line}"));
        }
        if next.len() > limit {
            return Err("cohort replay state limit; incomplete".into());
        }
        report.maximum_frontier = report.maximum_frontier.max(next.len());
        states = next;
    }
    for g in 0..layout.count() {
        let id = 100 + u64::try_from(g).unwrap();
        if mixed {
            for kind in ["carrier_abi", "batch_target"] {
                if !seen.contains(&(kind.to_owned(), id)) {
                    return Err(format!("missing {kind}/{id}"));
                }
            }
        }
        for kind in [
            "call_enter",
            "sdk_start",
            "generation",
            "callback_admit",
            "callback_begin",
            "callback_return",
            "call_return",
            "call_close",
            "call_settle",
            "call_read",
            "handoff",
            "call_env_free",
            "publish_begin",
            "sdk_prefix",
            "sdk_exchange",
            "publish_end",
            "close_admission",
            "carrier_free",
            "sdk_pin_free",
            "producer_release",
            "producer_released",
        ] {
            if matches!(kind, "producer_release" | "producer_released")
                && (if unordered {
                    !captured.contains(&g)
                } else {
                    matches!(mode, CohortMode::Early | CohortMode::Deferred) && g == 1
                })
            {
                continue;
            }
            if !seen.contains(&(kind.to_owned(), id)) {
                return Err(format!("missing {kind}/{id}"));
            }
        }
    }
    let future_count = receipt_counts.iter().sum::<usize>() + usize::from(polling);
    if futures.len() != future_count
        || receipts.len() != future_count - direct_pairs.len()
        || target.is_none()
    {
        return Err("incomplete receipt bindings".into());
    }
    for (&op, &(g, i)) in &receipts {
        let future = operations[&op];
        for kind in [
            "admit",
            "wake_bind",
            "register_begin",
            "register_retain",
            "register_cas",
            "registered",
            "copied",
            "completion_claim",
            "physical",
            "owner_settle",
            "record_release",
            "receipt",
        ] {
            if polling
                && g == poll_index
                && matches!(
                    kind,
                    "wake_bind"
                        | "register_begin"
                        | "register_retain"
                        | "register_cas"
                        | "registered"
                )
            {
                continue;
            }
            if binding == Binding::RustChannel
                && kind == "copied"
                && (polling && g == poll_index || mixed && series::VERSIONS[g] == 2)
            {
                continue;
            }
            if !seen.contains(&(kind.to_owned(), op)) {
                return Err(format!("missing {kind}/{op}"));
            }
        }
        for kind in ["drain_receipt", "receipt_decision", "drain_release"] {
            if !seen.contains(&(kind.to_owned(), future)) {
                return Err(format!("missing {kind}/{future} g{g}r{i}"));
            }
        }
    }
    if !direct_pairs.is_empty() {
        for (&future, &(g, i)) in &futures {
            if direct_pairs.contains(&(g, i)) {
                for k in [
                    "drain_deferred",
                    "direct_copy",
                    "direct_settle",
                    "drain_release",
                ] {
                    if !seen.contains(&(k.to_owned(), future)) {
                        return Err(format!("missing {k}/{future}"));
                    }
                }
                if operations.values().any(|f| *f == future) {
                    return Err("direct future acquired a receipt".into());
                }
            }
        }
    }
    if polling {
        if mixed && !seen.contains(&("carrier_abi".to_owned(), 200)) {
            return Err("missing polling carrier_abi".into());
        }
        for k in [
            "call_enter",
            "sdk_start",
            "generation",
            "callback_admit",
            "callback_begin",
            "callback_return",
            "call_return",
            "call_close",
            "call_settle",
            "call_read",
            "handoff",
            "call_env_free",
            "poll_bind",
            "poll_publish",
            "close_admission",
            "carrier_free",
            "sdk_pin_free",
        ] {
            if !seen.contains(&(k.to_owned(), 200)) {
                return Err(format!("missing polling {k}"));
            }
        }
        if binding == Binding::CJoined {
            for k in [
                "helper_wake_begin",
                "helper_wake_end",
                "helper_quiescent",
                "helper_join_wait",
                "helper_joined",
            ] {
                if !seen.contains(&(k.to_owned(), 0)) {
                    return Err(format!("missing {k}"));
                }
            }
        }
    }
    if (extended || unordered || batched || mixed) && binding == Binding::RustChannel && polling {
        let (&op, _) = receipts
            .iter()
            .find(|(_, pair)| pair.0 == poll_index)
            .ok_or("missing polling receipt")?;
        for k in ["worker_wake_begin", "worker_wake_end"] {
            if !seen.contains(&(k.to_owned(), op)) {
                return Err(format!("missing polling {k}"));
            }
        }
    }
    if !states.iter().any(Protocol::terminal) {
        return Err("incomplete cohort lifetime".into());
    }
    Ok(report)
}
