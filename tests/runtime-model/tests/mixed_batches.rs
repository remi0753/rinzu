use rinzu_runtime_model::native_lifetime::{
    Binding, Cohort, CohortFault, FailureKind, MixedBatchSchedule, explore,
};

fn verify(binding: Binding, schedule: MixedBatchSchedule) {
    for controlled in [false, true] {
        let report = explore(
            Cohort::mixed_batches(binding, schedule, controlled),
            1_000_000,
        )
        .unwrap_or_else(|problem| panic!("{problem}"));
        assert_eq!(report.allowed_stalls, 0);
        for witness in [
            "first-batch-v3-collected-first",
            "first-batch-leader-returned-first",
            "second-batch-leader-returned-first",
            "new-call-reuses-storage-with-old-sdk-reference",
        ] {
            assert!(report.witnesses.contains(witness), "{witness}");
        }
        for (witness, reachable) in [
            (
                "first-batch-v2-collected-first",
                schedule != MixedBatchSchedule::Together,
            ),
            (
                "second-batch-v2-collected-first",
                schedule != MixedBatchSchedule::Together,
            ),
            (
                "second-batch-v3-collected-first",
                schedule != MixedBatchSchedule::LateArrival,
            ),
            (
                "second-batch-with-previous-receipt-pending",
                schedule != MixedBatchSchedule::LateArrival,
            ),
            (
                "older-receipt-arrived-after-ready-selection",
                schedule == MixedBatchSchedule::LateArrival,
            ),
        ] {
            assert_eq!(report.witnesses.contains(witness), reachable, "{witness}");
        }
        println!(
            "MIXED BATCH MODEL {binding:?} {schedule:?} controlled={controlled} states={} edges={} terminals={} depth={}",
            report.states, report.transitions, report.terminals, report.maximum_shortest_depth,
        );
    }
}

#[test]
fn c_joined_staggered() {
    verify(Binding::CJoined, MixedBatchSchedule::Staggered);
}

#[test]
fn c_joined_together() {
    verify(Binding::CJoined, MixedBatchSchedule::Together);
}

#[test]
fn rust_staggered() {
    verify(Binding::RustChannel, MixedBatchSchedule::Staggered);
}

#[test]
fn rust_together() {
    verify(Binding::RustChannel, MixedBatchSchedule::Together);
}

#[test]
fn ready_selection_survives_late_arrival() {
    for binding in [Binding::CJoined, Binding::RustChannel] {
        verify(binding, MixedBatchSchedule::LateArrival);
        let mut model = Cohort::mixed_batches(binding, MixedBatchSchedule::LateArrival, true);
        model.fault = CohortFault::LateArrivalReplacesSelection;
        let failure = explore(model, 1_000_000).expect_err("late arrival replaced ready selection");
        assert_eq!(failure.kind, FailureKind::Invariant, "{failure}");
        assert!(failure.reason.starts_with("NLS03"), "{failure}");
        println!("MIXED BATCH MUTANT {binding:?} {}", failure.reason);
    }
}

#[test]
fn detects_cross_batch_reference_and_pin_faults() {
    let initial = Cohort::mixed_batches(Binding::CJoined, MixedBatchSchedule::Together, true);
    for (fault, invariant) in [
        (CohortFault::BatchNotifyBeforeDuplicateRelease, "NL-I7"),
        (CohortFault::BatchReleaseConsumesOldReference, "NL-I7"),
        (CohortFault::ReuseBeforeAllCopies, "NL-I4"),
        (CohortFault::ReuseConsumesOldReference, "NL-I7"),
        (CohortFault::DetachDuringHelperWake, "NL-I5"),
    ] {
        let mut model = initial.clone();
        model.fault = fault;
        let failure = explore(model, 1_000_000).expect_err("missed mixed batch fault");
        assert_eq!(failure.kind, FailureKind::Invariant, "{failure}");
        assert!(failure.reason.starts_with(invariant), "{failure}");
        println!("MIXED BATCH MUTANT {fault:?} {}", failure.reason);
    }
}
