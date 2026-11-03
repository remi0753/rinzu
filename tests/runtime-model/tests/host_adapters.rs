use rinzu_runtime_model::host_adapters::{
    AdapterError, FrameTickAdapter, HostAdapter, NestedMode, QueuePostAdapter,
    run_common_conformance, validate_declared_mode,
};

#[test]
fn queue_post_and_frame_tick_pass_the_same_conformance_suite() {
    let queue = run_common_conformance(&mut QueuePostAdapter::default());
    let frame = run_common_conformance(&mut FrameTickAdapter::default());
    assert_eq!(queue, frame);
    assert_eq!(queue.posted, queue.delivered);
    assert_eq!(queue.duplicate_deliveries, 0);
}

#[test]
fn declared_nested_modes_deliver_and_unsupported_modes_fail_explicitly() {
    let mut queue = QueuePostAdapter::default();
    validate_declared_mode(&queue, NestedMode::Modal).unwrap();
    assert_eq!(
        validate_declared_mode(&queue, NestedMode::FrameOverlay),
        Err(AdapterError::UnsupportedAtGeneration(
            NestedMode::FrameOverlay
        ))
    );
    queue.activate(1);
    queue.post_owner_turn(1, NestedMode::Modal, 41).unwrap();
    assert_eq!(queue.drive_once(), Some(41));
    assert_eq!(
        queue.post_owner_turn(1, NestedMode::Undeclared, 42),
        Err(AdapterError::UnsupportedAtCall(NestedMode::Undeclared))
    );

    let mut frame = FrameTickAdapter::default();
    validate_declared_mode(&frame, NestedMode::FrameOverlay).unwrap();
    assert_eq!(
        validate_declared_mode(&frame, NestedMode::Modal),
        Err(AdapterError::UnsupportedAtGeneration(NestedMode::Modal))
    );
    frame.activate(2);
    frame
        .post_owner_turn(2, NestedMode::FrameOverlay, 73)
        .unwrap();
    assert_eq!(frame.drive_once(), Some(73));
    assert_eq!(
        frame.post_owner_turn(2, NestedMode::Undeclared, 74),
        Err(AdapterError::UnsupportedAtCall(NestedMode::Undeclared))
    );
}
