use rinzu_runtime_model::native_lifetime::{
    Binding, FailureKind, Handoff, HandoffFault, Protocol, explore,
};
use std::collections::BTreeSet;

#[test]
fn exhausts_call_admission_completion_wake_product() {
    let mut witnesses = BTreeSet::new();
    for (binding, full) in [
        (Binding::CDirect, false),
        (Binding::RustChannel, false),
        (Binding::RustChannel, true),
    ] {
        let r = explore(Handoff::new(binding, full), 5_000_000).unwrap_or_else(|e| panic!("{e}"));
        println!(
            "{} states={} edges={} terminals={} depth={} witnesses={:?}",
            r.profile, r.states, r.transitions, r.terminals, r.maximum_shortest_depth, r.witnesses
        );
        assert!(r.terminals > 0);
        assert_eq!(r.allowed_stalls, 0);
        witnesses.extend(r.witnesses);
    }
    for w in [
        "sdk-publication-before-call-return",
        "call-environment-released-sdk-pin-retained",
        "call-close-waits-for-callback",
        "controlled-deferred-sdk-still-running",
        "deferred-control-transferred-before-reactor-publication",
        "direct-deferred-drain-without-normal-credit",
    ] {
        assert!(witnesses.contains(w), "unreached {w}");
    }
}

#[test]
fn handoff_interface_mutations_have_counterexamples() {
    for (fault, id) in [
        (HandoffFault::ReturnPublishesSdk, "NL-I3"),
        (HandoffFault::LoseDeferredDrain, "NL-I9"),
        (HandoffFault::RetryLosesDecision, "NL-I1"),
        (HandoffFault::ReleasePinWithCall, "NL-I5"),
        (HandoffFault::DisposeOnCancel, "NL-I5"),
        (HandoffFault::AdmitBeforeRead, "NL-I9"),
    ] {
        let mut m = Handoff::new(Binding::RustChannel, true);
        m.fault = fault;
        let e = explore(m, 1_000_000).expect_err("mutant survived");
        assert_eq!(e.kind, FailureKind::Invariant, "{e}");
        assert!(e.reason.starts_with(id), "{e}");
        println!("HANDOFF MUTANT {fault:?} {}", e.trace.join(" -> "));
    }
    let e = explore(Handoff::new(Binding::CDirect, false), 1).unwrap_err();
    assert_eq!(e.kind, FailureKind::StateLimit);
    assert!(!Handoff::new(Binding::CDirect, false).terminal());
}
