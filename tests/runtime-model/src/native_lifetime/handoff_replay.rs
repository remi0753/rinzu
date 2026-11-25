use super::admission::Owner;
use super::call::Action as K;
use super::replay::Event;
use super::{
    Binding, ComposedAction as C, Decision, Handoff, HandoffAction as H, Protocol,
    RegistrationAction as R, ReplayReport,
};
use std::collections::{HashSet, VecDeque};

fn checked(s: &Handoff, a: H) -> Option<Handoff> {
    if !s.enabled(a) {
        return None;
    }
    let mut n = s.clone();
    n.apply(a);
    (s.check_step(a, &n).is_ok() && n.check().is_ok()).then_some(n)
}
fn closure(
    input: HashSet<Handoff>,
    publishing: bool,
    prefix: bool,
    claim: bool,
    deferred: Option<Decision>,
    limit: usize,
) -> Result<HashSet<Handoff>, String> {
    let mut queue: VecDeque<_> = input.iter().cloned().collect();
    let mut seen = input;
    while let Some(s) = queue.pop_front() {
        for a in s.actions() {
            let hidden = (matches!(a,H::Control(d) if Some(d)==deferred))
                || matches!(
                    a,
                    H::SdkWrite
                        | H::Call(K::FinishContinuation | K::SettleCallback(_))
                        | H::Runtime(
                            R::CompareExchange(_)
                                | R::Product(C::BeginCopy(_) | C::Claim(_) | C::ResolveControl(_))
                        )
                )
                || (publishing && matches!(a, H::SdkPrefix))
                || (publishing && prefix && a == H::SdkPublish)
                || (claim && matches!(a, H::Runtime(R::Product(C::Physical(_)))));
            if !hidden {
                continue;
            }
            let next = checked(&s, a).ok_or("hidden handoff transition violated invariant")?;
            if seen.insert(next.clone()) {
                if seen.len() > limit {
                    return Err("handoff replay state limit; incomplete".into());
                }
                queue.push_back(next);
            }
        }
    }
    Ok(seen)
}
fn custom(s: &Handoff, kind: &str, arg: i64, extra: i64, payload: i64) -> Option<Handoff> {
    if extra != 0 {
        return None;
    }
    let a = match (kind, arg) {
        ("call_enter", 0) => H::Call(K::Enter),
        ("sdk_start", 0) => H::StartSdk,
        ("callback_admit", 0) => H::Call(K::Admit(0)),
        ("callback_begin", 0) => H::Call(K::Begin(0)),
        ("callback_return", 0) => H::Call(K::ReturnCallback(0)),
        ("call_return", 0) => H::Call(K::ReturnNative),
        ("call_close", 0) => H::Call(K::CloseGate),
        ("call_settle", 0) => H::Call(K::SettleGate),
        ("call_read", 0) => H::Call(K::ReadResult),
        ("handoff", 0) => H::HandOff,
        ("call_env_free", 0) => H::Call(K::ReleaseEnvironment),
        ("drain_deferred", 0) if s.admission.owner == Owner::Caller && s.full => H::TryAdmit,
        ("drain_receipt", 0) if s.admission.owner == Owner::Caller && !s.full => H::TryAdmit,
        ("credit", 0) => H::Credit,
        ("drain_receipt", 0) if s.admission.owner == Owner::Deferred => H::Retry,
        ("direct_copy", v) if v == payload => H::DirectCopy,
        ("drain_release", 0) => H::ReleaseDrain,
        ("sdk_pin_free", 0) => H::ReleasePin,
        ("owner_dispose", 0) => H::Dispose,
        _ => return None,
    };
    checked(s, a)
}

/// Replay one complete, non-spliced Call/admission/completion/wake execution.
/// Missing boundaries, foreign identities, duplicates, malformed fields and
/// exhausted search bounds are errors. No projection/fallback accepts a trace.
/// # Errors
/// Returns the first unmatched event or incomplete terminal obligation.
#[allow(
    clippy::too_many_lines,
    reason = "Keep invocation intervals, mandatory observations and terminal checks in one auditable pass"
)]
pub fn replay_handoff(
    binding: Binding,
    full: bool,
    payload: i64,
    lines: &[String],
    limit: usize,
) -> Result<ReplayReport, String> {
    if limit == 0 || binding == Binding::CJoined || (full && binding != Binding::RustChannel) {
        return Err("invalid replay bound".into());
    }
    let mut states = HashSet::from([Handoff::new(binding, full)]);
    let mut seen = HashSet::new();
    let mut id = None;
    let mut publishing = false;
    let mut prefix = false;
    let mut claim = false;
    let mut deferred = None;
    let mut report = ReplayReport {
        events: lines.len(),
        maximum_frontier: 1,
    };
    let custom_kinds = [
        "call_enter",
        "sdk_start",
        "callback_admit",
        "callback_begin",
        "callback_return",
        "call_return",
        "call_close",
        "call_settle",
        "call_read",
        "handoff",
        "call_env_free",
        "drain_deferred",
        "drain_receipt",
        "credit",
        "direct_copy",
        "drain_release",
        "sdk_pin_free",
        "owner_dispose",
    ];
    for (offset, line) in lines.iter().enumerate() {
        let f: Vec<_> = line.split(',').collect();
        if f.len() != 4 {
            return Err(format!("invalid event {offset}"));
        }
        let n: u64 = f[1].parse().map_err(|_| "invalid id")?;
        let arg: i64 = f[2].parse().map_err(|_| "invalid argument")?;
        let extra: i64 = f[3].parse().map_err(|_| "invalid argument")?;
        let is_custom = custom_kinds.contains(&f[0]);
        if is_custom && n != 0 {
            return Err("foreign handoff identity".into());
        }
        if !matches!(
            f[0],
            "signal_touch" | "context_touch" | "detach_wait" | "control_begin" | "control_end"
        ) && !seen.insert((f[0].to_owned(), n))
        {
            return Err(format!("duplicate {}", f[0]));
        }
        if f[0] == "publish_begin" {
            publishing = true;
        }
        if deferred.is_some()
            && !matches!(
                f[0],
                "deferred_flag"
                    | "deferred_end"
                    | "publish_begin"
                    | "sdk_prefix"
                    | "sdk_exchange"
                    | "publish_end"
            )
        {
            return Err("Owner event inside deferred control invocation".into());
        }
        states = closure(states, publishing, prefix, claim, deferred, limit)?;
        let mut next = HashSet::new();
        if matches!(f[0], "deferred_begin" | "deferred_flag" | "deferred_end") {
            if n != 0 || extra != 0 {
                return Err("invalid deferred control schema".into());
            }
            match f[0] {
                "deferred_begin" if deferred.is_none() && (1..=2).contains(&arg) => {
                    next.extend(
                        states
                            .iter()
                            .filter(|s| s.runtime.is_none() && s.admission.owner == Owner::Deferred)
                            .cloned(),
                    );
                }
                "deferred_flag" if deferred.is_some() && arg == 0 => {
                    next.extend(
                        states
                            .iter()
                            .filter(|s| s.admission.cancelled && Some(s.decision) == deferred)
                            .cloned(),
                    );
                }
                "deferred_end" if deferred.is_some() && (0..=1).contains(&arg) => {
                    let flag = seen.contains(&("deferred_flag".to_owned(), 0));
                    next.extend(
                        states
                            .iter()
                            .filter(|s| {
                                if arg == 1 {
                                    flag && s.admission.cancelled && Some(s.decision) == deferred
                                } else {
                                    !flag && s.sdk == 3 && s.decision == Decision::Pending
                                }
                            })
                            .cloned(),
                    );
                }
                _ => return Err("unmatched deferred control interval".into()),
            }
        } else if f[0] == "signal_touch" {
            if n != 0 || arg != 0 || extra != 0 {
                return Err("invalid signal touch".into());
            }
            next.extend(
                states
                    .iter()
                    .filter(|s| {
                        !s.disposed && s.runtime.as_ref().is_some_and(|r| r.inner.environment)
                    })
                    .cloned(),
            );
        } else if matches!(f[0], "direct_settle" | "receipt_decision") {
            if n != 0 || extra != 0 || !(0..=2).contains(&arg) {
                return Err("invalid direct settlement".into());
            }
            next.extend(
                states
                    .iter()
                    .filter(|s| {
                        (s.runtime.is_none() == (f[0] == "direct_settle"))
                            && s.admission.collected
                            && match s.decision {
                                Decision::Completed => arg == 0,
                                Decision::Cancelled => arg == 1,
                                Decision::TimedOut => arg == 2,
                                Decision::Pending => false,
                            }
                    })
                    .cloned(),
            );
        } else if is_custom {
            for s in &states {
                if let Some(n) = custom(s, f[0], arg, extra, payload) {
                    next.insert(n);
                }
            }
        } else {
            let e = Event::parse(line)?;
            if e.kind == "admit" && id.replace(e.id).is_some() {
                return Err("duplicate receipt".into());
            }
            if e.indexed() && id != Some(e.id) {
                return Err("unknown receipt".into());
            }
            for s in &states {
                if let Some(r) = &s.runtime {
                    if e.kind == "admit" {
                        if r.inner.completion.stage(0) == super::ReceiptStage::Waiting {
                            next.insert(s.clone());
                        }
                    } else if let Some(r2) = super::registration_replay::observe(&e, r, 0, payload)
                    {
                        if r == &r2 {
                            next.insert(s.clone());
                        } else {
                            // The component observation must also be enabled by
                            // the product's handoff and drain ownership guards.
                            for action in s.actions() {
                                if matches!(action, H::Runtime(_))
                                    && let Some(n) = checked(s, action)
                                    && n.runtime.as_ref() == Some(&r2)
                                {
                                    next.insert(n);
                                }
                            }
                        }
                    }
                } else {
                    let bit = match e.kind {
                        "shutdown" => Some(1),
                        "worker_quiescent" => Some(2),
                        "detach" => Some(4),
                        "joined" if e.arg == 0 => Some(8),
                        "runtime_reference_release" => Some(16),
                        "target_free" => Some(32),
                        "env_free" => Some(64),
                        _ => None,
                    };
                    if let Some(bit) = bit {
                        if let Some(n) = checked(s, H::Unused(bit)) {
                            next.insert(n);
                        }
                        continue;
                    }
                    let valid = match e.kind {
                        "publish_begin" => s.started && s.sdk < 3,
                        "sdk_prefix" => s.sdk >= 2,
                        "sdk_exchange" => s.sdk == 3 && e.arg == 0,
                        "publish_end" => s.sdk == 3,
                        _ => false,
                    };
                    if valid {
                        next.insert(s.clone());
                    }
                }
            }
        }
        if next.is_empty() {
            return Err(format!(
                "event {offset} does not refine Call/admission/wake: {line}"
            ));
        }
        if next.len() > limit {
            return Err("handoff replay state limit; incomplete".into());
        }
        report.maximum_frontier = report.maximum_frontier.max(next.len());
        states = next;
        if f[0] == "sdk_prefix" {
            if !publishing {
                return Err("prefix outside publication".into());
            }
            prefix = true;
        }
        if f[0] == "sdk_exchange" && (!publishing || !prefix) {
            return Err("exchange before prefix observation".into());
        }
        if f[0] == "publish_end" {
            publishing = false;
        }
        if f[0] == "deferred_begin" {
            deferred = Some(if arg == 1 {
                Decision::Cancelled
            } else {
                Decision::TimedOut
            });
        }
        if f[0] == "deferred_end" {
            deferred = None;
        }
        if f[0] == "completion_claim" {
            claim = true;
        }
    }
    for kind in [
        "call_enter",
        "sdk_start",
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
        "drain_release",
        "sdk_pin_free",
        "owner_dispose",
    ] {
        if !seen.contains(&(kind.to_owned(), 0)) {
            return Err(format!("missing {kind}"));
        }
    }
    for s in &states {
        if s.runtime.is_some() {
            if !seen.contains(&("receipt_decision".to_owned(), 0)) {
                return Err("missing receipt decision".into());
            }
            let id = id.ok_or("missing receipt")?;
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
                if !seen.contains(&(kind.to_owned(), id)) {
                    return Err(format!("missing {kind}"));
                }
            }
        }
    }
    if seen.contains(&("direct_copy".to_owned(), 0))
        && !seen.contains(&("direct_settle".to_owned(), 0))
    {
        return Err("missing direct settlement".into());
    }
    if seen.contains(&("producer_release".to_owned(), 0))
        && !seen.contains(&("producer_released".to_owned(), 0))
    {
        return Err("missing producer release return".into());
    }
    if publishing || deferred.is_some() || !states.iter().any(Protocol::terminal) {
        return Err("incomplete handoff trace".into());
    }
    Ok(report)
}
