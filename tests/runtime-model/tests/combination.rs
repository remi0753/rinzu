use rinzu_runtime_model::native_lifetime::{Binding, Cohort, CohortMode, Protocol, explore};
#[test]
fn exhausts_early_deferred_and_joined_multiple_call_products() {
    for (binding, mode) in [
        (Binding::CDirect, CohortMode::Early),
        (Binding::RustChannel, CohortMode::Early),
        (Binding::RustChannel, CohortMode::Deferred),
        (Binding::CJoined, CohortMode::Early),
    ] {
        for controlled in [false, true] {
            let r = explore(Cohort::combination(binding, mode, controlled), 1_000_000)
                .unwrap_or_else(|e| panic!("{e}"));
            println!(
                "COMBINATION {binding:?} {mode:?} controlled={controlled} states={} edges={} terminals={} depth={} witnesses={:?}",
                r.states, r.transitions, r.terminals, r.maximum_shortest_depth, r.witnesses
            );
            assert!(r.terminals > 0);
            assert_eq!(r.allowed_stalls, 0);
            let w = if mode == CohortMode::Deferred {
                "two-direct-deferred-copies-without-credit"
            } else {
                "sdk-published-before-first-admission"
            };
            assert!(r.witnesses.contains(w), "{w}");
            if mode == CohortMode::Deferred && controlled {
                assert!(
                    r.witnesses
                        .contains("direct-cancel-and-timeout-preserved-independently")
                );
            }
            if binding == Binding::CJoined {
                assert!(
                    r.witnesses
                        .contains("polling-call-drained-with-notified-calls")
                );
                assert!(
                    r.witnesses
                        .contains("all-records-released-before-helper-wake-return")
                );
            }
        }
    }
}
#[test]
fn combination_state_limit_is_incomplete() {
    use rinzu_runtime_model::native_lifetime::{CohortLayout, replay_cohort};
    assert!(replay_cohort(Binding::CJoined, CohortLayout::Carriers, false, &[], 10).is_err());
    assert!(
        explore(
            Cohort::combination(Binding::RustChannel, CohortMode::Deferred, true),
            1
        )
        .is_err()
    );
}
#[test]
fn combination_mutants_expose_direct_and_helper_lifetime_errors() {
    use rinzu_runtime_model::native_lifetime::{CohortAction as H, CohortFault};
    use std::collections::HashSet;
    for (fault, binding, mode, invariant) in [
        (
            CohortFault::PollRecordBeforeDrain,
            Binding::CJoined,
            CohortMode::Early,
            "NL-I9",
        ),
        (
            CohortFault::CandidateConsumesProducer,
            Binding::CDirect,
            CohortMode::Early,
            "NL-I7",
        ),
        (
            CohortFault::DirectLosesDecision,
            Binding::RustChannel,
            CohortMode::Deferred,
            "NL-I1",
        ),
        (
            CohortFault::DirectInventsReceipt,
            Binding::RustChannel,
            CohortMode::Deferred,
            "NL-I9",
        ),
        (
            CohortFault::PollRetireBeforeCopy,
            Binding::CJoined,
            CohortMode::Early,
            "NL-I4",
        ),
        (
            CohortFault::DetachDuringHelperWake,
            Binding::CJoined,
            CohortMode::Early,
            "NL-I5",
        ),
        (
            CohortFault::JoinDuringHelperWake,
            Binding::CJoined,
            CohortMode::Early,
            "NL-I5",
        ),
    ] {
        let mut initial = Cohort::combination(binding, mode, true);
        initial.fault = fault;
        let mut seen = HashSet::from([initial.clone()]);
        let mut stack = vec![(initial, Vec::new())];
        let mut found = false;
        'search: while let Some((s, path)) = stack.pop() {
            let mut actions = s.actions();
            actions.sort_by_key(|a| match a {
                H::TargetFree => 0,
                H::ReleaseBegin(_)
                | H::Decrement(_)
                | H::ReleaseEnd(_)
                | H::PublishEnd(_)
                | H::HelperWakeEnd => 2,
                _ => 1,
            });
            for a in actions.into_iter().rev() {
                let mut n = s.clone();
                n.apply(a);
                let mut trace = path.clone();
                trace.push(format!("{a:?}"));
                if let Err(v) = s.check_step(a, &n).and_then(|()| n.check()) {
                    assert_eq!(v.invariant, invariant, "{fault:?}: {v:?}");
                    println!("COMBINATION MUTANT {fault:?} {}", trace.join(" -> "));
                    found = true;
                    break 'search;
                }
                if !n.terminal() && seen.insert(n.clone()) {
                    assert!(seen.len() < 100_000, "mutant search incomplete");
                    stack.push((n, trace));
                }
            }
        }
        assert!(found, "missing {fault:?} counterexample");
    }
}
