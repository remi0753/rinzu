use rinzu_runtime_model::lifecycle::{
    AdmissionError, ExitInitiator, ExitRequest, LifecycleState, StandaloneLifecycle,
};

#[test]
fn exit_latches_first_initiator_and_allows_reserved_pending_cleanup() {
    let mut lifecycle = StandaloneLifecycle::new(2);
    lifecycle.admit().unwrap();
    lifecycle.admit().unwrap();
    assert_eq!(
        lifecycle.request_exit(ExitInitiator::Task(7), false),
        ExitRequest::Latched
    );
    assert_eq!(
        lifecycle.request_exit(ExitInitiator::Task(8), false),
        ExitRequest::AlreadyLatched
    );
    assert_eq!(lifecycle.admit(), Err(AdmissionError::Closed));

    assert!(lifecycle.begin_cleanup_safe());
    assert!(!lifecycle.settle_initiator());
    assert!(lifecycle.complete_admitted_during_shutdown());
    lifecycle.release_late_callback();
    assert!(lifecycle.complete_cleanup_safe());
    assert!(lifecycle.settle_initiator());
    assert!(lifecycle.complete_admitted_during_shutdown());
    assert!(lifecycle.finish_root_close(Some(7)));

    let snapshot = lifecycle.snapshot();
    assert_eq!(snapshot.state, LifecycleState::Terminated);
    assert_eq!(snapshot.initiator, Some(ExitInitiator::Task(7)));
    assert_eq!(snapshot.general_slots_used, 0);
    assert_eq!(snapshot.cleanup_completions, 1);
    assert_eq!(snapshot.shutdown_completions, 2);
    assert_eq!(snapshot.late_callback_releases, 1);
    assert_eq!(snapshot.self_join_attempts, 0);
}

#[test]
fn owner_sync_exit_is_rejected_and_host_loop_return_can_then_close_root() {
    let mut lifecycle = StandaloneLifecycle::new(1);
    assert_eq!(
        lifecycle.request_exit(ExitInitiator::Task(1), true),
        ExitRequest::RejectedOwnerSync
    );
    assert_eq!(lifecycle.snapshot().state, LifecycleState::Running);
    assert!(lifecycle.snapshot().admission_open);

    assert_eq!(
        lifecycle.request_exit(ExitInitiator::HostLoopReturn, false),
        ExitRequest::Latched
    );
    assert!(lifecycle.settle_initiator());
    assert!(lifecycle.finish_root_close(None));
    assert_eq!(lifecycle.snapshot().state, LifecycleState::Terminated);
}
