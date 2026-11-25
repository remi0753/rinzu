use rinzu_runtime_model::native_lifetime::{Binding, replay_unordered};
use std::collections::HashMap;

pub const PERMUTATIONS: [[usize; 3]; 6] = [
    [0, 1, 2],
    [0, 2, 1],
    [1, 0, 2],
    [1, 2, 0],
    [2, 0, 1],
    [2, 1, 0],
];

pub fn check(binding: Binding, controlled: bool, expected: [usize; 3], events: &[String]) {
    let replay = |lines: &[String]| replay_unordered(binding, controlled, lines, 100_000);
    let report = replay(events).unwrap_or_else(|e| panic!("{e}\n{}", events.join("\n")));
    let mut future = HashMap::new();
    let mut operation = HashMap::new();
    let mut order = Vec::new();
    let mut normal_settles = Vec::new();
    for (index, line) in events.iter().enumerate() {
        let fields: Vec<_> = line.split(',').collect();
        let id = fields[1].parse::<u64>().unwrap();
        match fields[0] {
            "future_bind" => {
                let slot = match fields[2] {
                    "100" => 0,
                    "101" => 1,
                    "200" => 2,
                    _ => unreachable!(),
                };
                future.insert(id, slot);
            }
            "receipt_link" => {
                operation.insert(id, fields[2].parse::<u64>().unwrap());
            }
            "direct_settle" => order.push(future[&id]),
            "owner_settle" => {
                order.push(future[&operation[&id]]);
                normal_settles.push((index, id));
            }
            _ => {}
        }
    }
    assert_eq!(order, expected, "actual Owner recovery order");

    let position = |prefix: &str| {
        events
            .iter()
            .position(|line| line.starts_with(prefix))
            .unwrap()
    };
    let reject = |bad: &[String], reason| {
        assert!(
            replay(bad).is_err(),
            "accepted {reason}\n{}",
            bad.join("\n")
        );
    };
    let mut rejected = 0;
    for prefix in ["call_env_free,200,", "sdk_pin_free,200,", "env_free,0,"] {
        let mut bad = events.to_vec();
        bad.remove(position(prefix));
        reject(&bad, prefix);
        rejected += 1;
    }
    let (settle, op) = normal_settles[0];
    let mut bad = events.to_vec();
    let claim = position(&format!("completion_claim,{op},"));
    bad.swap(settle, claim);
    reject(&bad, "receipt settled before its physical claim");
    rejected += 1;

    let mut bad = events.to_vec();
    bad.swap(
        position("carrier_free,100,"),
        position("publish_begin,100,"),
    );
    reject(&bad, "carrier retired before publication");
    rejected += 1;

    let mut bad = events.to_vec();
    let decisive = events
        .iter()
        .position(|line| {
            line.starts_with("receipt_decision,") || line.starts_with("direct_settle,")
        })
        .unwrap();
    let parts: Vec<_> = events[decisive].split(',').collect();
    let different = if parts[2] == "1" { 2 } else { 1 };
    bad[decisive] = format!("{},{},{different},0", parts[0], parts[1]);
    reject(&bad, "substituted terminal decision");
    rejected += 1;

    if binding == Binding::RustChannel {
        let (&deferred, _) = future.iter().find(|(_, slot)| **slot == 1).unwrap();
        let mut bad = events.to_vec();
        bad.remove(position(&format!("drain_deferred,{deferred},")));
        reject(&bad, "omitted initially full drain admission");
        rejected += 1;
        let readmission = format!("drain_receipt,{deferred},");
        if let Some(at) = events
            .iter()
            .position(|line| line.starts_with(&readmission))
        {
            let mut bad = events.to_vec();
            let early = bad.remove(at);
            bad.insert(settle, early);
            reject(&bad, "retry before mailbox settlement returns credit");
            rejected += 1;
        }
    } else {
        let mut bad = events.to_vec();
        bad.swap(position("helper_wake_end,"), position("helper_joined,"));
        reject(&bad, "helper joined before wake returned");
        rejected += 1;
    }
    assert!(replay_unordered(binding, controlled, events, 1).is_err());
    println!(
        "UNORDERED TRACE {binding:?} controlled={controlled} order={expected:?} events={} frontier={} rejected={}",
        report.events,
        report.maximum_frontier,
        rejected + 1
    );
}
