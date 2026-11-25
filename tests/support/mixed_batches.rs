use rinzu_runtime_model::native_lifetime::{Binding, MixedBatchSchedule, replay_mixed_batches};

pub fn check(
    binding: Binding,
    schedule: MixedBatchSchedule,
    controlled: bool,
    leaders: [u64; 2],
    reverse_release: bool,
    events: &[String],
) {
    let replay =
        |lines: &[String]| replay_mixed_batches(binding, schedule, controlled, lines, 100_000);
    let report = replay(events).unwrap_or_else(|error| panic!("{error}\n{}", events.join("\n")));
    let position = |event: &str| {
        events
            .iter()
            .position(|line| line.starts_with(event))
            .unwrap_or_else(|| panic!("missing {event}\n{}", events.join("\n")))
    };
    let index = |kind: &str, token: u64| position(&format!("{kind},{token},"));
    let duplicate = 100 + ((leaders[1] - 100) ^ 1);
    let second_merge = index("merge_wake", duplicate);
    let bound_id = |kind: &str, generation: u64| {
        let id = generation.to_string();
        events
            .iter()
            .find_map(|row| {
                let f: Vec<_> = row.split(',').collect();
                (f[0] == kind && f[2] == id).then(|| f[1].parse::<u64>().unwrap())
            })
            .unwrap()
    };
    let old_record = bound_id("receipt", 101);
    if schedule == MixedBatchSchedule::Together {
        assert!(second_merge < index("owner_settle", old_record));
    }
    if schedule == MixedBatchSchedule::LateArrival {
        let selection = index("ready_selection", bound_id("future_bind", 103));
        let older_physical = index("physical", bound_id("receipt", 102));
        let younger_settle = index("owner_settle", bound_id("receipt", 103));
        assert!(selection < older_physical && older_physical < younger_settle);
        assert!(younger_settle < index("owner_settle", bound_id("receipt", 102)));
    }
    let environment = index("env_free", 0);
    let first_return = index("producer_released", leaders[0]);
    let second_return = index("producer_released", leaders[1]);
    assert!(environment < first_return.min(second_return));
    assert_eq!(first_return > second_return, reverse_release);

    // A foreign batch member, a different native target and an incorrect
    // header must fail even though all physical events remain in place.
    let unit_abi = index("carrier_abi", 101);
    let target = index("batch_target", 103);
    let mut mutations = Vec::new();
    if schedule == MixedBatchSchedule::LateArrival {
        let mut corrupt = events.to_vec();
        corrupt.remove(index("ready_selection", bound_id("future_bind", 103)));
        mutations.push(corrupt);
    }
    let mut corrupt = events.to_vec();
    let merge_target = events[second_merge].rsplit(',').next().unwrap();
    corrupt[second_merge] = format!("merge_wake,{duplicate},101,{merge_target}");
    mutations.push(corrupt);
    let mut corrupt = events.to_vec();
    corrupt[unit_abi] = "carrier_abi,101,2,32".into();
    mutations.push(corrupt);
    let mut corrupt = events.to_vec();
    corrupt[target] = "batch_target,103,8,0".into();
    mutations.push(corrupt);
    let mut corrupt = events.to_vec();
    corrupt.remove(unit_abi);
    mutations.push(corrupt);
    let mut corrupt = events.to_vec();
    if binding == Binding::RustChannel {
        corrupt.insert(
            index("completion_claim", old_record),
            format!("copied,{old_record},29,0"),
        );
    } else {
        corrupt[index("copied", old_record)] = format!("copied,{old_record},29,0");
    }
    mutations.push(corrupt);
    let mut corrupt = events.to_vec();
    let abi = index("generation", 200);
    let fields: Vec<_> = events[abi].split(',').collect();
    let address: i64 = fields[2].parse().unwrap();
    corrupt[abi] = format!("generation,200,{},4", address - 4);
    mutations.push(corrupt);
    let rejected = mutations.len();
    for (case, corrupt) in mutations.into_iter().enumerate() {
        assert!(
            replay(&corrupt).is_err(),
            "accepted mixed ABI trace mutation {case}"
        );
    }
    println!(
        "MIXED BATCH TRACE {binding:?} {schedule:?} leaders={leaders:?} controlled={controlled} reverse={reverse_release} events={} frontier={} rejected={rejected}",
        report.events, report.maximum_frontier,
    );
}
