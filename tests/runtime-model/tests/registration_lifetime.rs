use rinzu_runtime_model::native_lifetime::{
    Binding, FailureKind, Registration, RegistrationFault, explore,
};

#[test]
fn exhausts_registration_publication_and_teardown_races() {
    for binding in [Binding::CDirect, Binding::CJoined, Binding::RustChannel] {
        for count in [1, 2] {
            let report = explore(Registration::new(count, binding), 5_000_000)
                .unwrap_or_else(|e| panic!("{e}"));
            println!(
                "{} states={} edges={} terminals={} depth={}",
                report.profile,
                report.states,
                report.transitions,
                report.terminals,
                report.maximum_shortest_depth
            );
            assert!(report.terminals > 0);
            assert_eq!(report.allowed_stalls, 0);
            for expected in [
                "sdk-published-before-first-registration",
                "registration-success-return-can-follow-sdk-exchange",
                "candidate-loses-to-done",
                "no-notify-after-unregistered-publication",
                "producer-notify-outlives-environment",
            ] {
                assert!(
                    report.witnesses.contains(expected),
                    "{} missing {expected}",
                    report.profile
                );
            }
            if count == 2 {
                assert!(report.witnesses.contains("candidate-loses-to-same-target"));
            }
            if binding == Binding::RustChannel {
                assert!(
                    report
                        .witnesses
                        .contains("producer-release-before-registration-return")
                );
            }
        }
    }
}

#[test]
fn rejects_reference_transfer_and_registration_pin_faults() {
    for (fault, invariant) in [
        (RegistrationFault::TransferWithoutRetain, "NL-I7"),
        (RegistrationFault::LeakFailedCandidate, "NL-I7"),
        (RegistrationFault::DropTransferredReference, "NL-I7"),
        (RegistrationFault::TransferAfterDone, "NL-I7"),
        (RegistrationFault::TransferTwice, "NL-I7"),
        (RegistrationFault::RetireDuringRegistration, "NL-I4"),
        (RegistrationFault::NotifyThroughLockedRuntime, "NL-I7"),
    ] {
        let mut state = Registration::new(2, Binding::CDirect);
        state.fault = fault;
        let error = explore(state, 5_000_000).expect_err("unsafe registration passed");
        assert_eq!(error.kind, FailureKind::Invariant, "{error}");
        assert!(error.reason.starts_with(invariant), "{error}");
        println!(
            "REGISTRATION MUTANT {fault:?} {} {}",
            error.reason,
            error.trace.join(" -> ")
        );
    }
}
