use rinzu_runtime_model::native_lifetime::{
    Binding, CohortMode, ReplayReport, replay_combination, replay_extended,
};
#[allow(
    dead_code,
    reason = "Shared support is included by profile-specific native harnesses"
)]
pub fn reject_corruptions(
    binding: Binding,
    mode: CohortMode,
    controlled: bool,
    events: &[String],
) -> usize {
    reject(binding, events, |bad, limit| {
        replay_combination(binding, mode, controlled, bad, limit)
    })
}
#[allow(
    dead_code,
    reason = "Shared support is included by profile-specific native harnesses"
)]
pub fn reject_extended(
    binding: Binding,
    mode: CohortMode,
    controlled: bool,
    polling: bool,
    events: &[String],
) -> usize {
    reject(binding, events, |bad, limit| {
        replay_extended(binding, mode, controlled, polling, bad, limit)
    })
}
fn reject(
    binding: Binding,
    events: &[String],
    replay: impl Fn(&[String], usize) -> Result<ReplayReport, String>,
) -> usize {
    let rejects = |bad: &[String], label: &str| {
        assert!(
            replay(bad, 100_000).is_err(),
            "accepted corrupt combination ({label}):\n{}",
            bad.join("\n")
        );
    };
    let mut count = 0;
    for (i, e) in events.iter().enumerate() {
        if ["signal_touch,", "context_touch,", "detach_wait,"]
            .iter()
            .any(|p| e.starts_with(p))
        {
            continue;
        }
        let mut bad = events.to_vec();
        bad.remove(i);
        rejects(&bad, &format!("delete {i} {e}"));
        count += 1;
        let mut bad = events.to_vec();
        bad.insert(i, e.clone());
        rejects(&bad, &format!("duplicate {i} {e}"));
        count += 1;
    }
    for kind in [
        "copied",
        "direct_copy",
        "direct_settle",
        "receipt_decision",
        "wake_bind",
        "generation",
        "registered",
        "receipt",
        "receipt_link",
        "future_bind",
        "alias_result",
        "generation_status",
    ] {
        if let Some(i) = events
            .iter()
            .position(|e| e.starts_with(&format!("{kind},")))
        {
            let mut bad = events.to_vec();
            let mut f: Vec<_> = bad[i].split(',').map(str::to_owned).collect();
            f[2] = if matches!(kind, "receipt_decision" | "direct_settle") {
                if f[2] == "2" { "1".into() } else { "2".into() }
            } else {
                f[2].parse::<i64>().unwrap().saturating_add(8).to_string()
            };
            if kind == "generation" {
                f = events[i].split(',').map(str::to_owned).collect();
                f[3] = "8".into();
            }
            if kind == "registered" {
                f = events[i].split(',').map(str::to_owned).collect();
                f[3] = (f[3].parse::<i64>().unwrap() + 1).to_string();
            }
            bad[i] = f.join(",");
            rejects(&bad, "boundary mutation");
            count += 1;
        }
    }
    for (first, second) in [
        ("handoff,100,", "call_env_free,100,"),
        ("helper_wake_end,", "helper_joined,"),
        ("helper_joined,", "detach,"),
        ("direct_copy,", "direct_settle,"),
        ("sdk_exchange,101,", "callback_return,101,"),
        ("worker_wake_end,", "record_release,"),
        ("drain_release,", "record_release,"),
        ("completion_claim,", "physical,"),
        ("carrier_free,100,", "generation,102,"),
    ] {
        if let (Some(a), Some(b)) = (
            events.iter().position(|e| e.starts_with(first)),
            events.iter().position(|e| e.starts_with(second)),
        ) {
            let mut bad = events.to_vec();
            bad.swap(a, b);
            rejects(&bad, "boundary mutation");
            count += 1;
        }
    }
    for (token, displacement) in [("100", 4), ("200", 2)] {
        let prefix = format!("generation,{token},");
        if let Some(i) = events.iter().position(|e| e.starts_with(&prefix)) {
            let fields = events[i].strip_prefix(&prefix).unwrap();
            let (address, slot) = fields.split_once(',').unwrap();
            let mut bad = events.to_vec();
            bad[i] = format!(
                "{prefix}{},{slot}",
                address.parse::<i64>().unwrap() + displacement
            );
            rejects(&bad, "carrier below its ABI alignment");
            count += 1;
        }
    }
    if let Some(i) = events.iter().position(|e| {
        let fields: Vec<_> = e.split(',').collect();
        fields[0] == "receipt" && fields[2] == "200"
    }) {
        let polling_receipt = events[i].split(',').nth(1).unwrap();
        let target = events
            .iter()
            .find_map(|e| e.strip_prefix("wake_bind,")?.split(',').nth(1))
            .unwrap();
        let mut bad = events.to_vec();
        bad.insert(i + 1, format!("wake_bind,{polling_receipt},{target},0"));
        rejects(&bad, "v1 receipt bound a notification target");
        count += 1;
        if binding == Binding::RustChannel {
            let prefix = format!("completion_claim,{polling_receipt},");
            let claim = events.iter().position(|e| e.starts_with(&prefix)).unwrap();
            let mut bad = events.to_vec();
            bad.insert(claim, format!("copied,{polling_receipt},0,0"));
            rejects(&bad, "Rust v1 receipt invented a payload copy");
            count += 1;
        }
    }
    for e in [
        "unknown_event,0,0,0",
        "call_enter,400,0,0",
        "env_free,100,0,0",
    ] {
        let mut bad = events.to_vec();
        bad.insert(0, e.into());
        rejects(&bad, "boundary mutation");
        count += 1;
    }
    rejects(&events[..events.len() - 1], "truncate");
    assert!(replay(events, 1).is_err());
    count + 2
}
