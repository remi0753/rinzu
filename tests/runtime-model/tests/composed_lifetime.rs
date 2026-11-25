use rinzu_runtime_model::native_lifetime::{
    Binding, Composed, ComposedFault, FailureKind, explore,
};
use std::collections::BTreeSet;

#[test]
fn exhausts_synchronized_completion_and_wake_profiles() {
    let mut witnesses = BTreeSet::new();
    for binding in [Binding::CDirect, Binding::CJoined, Binding::RustChannel] {
        for count in [1, 2] {
            let r =
                explore(Composed::new(count, binding), 1_000_000).unwrap_or_else(|e| panic!("{e}"));
            println!(
                "{} states={} edges={} terminals={} depth={}",
                r.profile, r.states, r.transitions, r.terminals, r.maximum_shortest_depth
            );
            assert!(r.terminals > 0);
            assert_eq!(r.allowed_stalls, 0);
            witnesses.extend(r.witnesses);
        }
    }
    for expected in [
        "completion-claimed-before-physical-publication",
        "producer-notify-outlives-environment",
        "one-copy-does-not-authorize-detach",
    ] {
        assert!(
            witnesses.contains(expected),
            "missing composed interface case {expected}"
        );
    }
}
#[test]
fn rejects_faults_at_the_component_boundaries() {
    for (fault, binding, id) in [
        (ComposedFault::DetachAfterOneCopy, Binding::CDirect, "NL-I5"),
        (
            ComposedFault::FreeEnvironmentOnCancel,
            Binding::CDirect,
            "NL-I3",
        ),
        (
            ComposedFault::FreeWithProducerReference,
            Binding::CDirect,
            "NL-I7",
        ),
        (
            ComposedFault::IgnoreRuntimeReference,
            Binding::CDirect,
            "NL-I7",
        ),
        (
            ComposedFault::ReleaseDuringWorkerWake,
            Binding::RustChannel,
            "NL-I6",
        ),
    ] {
        let mut s = Composed::new(2, binding);
        s.fault = fault;
        let e = explore(s, 1_000_000).expect_err("unsafe composition passed");
        assert_eq!(e.kind, FailureKind::Invariant, "{e}");
        assert!(e.reason.starts_with(id), "{e}");
        println!(
            "COMPOSED MUTANT {fault:?} {} {}",
            e.reason,
            e.trace.join(" -> ")
        );
    }
}
