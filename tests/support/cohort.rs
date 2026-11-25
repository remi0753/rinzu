use rinzu_runtime_model::native_lifetime::{Binding, CohortLayout, replay_cohort};
pub fn reject_corruptions(
    binding: Binding,
    layout: CohortLayout,
    controlled: bool,
    events: &[String],
) -> usize {
    let rejects = |bad: &[String], label: &str| {
        assert!(
            replay_cohort(binding, layout, controlled, bad, 100_000).is_err(),
            "accepted corrupt cohort ({label}):\n{}",
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
            f[2] = if kind == "receipt_decision" {
                if f[2] == "2" { "1".into() } else { "2".into() }
            } else {
                f[2].parse::<i64>().unwrap().saturating_add(8).to_string()
            };
            if kind == "generation" {
                f = events[i].split(',').map(str::to_owned).collect();
                f[3] = "8".into();
            }
            bad[i] = f.join(",");
            rejects(&bad, "boundary mutation");
            count += 1;
        }
    }
    for (first, second) in [
        ("handoff,100,", "call_env_free,100,"),
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
    assert!(replay_cohort(binding, layout, controlled, events, 1).is_err());
    count + 2
}
