use rinzu_runtime_model::native_lifetime::{BatchSchedule, Binding, replay_batched};
use std::collections::HashMap;

pub fn check(
    binding: Binding,
    schedule: BatchSchedule,
    controlled: bool,
    leader: u64,
    events: &[String],
) {
    let replay = |rows: &[String]| replay_batched(binding, schedule, controlled, rows, 100_000);
    let report = replay(events).unwrap_or_else(|e| panic!("{e}\n{}", events.join("\n")));
    let mut futures = HashMap::new();
    let mut operations = HashMap::new();
    let mut physical = HashMap::new();
    let mut collected = Vec::new();
    let mut direct = None;
    for (offset, event) in events.iter().enumerate() {
        let fields: Vec<_> = event.split(',').collect();
        let id = fields[1].parse::<u64>().unwrap();
        match fields[0] {
            "future_bind" => {
                futures.insert(id, (fields[2].parse::<u64>().unwrap(), fields[3]));
            }
            "receipt_link" => {
                operations.insert(id, fields[2].parse::<u64>().unwrap());
            }
            "physical" => {
                physical.insert(id, offset);
            }
            "direct_settle" => {
                assert!(direct.replace((futures[&id], offset, fields[2])).is_none());
            }
            "owner_settle" => {
                let pair = futures[&operations[&id]];
                if matches!(pair.0, 101 | 102) {
                    collected.push((pair, id, offset));
                }
            }
            _ => {}
        }
    }
    let (first, last) = if schedule != BatchSchedule::Staggered {
        let first_turn = if schedule == BatchSchedule::TogetherDeferred {
            let (pair, offset, decision) = direct.expect("missing direct collection");
            assert_eq!(pair, (102, "1"));
            assert_eq!(decision, if controlled { "2" } else { "0" });
            assert!(offset < collected[0].2);
            offset
        } else {
            assert!(direct.is_none());
            collected[0].2
        };
        assert!(
            collected.iter().all(|(_, id, _)| physical[id] < first_turn),
            "the Owner did not have all admitted physical receipts"
        );
        assert_eq!(collected[0].0, (101, "0"));
        assert!(collected.windows(2).all(|pair| pair[0].0 < pair[1].0));
        (101, 102)
    } else {
        let other = 203 - leader;
        let exchange = events
            .iter()
            .position(|l| l.starts_with(&format!("sdk_exchange,{other},1,")))
            .unwrap();
        assert!(
            collected[..2]
                .iter()
                .all(|(_, _, offset)| *offset < exchange)
        );
        (leader, other)
    };
    assert_eq!(
        collected
            .iter()
            .map(|((g, _), _, _)| *g)
            .collect::<Vec<_>>(),
        if direct.is_some() {
            vec![first, first, last]
        } else {
            vec![first, first, last, last]
        },
    );
    let duplicate = 203 - leader;
    let position = |prefix: &str| events.iter().position(|l| l.starts_with(prefix)).unwrap();
    let notify = position(&format!("notify_enter,{leader},"));
    let returned = position(&format!("producer_released,{duplicate},"));
    assert!(returned < notify);
    let batch_notifies = events
        .iter()
        .filter(|l| l.starts_with("notify_enter,101,") || l.starts_with("notify_enter,102,"));
    assert_eq!(batch_notifies.count(), 1);
    let env = position("env_free,0,");
    assert!(position("producer_release,100,") > env);
    assert!(position(&format!("producer_release,{leader},")) > env);

    let reject = |rows: &[String], reason| {
        assert!(replay(rows).is_err(), "accepted {reason}");
    };
    let mut corrupt = events.to_vec();
    corrupt.swap(returned, notify);
    reject(&corrupt, "notifying before the duplicate returned");

    let mut corrupt = events.to_vec();
    let release = position(&format!("producer_release,{duplicate},"));
    corrupt[release] = "producer_release,100,0,0".into();
    reject(&corrupt, "consuming the retired generation's SDK reference");

    let mut corrupt = events.to_vec();
    let capture = position("batch_target,102,");
    let address = events[capture]
        .split(',')
        .nth(2)
        .unwrap()
        .parse::<i64>()
        .unwrap();
    corrupt[capture] = format!("batch_target,102,{},0", address + 8);
    reject(&corrupt, "coalescing across two target addresses");

    let mut corrupt = events.to_vec();
    let old = position("generation,100,");
    let new = position("generation,102,");
    let old_address = events[old]
        .split(',')
        .nth(2)
        .unwrap()
        .parse::<i64>()
        .unwrap();
    corrupt[new] = format!("generation,102,{},0", old_address + 8);
    reject(&corrupt, "changing the declared reused address");

    let mut corrupt = events.to_vec();
    corrupt.remove(position(&format!(
        "copied,{},",
        collected.last().unwrap().1
    )));
    reject(&corrupt, "omitting a copy required by an admitted receipt");

    if binding == Binding::CJoined {
        let mut corrupt = events.to_vec();
        corrupt.swap(env, position("helper_joined,0,"));
        reject(&corrupt, "freeing the environment before helper join");
    }
    if schedule.split_admission() {
        let (&replacement, _) = futures
            .iter()
            .find(|(_, pair)| **pair == (102, "1"))
            .unwrap();
        let mut corrupt = events.to_vec();
        if schedule == BatchSchedule::TogetherDeferred {
            let settle = position(&format!("direct_settle,{replacement},"));
            let decision = if controlled { 0 } else { 2 };
            corrupt[settle] = format!("direct_settle,{replacement},{decision},0");
            reject(&corrupt, "losing the Deferred drain's exact decision");
        } else {
            let retry = position(&format!("drain_receipt,{replacement},"));
            let (&poll, _) = operations
                .iter()
                .find(|(_, future)| futures[*future].0 == 200)
                .unwrap();
            let settle = position(&format!("owner_settle,{poll},"));
            assert!(settle < retry);
            let premature = corrupt.remove(retry);
            corrupt.insert(settle, premature);
            reject(
                &corrupt,
                "retrying before polling returns its mailbox credit",
            );
        }
    }

    assert!(replay_batched(binding, schedule, controlled, events, 1).is_err());
    let order: Vec<_> = direct
        .iter()
        .map(|((g, _), _, _)| *g)
        .chain(collected.iter().map(|((g, _), _, _)| *g))
        .collect();
    println!(
        "BATCH TRACE {binding:?} {schedule:?} controlled={controlled} leader={leader} order={:?} events={} frontier={} rejected={}",
        order,
        report.events,
        report.maximum_frontier,
        6 + usize::from(binding == Binding::CJoined) + usize::from(schedule.split_admission())
    );
}
