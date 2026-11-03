use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::Duration;

use rinzu_runtime::reactor::{AsyncReactor, ReactorOperationKind, ReactorTracePhase};
use rinzu_runtime::scheduler::{Resolution, ResolveResult, TaskOutcome};
use rinzu_runtime::standalone::{
    ManagedTask, RunStatus, StandaloneOptions, StandaloneRuntime, TaskAction, TaskEvent,
};

struct AsyncMain {
    stage: u8,
    resumes: Arc<Mutex<Vec<ThreadId>>>,
}

impl ManagedTask for AsyncMain {
    fn resume(&mut self, event: TaskEvent) -> TaskAction {
        self.resumes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(std::thread::current().id());
        match (self.stage, event) {
            (0, TaskEvent::Start) => {
                self.stage = 1;
                TaskAction::Pending {
                    label: "timer".into(),
                    cleanup_safe: false,
                }
            }
            (1, TaskEvent::Operation(Resolution::Completed(Ok(())))) => {
                self.stage = 2;
                TaskAction::Pending {
                    label: "file-read".into(),
                    cleanup_safe: false,
                }
            }
            (2, TaskEvent::Operation(Resolution::Completed(Ok(())))) => {
                self.stage = 3;
                TaskAction::Return
            }
            (_, event) => TaskAction::Fail(format!("unexpected async event: {event:?}")),
        }
    }
}

#[test]
fn timer_and_file_await_release_owner_and_resume_the_original_domain() {
    let owner_thread = std::thread::current().id();
    let resumes = Arc::new(Mutex::new(Vec::new()));
    let mut runtime = StandaloneRuntime::new(StandaloneOptions::default());
    let main = runtime
        .start_main(Box::new(AsyncMain {
            stage: 0,
            resumes: Arc::clone(&resumes),
        }))
        .unwrap();
    let reactor = AsyncReactor::new();

    assert_eq!(
        runtime.run_until_stalled().unwrap(),
        RunStatus::Stalled {
            pending_operations: 1
        }
    );
    let timer = runtime.take_operation_tickets().pop().unwrap();
    assert_eq!(timer.task, main);
    let _timer = reactor
        .submit_timer(timer, Duration::from_millis(1))
        .unwrap();

    // The owner thread is not parked by the pending await; it can perform its
    // own turn before the foreign completion is imported.
    let owner_progress_marker = std::thread::current().id();
    assert_eq!(owner_progress_marker, owner_thread);
    let timer_completion = reactor.wait_completion(Duration::from_secs(1)).unwrap();
    assert_eq!(timer_completion.task, main);
    assert_eq!(timer_completion.kind, ReactorOperationKind::Timer);

    assert_eq!(
        runtime.run_until_stalled().unwrap(),
        RunStatus::Stalled {
            pending_operations: 1
        }
    );
    let file_ticket = runtime.take_operation_tickets().pop().unwrap();
    assert_eq!(file_ticket.task, main);
    let fixture = fixture_path();
    let file_result = reactor.submit_file_read(file_ticket, &fixture).unwrap();
    let file_completion = reactor.wait_completion(Duration::from_secs(1)).unwrap();
    assert_eq!(file_completion.task, main);
    assert_eq!(file_completion.kind, ReactorOperationKind::FileRead);

    assert_eq!(
        runtime.run_until_stalled().unwrap(),
        RunStatus::Complete(TaskOutcome::Returned)
    );
    assert_eq!(
        file_result.take().unwrap().unwrap(),
        b"rinzu-async-fixture\n"
    );
    assert!(runtime.resources().used.is_baseline());

    let resumes = resumes
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    assert_eq!(resumes, vec![owner_thread, owner_thread, owner_thread]);
    let trace = reactor.trace();
    assert_eq!(trace.len(), 6);
    assert!(trace.iter().all(|event| event.task == main));
    assert!(
        trace
            .iter()
            .filter(|event| event.phase == ReactorTracePhase::Submitted)
            .all(|event| event.thread == owner_thread)
    );
    assert!(
        trace
            .iter()
            .filter(|event| event.phase != ReactorTracePhase::Submitted)
            .all(|event| event.thread != owner_thread)
    );
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("async.txt")
}

#[test]
fn pending_timer_does_not_serialize_zero_timer_or_file_and_cancel_retains_admission() {
    use rinzu_runtime::{
        resources::ResourceLimits, scheduler::TaskId, standalone::OperationMailbox,
    };
    let reactor = AsyncReactor::new();
    let mut mailbox = OperationMailbox::new(ResourceLimits::default(), Arc::new(|| {}));
    let slow = mailbox.admit(TaskId(1), "slow timer".into()).unwrap();
    let slow_id = slow.operation;
    let started = std::time::Instant::now();
    let timer = reactor
        .submit_timer(slow, Duration::from_millis(500))
        .unwrap();
    let zero = mailbox.admit(TaskId(2), "zero timer".into()).unwrap();
    let zero_id = zero.operation;
    reactor.submit_timer(zero, Duration::ZERO).unwrap();
    let file = reactor
        .submit_file_read(
            mailbox.admit(TaskId(3), "file".into()).unwrap(),
            fixture_path(),
        )
        .unwrap();
    assert_eq!(timer.request_cancel(), ResolveResult::Won);
    // Ready operations can resolve before the slow timer is cancelled.
    let cancellation = mailbox
        .take_resolutions()
        .unwrap()
        .into_iter()
        .find(|event| event.operation == slow_id)
        .expect("the cancellation winner publishes its own resolution");
    assert_eq!(cancellation.resolution, Resolution::Cancelled);
    for expected in [ReactorOperationKind::Timer, ReactorOperationKind::FileRead] {
        let packet = reactor.wait_completion(Duration::from_secs(2)).unwrap();
        assert_ne!(
            packet.operation, slow_id,
            "later ready services must progress before the slow timer"
        );
        assert_eq!(packet.kind, expected);
        if expected == ReactorOperationKind::Timer {
            assert_eq!(packet.operation, zero_id);
        }
        mailbox.take_resolutions().unwrap();
        mailbox.settle(packet.operation).unwrap();
    }
    assert_eq!(file.take().unwrap().unwrap(), b"rinzu-async-fixture\n");
    assert!(
        !mailbox.resources().used.is_baseline(),
        "a cancellation winner is not a physical return"
    );
    let packet = reactor.wait_completion(Duration::from_secs(2)).unwrap();
    assert_eq!(packet.operation, slow_id);
    assert_eq!(packet.resolution, ResolveResult::Late);
    assert!(started.elapsed() >= Duration::from_millis(500));
    assert_eq!(mailbox.settle(slow_id).unwrap(), Resolution::Cancelled);
    assert!(mailbox.resources().used.is_baseline());
}

struct TimeoutMain;

impl ManagedTask for TimeoutMain {
    fn resume(&mut self, event: TaskEvent) -> TaskAction {
        match event {
            TaskEvent::Start => TaskAction::Pending {
                label: "timer-timeout".into(),
                cleanup_safe: false,
            },
            TaskEvent::Operation(Resolution::TimedOut) => TaskAction::Return,
            event => TaskAction::Fail(format!("unexpected timeout event: {event:?}")),
        }
    }
}

#[test]
fn timer_timeout_wins_once_and_late_completion_is_ignored() {
    let mut runtime = StandaloneRuntime::new(StandaloneOptions::default());
    runtime.start_main(Box::new(TimeoutMain)).unwrap();
    assert!(matches!(
        runtime.run_until_stalled().unwrap(),
        RunStatus::Stalled {
            pending_operations: 1
        }
    ));
    let ticket = runtime.take_operation_tickets().pop().unwrap();
    let reactor = AsyncReactor::new();
    let operation = reactor
        .submit_timer(ticket, Duration::from_millis(5))
        .unwrap();
    assert_eq!(operation.request_timeout(), ResolveResult::Won);
    assert_eq!(operation.request_cancel(), ResolveResult::Late);
    assert_eq!(
        runtime.run_until_stalled().unwrap(),
        RunStatus::Complete(TaskOutcome::Returned)
    );
    let completion = reactor.wait_completion(Duration::from_secs(1)).unwrap();
    assert_eq!(completion.resolution, ResolveResult::Late);
    assert!(runtime.resources().used.is_baseline());
}
