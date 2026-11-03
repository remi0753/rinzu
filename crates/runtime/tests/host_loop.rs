use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use rinzu_runtime::diagnostic::{LogicalFrameKind, SourceLocation};
use rinzu_runtime::host_loop::{
    HostLoopAdapter, HostLoopController, HostPhase, HostPostResult, OwnerTurnTarget, PostOutcome,
    standard_queue_post_controller,
};

struct QueueAdapter {
    result: Mutex<HostPostResult>,
    posts: Mutex<VecDeque<u64>>,
    prepares: AtomicUsize,
    deactivates: AtomicUsize,
}

impl QueueAdapter {
    fn new(result: HostPostResult) -> Self {
        Self {
            result: Mutex::new(result),
            posts: Mutex::new(VecDeque::new()),
            prepares: AtomicUsize::new(0),
            deactivates: AtomicUsize::new(0),
        }
    }

    fn take_post(&self) -> u64 {
        self.posts.lock().unwrap().pop_front().unwrap()
    }

    fn post_count(&self) -> usize {
        self.posts.lock().unwrap().len()
    }
}

impl HostLoopAdapter for QueueAdapter {
    fn prepare(&self, _owner: OwnerTurnTarget, generation: u64) -> Result<usize, String> {
        self.prepares.fetch_add(1, Ordering::Relaxed);
        Ok(generation as usize + 100)
    }

    fn post_owner_turn(&self, _ready_token: usize, generation: u64) -> HostPostResult {
        self.posts.lock().unwrap().push_back(generation);
        *self.result.lock().unwrap()
    }

    fn deactivate(&self, _ready_token: usize, _generation: u64) {
        self.deactivates.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn generation_coalescing_bounded_drain_and_late_delivery_are_safe() {
    let adapter = Arc::new(QueueAdapter::new(HostPostResult::Accepted));
    let controller = HostLoopController::new(adapter.clone(), 8, 2);
    let executions = Arc::new(AtomicUsize::new(0));

    let before_enter = Arc::clone(&executions);
    assert_eq!(
        controller
            .enqueue(move || {
                before_enter.fetch_add(1, Ordering::Relaxed);
            })
            .unwrap(),
        PostOutcome::Inactive
    );
    let activation = controller.enter().unwrap();
    let generation = activation.generation();
    assert_eq!(adapter.take_post(), generation);

    for _ in 0..4 {
        let executions = Arc::clone(&executions);
        assert_eq!(
            controller
                .enqueue(move || {
                    executions.fetch_add(1, Ordering::Relaxed);
                })
                .unwrap(),
            PostOutcome::Coalesced
        );
    }
    assert_eq!(adapter.post_count(), 0);

    let first = controller.owner_turn(generation).unwrap();
    assert_eq!(first.executed, 2);
    assert_eq!(first.follow_up, PostOutcome::Accepted);
    assert_eq!(adapter.take_post(), generation);

    let second = controller.owner_turn(generation).unwrap();
    assert_eq!(second.executed, 2);
    assert_eq!(second.follow_up, PostOutcome::Accepted);
    assert_eq!(adapter.take_post(), generation);

    let third = controller.owner_turn(generation).unwrap();
    assert_eq!(third.executed, 1);
    assert_eq!(executions.load(Ordering::Relaxed), 5);
    assert_eq!(controller.snapshot().queued_work, 0);

    let duplicate = controller.owner_turn(generation).unwrap();
    assert_eq!(duplicate.executed, 0);
    assert!(!duplicate.stale);

    let report = activation.leave().unwrap();
    assert!(!report.integration_failed);
    assert!(!report.needs_runtime_pump);
    assert_eq!(adapter.deactivates.load(Ordering::Relaxed), 1);
    assert_eq!(controller.snapshot().phase, HostPhase::Managed);

    let late = controller.owner_turn(generation).unwrap();
    assert!(late.stale);
    assert!(
        controller.enter().is_err(),
        "root Host Loop entry is single-use"
    );
}

#[test]
fn closed_and_failure_posts_close_admission_and_deactivate_once() {
    for (post_result, failed) in [
        (HostPostResult::Closed, false),
        (HostPostResult::Failure, true),
    ] {
        let adapter = Arc::new(QueueAdapter::new(post_result));
        let controller = HostLoopController::new(adapter.clone(), 2, 1);
        let activation = controller.enter().unwrap();
        let outcome = controller.enqueue(|| {}).unwrap();
        assert_eq!(
            outcome,
            if failed {
                PostOutcome::Failed
            } else {
                PostOutcome::Closed
            }
        );
        assert_eq!(
            controller.snapshot().phase,
            if failed {
                HostPhase::Closing
            } else {
                HostPhase::Leaving
            }
        );
        let report = activation.leave().unwrap();
        assert_eq!(report.integration_failed, failed);
        assert!(report.needs_runtime_pump);
        assert_eq!(adapter.deactivates.load(Ordering::Relaxed), 1);
    }
}

struct BlockingPostAdapter {
    entered: std::sync::mpsc::Sender<()>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
    deactivates: AtomicUsize,
}

impl HostLoopAdapter for BlockingPostAdapter {
    fn prepare(&self, _owner: OwnerTurnTarget, _generation: u64) -> Result<usize, String> {
        Ok(1)
    }

    fn post_owner_turn(&self, _ready_token: usize, _generation: u64) -> HostPostResult {
        self.entered.send(()).unwrap();
        self.release.lock().unwrap().recv().unwrap();
        HostPostResult::Accepted
    }

    fn deactivate(&self, _ready_token: usize, _generation: u64) {
        self.deactivates.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn leave_waits_for_the_last_post_lease_before_deactivate() {
    let (entered_sender, entered_receiver) = std::sync::mpsc::channel();
    let (release_sender, release_receiver) = std::sync::mpsc::channel();
    let adapter = Arc::new(BlockingPostAdapter {
        entered: entered_sender,
        release: Mutex::new(release_receiver),
        deactivates: AtomicUsize::new(0),
    });
    let controller = HostLoopController::new(adapter.clone(), 2, 1);
    let activation = controller.enter().unwrap();

    let producer_controller = Arc::clone(&controller);
    let producer = std::thread::spawn(move || producer_controller.enqueue(|| {}).unwrap());
    entered_receiver.recv().unwrap();
    assert_eq!(controller.snapshot().in_flight_posts, 1);

    let observer = Arc::clone(&controller);
    let release = std::thread::spawn(move || {
        while !matches!(
            observer.snapshot().phase,
            HostPhase::Leaving | HostPhase::Closing
        ) {
            std::thread::yield_now();
        }
        release_sender.send(()).unwrap();
    });
    let report = activation.leave().unwrap();
    release.join().unwrap();
    assert_eq!(producer.join().unwrap(), PostOutcome::Accepted);
    assert!(!report.integration_failed);
    assert_eq!(adapter.deactivates.load(Ordering::Relaxed), 1);
    assert_eq!(controller.snapshot().in_flight_posts, 0);
}

#[test]
fn standard_adapter_requires_only_an_owner_thread_post_primitive() {
    let deliveries = Arc::new(Mutex::new(VecDeque::<(OwnerTurnTarget, u64)>::new()));
    let posted = Arc::clone(&deliveries);
    let controller = standard_queue_post_controller(move |owner, generation| {
        posted.lock().unwrap().push_back((owner, generation));
        HostPostResult::Accepted
    });
    let activation = controller.enter().unwrap();
    let executions = Arc::new(AtomicUsize::new(0));
    let executed = Arc::clone(&executions);
    assert_eq!(
        controller
            .enqueue(move || {
                executed.fetch_add(1, Ordering::Relaxed);
            })
            .unwrap(),
        PostOutcome::Accepted
    );
    let (delivery, generation) = deliveries.lock().unwrap().pop_front().unwrap();
    // SAFETY: the standard adapter retained the controller-owned target and
    // this test delivers it on the owner thread for the active generation.
    unsafe { (delivery.callback)(delivery.context, generation) };
    assert_eq!(executions.load(Ordering::Relaxed), 1);

    activation.leave().unwrap();
    // A native queue may deliver an accepted duplicate after deactivation.
    // SAFETY: the controller and process-lifetime trampoline are still live.
    unsafe { (delivery.callback)(delivery.context, generation) };
    assert_eq!(executions.load(Ordering::Relaxed), 1);
}

#[test]
fn callback_failure_and_wrong_thread_include_logical_diagnostics() {
    let adapter = Arc::new(QueueAdapter::new(HostPostResult::Accepted));
    let controller = HostLoopController::new(adapter.clone(), 4, 2);
    let activation = controller.enter().unwrap();
    let generation = activation.generation();
    controller
        .enqueue_at(
            "onButton",
            Some(SourceLocation {
                file: "app/main.rinzu".into(),
                line: 12,
                column: 5,
            }),
            || panic!("contained callback panic"),
        )
        .unwrap();
    assert_eq!(adapter.take_post(), generation);
    let report = controller.owner_turn(generation).unwrap();
    assert!(report.work_panicked);

    let wrong_thread_controller = Arc::clone(&controller);
    let error = std::thread::spawn(move || wrong_thread_controller.owner_turn(generation))
        .join()
        .unwrap()
        .unwrap_err();
    assert!(matches!(
        error,
        rinzu_runtime::host_loop::HostLoopError::WrongThread { .. }
    ));

    let diagnostics = controller.diagnostics();
    let callback = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "RZRT1002")
        .unwrap();
    assert_eq!(
        callback
            .frames
            .iter()
            .map(|frame| frame.kind)
            .collect::<Vec<_>>(),
        vec![
            LogicalFrameKind::Managed,
            LogicalFrameKind::Native,
            LogicalFrameKind::OwnerSync,
        ]
    );
    assert_eq!(
        callback.frames[2].source.as_ref().unwrap().file,
        "app/main.rinzu"
    );
    let wrong_thread = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "RZRT1001")
        .unwrap();
    assert_ne!(wrong_thread.expected_thread, wrong_thread.actual_thread);
    activation.leave().unwrap();
}
