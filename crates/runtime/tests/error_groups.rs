use rinzu_runtime::error_groups::{ErrorGroupError, ErrorGroups, aggregate};
use rinzu_runtime::failure::{Diagnostic, ErrorKind, Failure, FailureCause, SourcePosition};

fn error(position: u32) -> Failure {
    Failure::new(Diagnostic {
        cause: FailureCause::Error(ErrorKind::User),
        source: Some(SourcePosition {
            file: 7,
            start_byte: position,
            end_byte: position + 1,
        }),
    })
}

#[test]
fn lexical_groups_preserve_observation_order_nested_cleanup_and_repeated_await_identity() {
    let mut groups = ErrorGroups::default();
    groups.enter(7);
    let mut first = error(10);
    first.append_cleanup(error(11));
    assert_eq!(groups.record(7, Some(42), first), Ok(true));
    assert_eq!(groups.record(7, Some(42), error(99)), Ok(false));
    groups.enter(7); // recursive or nested activations may reuse a source key
    assert_eq!(groups.record(7, Some(42), error(20)), Ok(true));
    let inner = groups.finish(7).unwrap();
    assert_eq!(inner[0].primary.source.unwrap().start_byte, 20);
    assert_eq!(groups.record(7, None, error(30)), Ok(true));
    assert_eq!(groups.record(7, None, error(30)), Ok(true));
    let failures = groups.finish(7).unwrap();
    assert_eq!(groups.depth(), 0);
    assert_eq!(failures.len(), 3);
    let combined = aggregate(failures).unwrap();
    let positions = std::iter::once(combined.primary)
        .chain(combined.cleanup)
        .map(|diagnostic| diagnostic.source.unwrap().start_byte)
        .collect::<Vec<_>>();
    assert_eq!(positions, [10, 11, 30, 30]);
    assert!(aggregate([]).is_none());
}

#[test]
fn control_traps_and_invalid_scope_do_not_modify_collected_normal_errors() {
    let mut groups = ErrorGroups::default();
    groups.enter(1);
    groups.record(1, None, error(1)).unwrap();
    assert_eq!(groups.finish(2), Err(ErrorGroupError::UnbalancedScope));
    assert_eq!(
        groups.record(2, None, error(2)),
        Err(ErrorGroupError::UnbalancedScope)
    );
    for cause in [
        FailureCause::ArithmeticTrap,
        FailureCause::Cancelled,
        FailureCause::Shutdown,
        FailureCause::Overloaded,
        FailureCause::RuntimePanic,
        FailureCause::Panic,
    ] {
        assert_eq!(
            groups.record(1, None, Failure::new(cause)),
            Err(ErrorGroupError::NotNormalError)
        );
    }
    let mut invalid_cleanup = error(2);
    invalid_cleanup.append_cleanup(Failure::new(FailureCause::ArithmeticTrap));
    assert_eq!(
        groups.record(1, None, invalid_cleanup),
        Err(ErrorGroupError::NotNormalError)
    );
    assert_eq!(groups.finish(1).unwrap(), [error(1)]);
    assert_eq!(groups.finish(1), Err(ErrorGroupError::UnbalancedScope));
}

#[test]
fn group_storage_grows_and_observation_sets_are_local_to_each_activation() {
    let mut groups = ErrorGroups::default();
    for _ in 0..3 {
        groups.enter(9);
        for origin in 0..8192 {
            assert!(
                groups
                    .record(9, Some(origin), error(origin as u32))
                    .unwrap()
            );
        }
        for origin in (0..8192).rev() {
            assert!(!groups.record(9, Some(origin), error(99)).unwrap());
        }
        let failures = groups.finish(9).unwrap();
        assert_eq!(failures.len(), 8192);
        assert!(
            failures.iter().enumerate().all(|(index, failure)| failure
                .primary
                .source
                .unwrap()
                .start_byte
                == index as u32)
        );
        assert_eq!(groups.depth(), 0);
    }
}

#[test]
fn cleanup_settlement_dispatches_normal_lists_and_preserves_control_outcomes() {
    use rinzu_runtime::error_groups::settle;
    use rinzu_runtime::failure::{FailureStateError, PendingFailure};
    let (handled, incoming) = settle(vec![error(1), error(2)], Some(error(3)), true);
    assert!(incoming.is_none());
    assert_eq!(handled, [error(1), error(2), error(3)]);
    let (handled, incoming) = settle(vec![error(1), error(2)], None, false);
    assert!(handled.is_empty());
    assert_eq!(incoming.unwrap().cleanup, [error(2).primary]);
    let (handled, incoming) = settle(
        vec![error(1)],
        Some(Failure::new(FailureCause::Cancelled)),
        true,
    );
    assert!(handled.is_empty());
    let control = incoming.unwrap();
    assert_eq!(control.primary.cause, FailureCause::Cancelled);
    assert_eq!(control.cleanup, [error(1).primary]);
    let mut pending = PendingFailure::default();
    assert_eq!(
        pending.replace_cleanup_incoming(None),
        Err(FailureStateError::UnbalancedCleanup)
    );
    pending.propagate(error(10));
    pending.begin_cleanup();
    assert_eq!(
        pending.replace_cleanup_incoming(None).unwrap(),
        Some(error(10))
    );
    assert!(
        pending
            .replace_cleanup_incoming(Some(error(20)))
            .unwrap()
            .is_none()
    );
    pending.end_cleanup().unwrap();
    assert_eq!(pending.finish().unwrap(), Some(error(20)));
}
