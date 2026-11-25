use rinzu_runtime_model::native_lifetime::{Binding, Cohort, CohortMode, Protocol, explore};
#[test]
fn exhausts_reuse_with_early_callback_split_admission_and_polling() {
    for binding in [Binding::CDirect, Binding::CJoined, Binding::RustChannel] {
        for mode in [CohortMode::Early, CohortMode::Deferred, CohortMode::Mixed] {
            if binding != Binding::RustChannel && mode != CohortMode::Early {
                continue;
            }
            for polling in [false, true] {
                if binding == Binding::CDirect && polling || binding == Binding::CJoined && !polling
                {
                    continue;
                }
                for controlled in [false, true] {
                    let r = explore(
                        Cohort::extended(binding, mode, controlled, polling),
                        500_000,
                    )
                    .unwrap_or_else(|e| panic!("{e}"));
                    println!(
                        "EXTENDED {binding:?} {mode:?} poll={polling} control={controlled} states={} edges={} terminals={} depth={} witnesses={:?}",
                        r.states, r.transitions, r.terminals, r.maximum_shortest_depth, r.witnesses
                    );
                    assert!(r.terminals > 0);
                    assert_eq!(r.allowed_stalls, 0);
                    assert!(
                        r.witnesses
                            .contains("new-call-reuses-storage-with-old-sdk-reference")
                    );
                    if mode == CohortMode::Early {
                        assert!(
                            r.witnesses
                                .contains("sdk-publishes-with-callback-environment-live")
                        );
                    }
                    if mode == CohortMode::Mixed {
                        assert!(
                            r.witnesses
                                .contains("split-admission-direct-copy-with-real-peer-receipt")
                        );
                        assert!(r.witnesses.contains("five-shared-target-references"));
                    }
                    if polling && binding == Binding::RustChannel {
                        assert!(
                            r.witnesses
                                .contains("rust-polling-call-drained-in-reuse-product")
                        );
                    }
                    if binding == Binding::CJoined {
                        assert!(
                            r.witnesses
                                .contains("all-records-released-before-helper-wake-return")
                        );
                    }
                }
            }
        }
    }
}
#[test]
fn extended_limit_never_means_success() {
    assert!(
        explore(
            Cohort::extended(Binding::RustChannel, CohortMode::Mixed, true, true),
            1
        )
        .is_err()
    );
}
#[test]
fn extended_mutants_expose_cross_generation_and_callback_errors() {
    use rinzu_runtime_model::native_lifetime::{CohortAction as H, CohortFault as F};
    use std::collections::HashSet;
    for (fault, mode, polling, invariant) in [
        (
            F::SdkDoneDropsCallEnvironment,
            CohortMode::Early,
            false,
            "NL-I5",
        ),
        (
            F::PollRecordDuringWorkerWake,
            CohortMode::Mixed,
            true,
            "NL-I5",
        ),
        (
            F::ReuseConsumesOldReference,
            CohortMode::Mixed,
            true,
            "NL-I7",
        ),
        (
            F::CandidateConsumesProducer,
            CohortMode::Mixed,
            false,
            "NL-I7",
        ),
        (F::DirectLosesDecision, CohortMode::Mixed, false, "NL-I1"),
        (F::DirectInventsReceipt, CohortMode::Mixed, false, "NL-I9"),
        (
            F::OldControlChangesReplacement,
            CohortMode::Deferred,
            true,
            "NL-I8",
        ),
        (
            F::OldAliasReadsReplacement,
            CohortMode::Early,
            true,
            "NL-I8",
        ),
    ] {
        let mut initial = Cohort::extended(Binding::RustChannel, mode, true, polling);
        initial.fault = fault;
        let mut stack = vec![(initial.clone(), Vec::new())];
        let mut seen = HashSet::from([initial]);
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
                    println!("EXTENDED MUTANT {fault:?} {}", trace.join(" -> "));
                    found = true;
                    break 'search;
                }
                if !n.terminal() && seen.insert(n.clone()) {
                    assert!(seen.len() < 100_000, "mutant search incomplete");
                    stack.push((n, trace));
                }
            }
        }
        assert!(found, "{fault:?} has no counterexample");
    }
}
