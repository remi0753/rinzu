use rinzu_runtime_model::native_lifetime::{
    BatchSchedule, Binding, Cohort, CohortFault, FailureKind, explore,
};

fn explore_orders(binding: Binding, schedule: BatchSchedule) {
    for controlled in [false, true] {
        let report = explore(Cohort::batched(binding, schedule, controlled), 1_000_000)
            .unwrap_or_else(|problem| panic!("{problem}"));
        assert_eq!(report.allowed_stalls, 0);
        for witness in [
            "duplicate-released-with-old-and-batch-references-live",
            "new-call-reuses-storage-with-old-sdk-reference",
        ] {
            assert!(report.witnesses.contains(witness), "{witness}");
        }
        assert_eq!(
            report
                .witnesses
                .contains("first-member-retired-before-second-exchange"),
            schedule == BatchSchedule::Staggered,
        );
        assert_eq!(
            report
                .witnesses
                .contains("replacement-receipt-settled-first"),
            matches!(
                schedule,
                BatchSchedule::Staggered | BatchSchedule::TogetherDeferred
            ),
        );
        assert_eq!(
            report.witnesses.contains("peer-receipt-settled-first"),
            schedule != BatchSchedule::TogetherDeferred,
        );
        if schedule.split_admission() {
            assert!(
                report
                    .witnesses
                    .contains(if schedule == BatchSchedule::TogetherDeferred {
                        "replacement-settled-deferred-receipt-without-mailbox-credit"
                    } else {
                        "replacement-retried-deferred-receipt-before-batch"
                    })
            );
        }
        println!(
            "BATCH MODEL {binding:?} {schedule:?} controlled={controlled} states={} edges={} terminals={} depth={} witnesses={:?}",
            report.states,
            report.transitions,
            report.terminals,
            report.maximum_shortest_depth,
            report.witnesses,
        );
    }
}

#[test]
fn c_joined_staggered() {
    explore_orders(Binding::CJoined, BatchSchedule::Staggered);
}

#[test]
fn c_joined_together() {
    explore_orders(Binding::CJoined, BatchSchedule::Together);
}

#[test]
fn rust_staggered() {
    explore_orders(Binding::RustChannel, BatchSchedule::Staggered);
}

#[test]
fn rust_together() {
    explore_orders(Binding::RustChannel, BatchSchedule::Together);
}

#[test]
fn rust_together_with_deferred_collection() {
    explore_orders(Binding::RustChannel, BatchSchedule::TogetherDeferred);
}

#[test]
fn rust_together_with_readmission() {
    explore_orders(Binding::RustChannel, BatchSchedule::TogetherRetry);
}

#[test]
fn detects_batch_faults_and_incomplete_exploration() {
    let initial = Cohort::batched(Binding::CJoined, BatchSchedule::Staggered, true);
    for fault in [
        CohortFault::BatchNotifyBeforeDuplicateRelease,
        CohortFault::BatchReleaseConsumesOldReference,
    ] {
        let mut model = initial.clone();
        model.fault = fault;
        let failure = explore(model, 250_000).expect_err("missed batch fault");
        assert_eq!(failure.kind, FailureKind::Invariant, "{failure}");
        assert!(failure.reason.starts_with("NL-I7"), "{failure}");
        println!("BATCH MUTANT {fault:?} {}", failure.reason);
    }
    let limited = explore(initial, 1).expect_err("a partial search passed");
    assert_eq!(limited.kind, FailureKind::StateLimit);
    for (schedule, fault) in [
        (
            BatchSchedule::TogetherDeferred,
            CohortFault::DirectLosesDecision,
        ),
        (
            BatchSchedule::TogetherRetry,
            CohortFault::RetryDropsDecision,
        ),
    ] {
        let mut model = Cohort::batched(Binding::RustChannel, schedule, true);
        model.fault = fault;
        let failure = explore(model, 250_000).expect_err("lost the Deferred decision");
        assert_eq!(failure.kind, FailureKind::Invariant, "{failure}");
        assert!(failure.reason.starts_with("NL-I1"), "{failure}");
        println!("BATCH MUTANT {fault:?} {}", failure.reason);
    }
}
