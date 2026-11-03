use rinzu_runtime::failure::{
    Diagnostic, ErrorKind as E, FailureCause as C, FailureStateError, PendingFailure,
    SourcePosition,
};

#[test]
fn propagating_a_completed_failure_preserves_all_diagnostics_and_cleanup_primary() {
    let mut child = PendingFailure::default();
    child.fail(Diagnostic {
        cause: C::Error(E::Conversion),
        source: Some(SourcePosition {
            file: 3,
            start_byte: 4,
            end_byte: 9,
        }),
    });
    child.begin_cleanup();
    child.fail(C::Error(E::InvalidUtf8));
    child.end_cleanup().unwrap();
    let child = child.finish().unwrap().unwrap();
    let mut caller = PendingFailure::default();
    caller.propagate(child.clone());
    caller.propagate(rinzu_runtime::failure::Failure::new(C::RuntimePanic));
    assert_eq!(caller.observe_error(), Some(child.clone()));
    caller.fail(C::Cancelled);
    caller.begin_cleanup();
    caller.propagate(child.clone());
    caller.end_cleanup().unwrap();
    let aggregated = caller.finish().unwrap().unwrap();
    assert_eq!(aggregated.primary.cause, C::Cancelled);
    assert_eq!(aggregated.cleanup, [child.primary, child.cleanup[0]]);
    assert!(!aggregated.recoverable());
}

#[test]
fn cleanup_diagnostics_preserve_primary_source_and_execution_order() {
    let source = SourcePosition {
        file: 42,
        start_byte: 10,
        end_byte: 20,
    };
    let mut pending = PendingFailure::default();
    pending.fail(Diagnostic {
        cause: C::Error(E::Conversion),
        source: Some(source),
    });
    pending.begin_cleanup();
    assert!(pending.current().is_none());
    pending.fail(C::Error(E::NullNativeValue));
    pending.begin_cleanup();
    pending.fail(C::Error(E::InvalidUtf8));
    pending.end_cleanup().unwrap();
    pending.end_cleanup().unwrap();
    pending.begin_cleanup();
    pending.fail(C::Error(E::NativeRecordBusy));
    pending.end_cleanup().unwrap();
    let observed = pending.observe_error().unwrap();
    assert_eq!(observed.primary.source, Some(source));
    assert_eq!(observed.primary.cause, C::Error(E::Conversion));
    assert_eq!(
        observed
            .cleanup
            .iter()
            .map(|diagnostic| diagnostic.cause)
            .collect::<Vec<_>>(),
        [
            C::Error(E::NullNativeValue),
            C::Error(E::InvalidUtf8),
            C::Error(E::NativeRecordBusy)
        ]
    );
    assert!(pending.finish().unwrap().is_none());
}

#[test]
fn aggregation_cannot_make_traps_or_cancellation_catchable() {
    for primary in [
        C::ArithmeticTrap,
        C::Cancelled,
        C::Shutdown,
        C::RuntimePanic,
        C::Error(E::Conversion),
    ] {
        let mut pending = PendingFailure::default();
        pending.fail(primary);
        pending.begin_cleanup();
        pending.fail(C::ArithmeticTrap);
        pending.end_cleanup().unwrap();
        assert!(pending.observe_error().is_none());
        let failure = pending.finish().unwrap().unwrap();
        assert_eq!(failure.primary.cause, primary);
        assert_eq!(failure.cleanup[0].cause, C::ArithmeticTrap);
        assert!(!failure.recoverable());
    }
}

#[test]
fn scoped_cleanup_settles_once_and_never_pops_the_same_region_in_a_caller() {
    let mut pending = PendingFailure::default();
    pending.fail(C::Cancelled);
    pending.begin_cleanup_scoped(1, 7);
    pending.fail(C::Error(E::Conversion));
    pending.begin_cleanup_scoped(2, 7);
    pending.fail(C::Error(E::InvalidUtf8));
    assert_eq!(
        pending.end_cleanup_scoped(1, 7),
        Err(FailureStateError::UnbalancedCleanup)
    );
    assert_eq!(pending.end_cleanup_scoped(2, 7), Ok(true));
    assert_eq!(pending.end_cleanup_scoped(2, 7), Ok(false));
    assert_eq!(pending.cleanup_depth(), 1);
    assert_eq!(pending.end_cleanup_scoped(1, 7), Ok(true));
    assert_eq!(pending.end_cleanup_scoped(1, 7), Ok(false));
    let failure = pending.finish().unwrap().unwrap();
    assert_eq!(failure.primary.cause, C::Cancelled);
    assert_eq!(
        failure
            .cleanup
            .iter()
            .map(|item| item.cause)
            .collect::<Vec<_>>(),
        [C::Error(E::Conversion), C::Error(E::InvalidUtf8)]
    );
}

#[test]
fn a_local_catch_in_finally_observes_only_its_new_error() {
    let mut pending = PendingFailure::default();
    pending.fail(C::Cancelled);
    pending.begin_cleanup();
    pending.fail(C::Error(E::Conversion));
    let local = pending.observe_error().unwrap();
    assert_eq!(local.primary.cause, C::Error(E::Conversion));
    pending.end_cleanup().unwrap();
    assert!(pending.observe_error().is_none());
    let original = pending.finish().unwrap().unwrap();
    assert_eq!(original.primary.cause, C::Cancelled);
    assert!(original.cleanup.is_empty());
}

#[test]
fn normal_cleanup_failure_propagates_and_unbalanced_regions_are_rejected() {
    let mut pending = PendingFailure::default();
    assert_eq!(
        pending.end_cleanup(),
        Err(FailureStateError::UnbalancedCleanup)
    );
    pending.begin_cleanup();
    pending.fail(C::Error(E::InvalidUtf8));
    pending.end_cleanup().unwrap();
    assert_eq!(
        pending.observe_error().unwrap().primary.cause,
        C::Error(E::InvalidUtf8)
    );
    pending.begin_cleanup();
    assert_eq!(pending.finish(), Err(FailureStateError::ActiveCleanup));
}
