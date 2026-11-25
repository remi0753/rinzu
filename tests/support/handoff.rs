use rinzu_runtime_model::native_lifetime::{Binding, replay_handoff};

pub fn reject_corruptions(binding: Binding, full: bool, events: &[String]) -> usize {
    let rejects = |bad: &[String]| {
        assert!(
            replay_handoff(binding, full, 7183, bad, 100_000).is_err(),
            "accepted corrupt trace:\n{}",
            bad.join("\n")
        )
    };
    let mut count = 0;
    for (i, event) in events.iter().enumerate() {
        if ["signal_touch,", "context_touch,", "detach_wait,"]
            .iter()
            .any(|p| event.starts_with(p))
        {
            continue;
        }
        let mut bad = events.to_vec();
        bad.remove(i);
        rejects(&bad);
        count += 1;
        let mut bad = events.to_vec();
        bad.insert(i, event.clone());
        rejects(&bad);
        count += 1;
    }
    for bad_event in [
        "owner_dispose,0,0,0",
        "sdk_pin_free,0,0,0",
        "unknown_boundary,0,0,0",
        "call_enter,9,0,0",
        "call_enter,0,1,0",
        "call_enter,0,0,1",
    ] {
        let mut bad = events.to_vec();
        bad.insert(0, bad_event.into());
        rejects(&bad);
        count += 1;
    }
    for kind in ["copied,", "direct_copy,"] {
        if let Some(index) = events.iter().position(|e| e.starts_with(kind)) {
            let mut bad = events.to_vec();
            let f: Vec<_> = bad[index].split(',').collect();
            bad[index] = format!("{},{},19,0", f[0], f[1]);
            rejects(&bad);
            count += 1;
        }
    }
    for kind in ["direct_settle,", "receipt_decision,"] {
        if let Some(i) = events.iter().position(|e| e.starts_with(kind)) {
            let f: Vec<_> = events[i].split(',').collect();
            let wrong = if f[2] == "2" { 1 } else { 2 };
            let mut bad = events.to_vec();
            bad[i] = format!("{},{},{wrong},0", f[0], f[1]);
            rejects(&bad);
            count += 1;
        }
    }
    for (first, second) in [
        ("drain_release,", "record_release,"),
        ("completion_claim,", "physical,"),
        ("handoff,", "call_env_free,"),
        ("sdk_start,", "call_return,"),
    ] {
        if let (Some(a), Some(b)) = (
            events.iter().position(|e| e.starts_with(first)),
            events.iter().position(|e| e.starts_with(second)),
        ) {
            let mut bad = events.to_vec();
            bad.swap(a, b);
            rejects(&bad);
            count += 1;
        }
    }
    rejects(&events[..events.len() - 1]);
    count += 1;
    assert!(replay_handoff(binding, full, 7183, events, 1).is_err());
    count + 1
}
