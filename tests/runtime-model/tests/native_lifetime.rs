use rinzu_runtime_model::native_lifetime::{
    Admission, AdmissionFault, Call, CallFault, Completion, CompletionFault, Counterexample,
    FailureKind, Wake, WakeFault, explore, verify,
};
use std::collections::BTreeSet;

#[test]
fn exhausts_bounded_native_lifetime_protocols() {
    let reports = verify(1_000_000).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(reports.len(), 15);
    let mut witnesses = BTreeSet::new();
    for r in reports {
        println!(
            "{} states={} edges={} terminals={} permitted-stalls={} depth={} witnesses={:?}",
            r.profile,
            r.states,
            r.transitions,
            r.terminals,
            r.allowed_stalls,
            r.maximum_shortest_depth,
            r.witnesses
        );
        assert!(r.states > 0 && r.transitions > 0);
        if r.profile.ends_with("cooperativefalse") {
            assert_eq!(r.terminals, 0);
            assert!(r.allowed_stalls > 0);
        } else {
            assert!(r.terminals > 0);
            assert_eq!(r.allowed_stalls, 0);
        }
        witnesses.extend(r.witnesses);
    }
    for expected in [
        "controlled-but-physically-running",
        "physical-done-before-helper-return",
        "publication-during-control-access",
        "old-alias-read-after-carrier-reuse",
        "cancelled-call-still-entered",
        "closed-gate-with-admitted-callback",
        "environment-freed-rejection-control-retained",
        "escaped-result-outlives-call-environment",
        "future-alias-outlives-parent-ledger",
        "physical-done-before-wake-tail-return",
        "late-notify-uses-target-without-runtime-context",
        "producer-reference-outlives-runtime",
        "saturated-cancelled-sdk-retains-deferred-drain",
        "deferred-direct-copy-without-runtime-credit",
        "deferred-or-late-admission-uses-returned-credit",
    ] {
        assert!(
            witnesses.contains(expected),
            "unreached interface case: {expected}"
        );
    }
}

fn counterexample(
    result: Result<rinzu_runtime_model::native_lifetime::Report, Counterexample>,
    id: &str,
) -> String {
    let e = result.expect_err("unsafe mutant unexpectedly passed");
    assert_eq!(e.kind, FailureKind::Invariant, "{e}");
    assert!(e.reason.starts_with(id), "{e}");
    assert!(!e.trace.is_empty(), "mutant was already invalid initially");
    println!(
        "MUTANT {id} depth={} {}",
        e.trace.len(),
        e.trace.join(" -> ")
    );
    e.reason
}

#[test]
fn unsafe_mutations_produce_counterexamples_for_all_ten_invariants() {
    let mut covered = BTreeSet::new();
    for (fault, id) in [
        (AdmissionFault::RetryDropsControl, "NL-I1"),
        (AdmissionFault::AcceptSecondControl, "NL-I1"),
        (AdmissionFault::ReleaseRootWithDeferred, "NL-I5"),
    ] {
        let mut model = Admission::new(true);
        model.fault = fault;
        counterexample(explore(model, 1_000_000), id);
    }
    for (fault, id) in [
        (CompletionFault::OverwriteWinner, "NL-I1"),
        (CompletionFault::AcquireWithoutPublication, "NL-I2"),
        (CompletionFault::CancelCompletesPhysical, "NL-I3"),
        (CompletionFault::RetireOnPrefix, "NL-I4"),
        (CompletionFault::RetireAfterOneCopy, "NL-I4"),
        (CompletionFault::SettleBeforeJoin, "NL-I6"),
        (CompletionFault::DoubleDiscard, "NL-I10"),
        (CompletionFault::LoseLateResult, "NL-I10"),
    ] {
        let mut model = Completion::new(3, 2, 1, true, true);
        model.fault = fault;
        counterexample(explore(model, 500_000), id);
        covered.insert(id);
    }
    let mut reused = Completion::new(3, 1, 2, true, true);
    reused.fault = CompletionFault::ReadReusedCarrier;
    counterexample(explore(reused, 500_000), "NL-I8");
    covered.insert("NL-I8");
    for (fault, id) in [
        (CallFault::CancelReleasesEnvironment, "NL-I3"),
        (CallFault::ReleaseWithCallback, "NL-I5"),
        (CallFault::FreeLiveCapability, "NL-I5"),
        (CallFault::LedgerDropsAliasRoot, "NL-I8"),
    ] {
        let mut model = Call::new(1, false);
        model.fault = fault;
        counterexample(explore(model, 500_000), id);
        covered.insert(id);
    }
    for (fault, id) in [
        (WakeFault::DetachOnPhysicalDone, "NL-I6"),
        (WakeFault::DetachDuringNotify, "NL-I7"),
        (WakeFault::FreeRetainedTarget, "NL-I7"),
    ] {
        let mut model = Wake::new();
        model.fault = fault;
        counterexample(explore(model, 10_000), id);
        covered.insert(id);
    }
    let mut admission = Admission::new(true);
    admission.fault = AdmissionFault::LoseDrainOnRejection;
    counterexample(explore(admission, 1_000), "NL-I9");
    covered.insert("NL-I9");
    assert_eq!(covered.len(), 10);
}

#[test]
fn state_limit_is_an_incomplete_verification_not_a_pass() {
    let e = explore(Completion::new(3, 2, 1, true, true), 1).unwrap_err();
    assert_eq!(e.kind, FailureKind::StateLimit);
    assert!(e.reason.contains("incomplete"));
}

#[test]
fn nonpublishing_sdk_preserves_two_physical_receipts() {
    for version in [1, 2, 3] {
        for dedicated in [false, true] {
            let model = Completion::new(version, 2, 1, dedicated, false);
            let r = explore(model.clone(), 500_000).unwrap();
            assert!(r.allowed_stalls > 0, "{r:?}");
            assert_eq!(r.terminals, 0);
            assert!(r.witnesses.contains("controlled-but-physically-running"));
            println!(
                "NONPUBLISHING MODEL v{version} dedicated={dedicated} states={} edges={} stalls={}",
                r.states, r.transitions, r.allowed_stalls
            );
            let mut invalid = model;
            invalid.fault = CompletionFault::CancelCompletesPhysical;
            counterexample(explore(invalid, 500_000), "NL-I3");
        }
    }
}
