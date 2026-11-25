use rinzu_runtime_model::native_lifetime::{Binding, Cohort, CohortFault, FailureKind, explore};

#[test]
fn explores_every_recovery_order_with_a_polling_call() {
    for (binding, controlled) in [
        (Binding::CJoined, false),
        (Binding::RustChannel, false),
        (Binding::RustChannel, true),
    ] {
        let report = explore(Cohort::unordered(binding, controlled), 1_000_000)
            .unwrap_or_else(|counterexample| panic!("{counterexample}"));
        println!(
            "UNORDERED MODEL {binding:?} controlled={controlled} states={} edges={} terminals={} depth={} witnesses={:?}",
            report.states,
            report.transitions,
            report.terminals,
            report.maximum_shortest_depth,
            report.witnesses
        );
        assert_eq!(report.allowed_stalls, 0);
        for order in [
            "recovered-first-second-poll",
            "recovered-first-poll-second",
            "recovered-second-first-poll",
            "recovered-second-poll-first",
            "recovered-poll-first-second",
            "recovered-poll-second-first",
        ] {
            assert!(report.witnesses.contains(order), "{binding:?}: {order}");
        }
        if binding == Binding::RustChannel {
            for path in [
                "deferred-collected-while-full",
                "retried-with-registered-target",
                "retry-rolled-back-against-done",
            ] {
                assert!(report.witnesses.contains(path), "{path}");
            }
        }
    }
}

#[test]
fn rejects_faults_in_credit_recovery_and_physical_lifetime() {
    for (fault, invariant) in [
        (CohortFault::RetryWithoutCredit, "NL-I9"),
        (CohortFault::RetryDropsDecision, "NL-I1"),
        (CohortFault::DirectLosesDecision, "NL-I1"),
        (CohortFault::PollSettleBeforePhysical, "NL-I2"),
        (CohortFault::PollRecordDuringWorkerWake, "NL-I5"),
    ] {
        let mut model = Cohort::unordered(Binding::RustChannel, true);
        model.fault = fault;
        let counterexample = explore(model, 100_000).expect_err("fault escaped the model");
        assert_eq!(
            counterexample.kind,
            FailureKind::Invariant,
            "{counterexample}"
        );
        assert!(
            counterexample.reason.starts_with(invariant),
            "{counterexample}"
        );
        println!("UNORDERED MUTANT {fault:?}: {}", counterexample.reason);
    }
    let limited = explore(Cohort::unordered(Binding::CJoined, false), 1)
        .expect_err("an incomplete exploration cannot pass");
    assert_eq!(limited.kind, FailureKind::StateLimit);
}
