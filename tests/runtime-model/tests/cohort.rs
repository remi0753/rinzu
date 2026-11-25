use rinzu_runtime_model::native_lifetime::{
    Binding, Cohort, CohortAction as H, CohortFault, CohortLayout, FailureKind, Protocol, explore,
};
#[test]
fn exhausts_multiple_calls_receipts_and_shared_generations() {
    for binding in [Binding::CDirect, Binding::RustChannel] {
        for layout in [CohortLayout::Carriers, CohortLayout::Reuse] {
            for controlled in [false, true] {
                let r = explore(Cohort::new(binding, layout, controlled), 2_000_000)
                    .unwrap_or_else(|e| panic!("{e}"));
                println!(
                    "{} states={} edges={} terminals={} depth={} witnesses={:?}",
                    r.profile,
                    r.states,
                    r.transitions,
                    r.terminals,
                    r.maximum_shortest_depth,
                    r.witnesses
                );
                assert!(r.terminals > 0);
                assert_eq!(r.allowed_stalls, 0);
                for w in [
                    "shared-references-with-duplicate-receipt-registration",
                    "second-call-started-before-first-sdk-drain",
                    "multiple-producers-outlive-all-call-and-wake-environments",
                ] {
                    assert!(r.witnesses.contains(w), "{} missing {w}", r.profile);
                }
                if layout == CohortLayout::Reuse {
                    assert!(r.witnesses.contains("five-shared-target-references"));
                    assert!(
                        r.witnesses
                            .contains("new-call-reuses-storage-with-old-sdk-reference")
                    );
                }
                if binding == Binding::RustChannel {
                    assert!(
                        r.witnesses
                            .contains("multiple-receipts-share-deferred-credit-pressure")
                    );
                }
                if controlled {
                    assert!(
                        r.witnesses
                            .contains("receipt-decisions-independent-on-one-carrier")
                    );
                }
            }
        }
    }
}
#[test]
fn cohort_faults_produce_cross_generation_counterexamples() {
    for (fault, id) in [
        (CohortFault::FreeWithOtherProducer, "NL-I7"),
        (CohortFault::ReuseBeforeAllCopies, "NL-I4"),
        (CohortFault::ReuseConsumesOldReference, "NL-I7"),
        (CohortFault::CandidateConsumesProducer, "NL-I7"),
        (CohortFault::DetachAfterOneGeneration, "NL-I5"),
        (CohortFault::CallReleaseDropsPin, "NL-I5"),
        (CohortFault::OldControlChangesReplacement, "NL-I8"),
        (CohortFault::OldAliasReadsReplacement, "NL-I8"),
        (CohortFault::RetryDropsDecision, "NL-I1"),
        (CohortFault::SecondReceiptTransfersAgain, "NL-I7"),
    ] {
        let mut m = Cohort::new(Binding::RustChannel, CohortLayout::Reuse, true);
        m.fault = fault;
        let (reason, trace) = find_fault(m);
        assert!(reason.starts_with(id), "{reason}");
        println!("COHORT MUTANT {fault:?} {}", trace.join(" -> "));
    }
    let mut gate = Cohort::new(Binding::CDirect, CohortLayout::Reuse, true);
    gate.fault = CohortFault::OverlapDetachedNotifications;
    let (reason, trace) = find_fault(gate);
    assert!(reason.starts_with("NL-I5"), "{reason}");
    println!(
        "COHORT MUTANT OverlapDetachedNotifications {}",
        trace.join(" -> ")
    );
    assert_eq!(
        explore(
            Cohort::new(Binding::CDirect, CohortLayout::Carriers, false),
            1
        )
        .unwrap_err()
        .kind,
        FailureKind::StateLimit
    );
}

// Finding a concrete mutant counterexample does not require exhausting the safe
// prefix. Preserve every enabled edge, prioritize SDK decrements last, and fail
// explicitly if the finite counterexample search exceeds its resource bound.
fn find_fault(initial: Cohort) -> (String, Vec<String>) {
    use std::collections::HashSet;
    let mut seen = HashSet::from([initial.clone()]);
    let mut stack = vec![(initial, Vec::new())];
    while let Some((state, trace)) = stack.pop() {
        let mut actions = state.actions();
        actions.sort_by_key(|a| match a {
            H::TargetFree => 0,
            H::ReleaseBegin(_) | H::Decrement(_) | H::ReleaseEnd(_) | H::PublishEnd(_) => 2,
            _ => 1,
        });
        for action in actions.into_iter().rev() {
            let mut next = state.clone();
            next.apply(action);
            let mut path = trace.clone();
            path.push(format!("{action:?}"));
            if let Err(v) = state.check_step(action, &next).and_then(|()| next.check()) {
                return (format!("{}: {}", v.invariant, v.detail), path);
            }
            if !next.terminal() && seen.insert(next.clone()) {
                assert!(seen.len() < 100_000, "mutant search incomplete");
                stack.push((next, path));
            }
        }
    }
    panic!("mutant has no counterexample");
}
