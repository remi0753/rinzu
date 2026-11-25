use rinzu_runtime_model::native_lifetime::{FailureKind, Joined, JoinedFault, explore};
#[test]
fn exhausts_notified_and_polling_receipts_with_real_helper_join() {
    for count in [1, 2] {
        let r = explore(Joined::new(count), 5_000_000).unwrap_or_else(|e| panic!("{e}"));
        println!(
            "{} states={} edges={} terminals={} depth={}",
            r.profile, r.states, r.transitions, r.terminals, r.maximum_shortest_depth
        );
        assert!(r.terminals > 0);
        assert_eq!(r.allowed_stalls, 0);
        for w in [
            "helper-and-sdk-context-readers-overlap",
            "polling-record-released-during-helper-wake",
            "shutdown-waits-for-helper-wake",
            "completion-claimed-before-physical-publication",
        ] {
            assert!(r.witnesses.contains(w), "missing {w}");
        }
    }
}
#[test]
fn rejects_join_and_auxiliary_retirement_faults() {
    for (fault, id) in [
        (JoinedFault::DetachWithoutJoin, "NL-I6"),
        (JoinedFault::FreeDuringHelperWake, "NL-I5"),
        (JoinedFault::RetireAuxEarly, "NL-I4"),
        (JoinedFault::JoinDuringHelperWake, "NL-I5"),
    ] {
        let mut s = Joined::new(1);
        s.fault = fault;
        let e = explore(s, 5_000_000).expect_err("unsafe joined lifetime passed");
        assert_eq!(e.kind, FailureKind::Invariant, "{e}");
        assert!(e.reason.starts_with(id), "{e}");
        println!(
            "JOINED MUTANT {fault:?} {} {}",
            e.reason,
            e.trace.join(" -> ")
        );
    }
}
