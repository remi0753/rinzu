use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use rinzu_runtime::resources::{ResourceKind, ResourceLimits};
use rinzu_runtime::scheduler::{Resolution, ResolveResult, TaskId, TaskOutcome};
use rinzu_runtime::standalone::{
    ManagedTask, OperationMailbox, RunStatus, StandaloneError, StandaloneOptions,
    StandaloneRuntime, TaskAction, TaskEvent,
};
use rinzu_runtime::worker::CpuWorkerPool;

#[test]
fn child_mailbox_keeps_credits_through_cancel_winner_and_physical_worker_return() {
    let wake_count = Arc::new(AtomicUsize::new(0));
    let wake = Arc::clone(&wake_count);
    let mut mailbox = OperationMailbox::new(
        ResourceLimits::default(),
        Arc::new(move || {
            wake.fetch_add(1, Ordering::Relaxed);
        }),
    );
    let workers = CpuWorkerPool::new();
    let ticket = mailbox.admit(TaskId(17), "child:9".into()).unwrap();
    let operation = ticket.operation;
    let (entered_send, entered) = std::sync::mpsc::channel();
    let (finish, finish_receive) = std::sync::mpsc::channel();
    let returned = Arc::new(AtomicBool::new(false));
    let worker_returned = Arc::clone(&returned);
    let result = workers.spawn(ticket, move |domain| {
        assert_eq!(domain.owner_thread(), std::thread::current().id());
        entered_send.send(()).unwrap();
        finish_receive.recv().unwrap();
        worker_returned.store(true, Ordering::Release);
        Ok(vec![7])
    });
    // The Domain's owner is the executing Worker; verify completion evidence
    // against the Origin after the native job has returned.
    let result = result.unwrap();
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(result.request_cancel(), ResolveResult::Won);
    let events = mailbox.take_resolutions().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].operation, operation);
    assert_eq!(events[0].resolution, Resolution::Cancelled);
    assert!(!returned.load(Ordering::Acquire));
    assert_eq!(mailbox.resources().used.pending_operations, 1);
    assert_eq!(mailbox.resources().used.queue_slots, 1);
    finish.send(()).unwrap();
    let completion = workers.wait_completion(Duration::from_secs(2)).unwrap();
    assert_eq!(completion.operation, operation);
    assert_eq!(completion.task, TaskId(17));
    assert_eq!(completion.resolution, ResolveResult::Late);
    let output = result.take().unwrap().unwrap();
    assert_ne!(output.evidence.thread, std::thread::current().id());
    assert_eq!(output.bytes, [7]);
    assert_eq!(mailbox.late_completions(), 1);
    assert!(mailbox.take_resolutions().unwrap().is_empty());
    assert_eq!(mailbox.settle(operation).unwrap(), Resolution::Cancelled);
    assert!(mailbox.resources().used.is_baseline());
    assert!(mailbox.settle(operation).is_err());
    assert_eq!(wake_count.load(Ordering::Relaxed), 1);
}

#[test]
fn mailbox_admission_is_atomic_and_completion_does_not_settle_a_parent_ticket() {
    struct Parent;
    impl ManagedTask for Parent {
        fn resume(&mut self, event: TaskEvent) -> TaskAction {
            if event == TaskEvent::Start {
                TaskAction::Pending {
                    label: "outer future".into(),
                    cleanup_safe: false,
                }
            } else {
                TaskAction::Return
            }
        }
    }
    let mut runtime = StandaloneRuntime::new(StandaloneOptions::default());
    runtime.start_main(Box::new(Parent)).unwrap();
    assert!(matches!(
        runtime.run_until_stalled().unwrap(),
        RunStatus::Stalled { .. }
    ));
    let parent = runtime.take_operation_tickets().pop().unwrap();
    let limits = ResourceLimits {
        queue_slots: 1,
        ..ResourceLimits::default()
    };
    let mut mailbox = OperationMailbox::new(limits, Arc::new(|| {}));
    let child = mailbox.admit(TaskId(18), "child".into()).unwrap();
    let before = mailbox.resources();
    assert!(matches!(
        mailbox.admit(TaskId(18), "rejected".into()),
        Err(StandaloneError::Resource(
            rinzu_runtime::resources::ResourceError::Limit {
                resource: ResourceKind::QueueSlot,
                ..
            }
        ))
    ));
    assert_eq!(mailbox.resources(), before);
    assert!(mailbox.settle(child.operation).is_err());
    assert_eq!(child.complete(Ok(())), ResolveResult::Won);
    assert_eq!(child.complete(Err("late".into())), ResolveResult::Late);
    let events = mailbox.take_resolutions().unwrap();
    assert_eq!(events[0].resolution, Resolution::Completed(Ok(())));
    assert_eq!(mailbox.resources(), before);
    assert_eq!(
        mailbox.settle(child.operation).unwrap(),
        Resolution::Completed(Ok(()))
    );
    assert!(mailbox.resources().used.is_baseline());
    assert!(matches!(
        runtime.run_until_stalled().unwrap(),
        RunStatus::Stalled { .. }
    ));
    parent.complete(Ok(()));
    assert_eq!(
        runtime.run_until_stalled().unwrap(),
        RunStatus::Complete(TaskOutcome::Returned)
    );
    assert!(runtime.resources().used.is_baseline());
    let next = mailbox.admit(TaskId(18), "next".into()).unwrap();
    assert!(next.operation > child.operation);
    next.timeout();
    mailbox.take_resolutions().unwrap();
    mailbox.settle(next.operation).unwrap();
    assert!(mailbox.resources().used.is_baseline());
    let mailbox = std::thread::spawn(move || {
        let mut mailbox = mailbox;
        assert!(matches!(
            mailbox.admit(TaskId(19), "foreign".into()),
            Err(StandaloneError::Runtime(
                rinzu_runtime::RuntimeError::WrongThread { .. }
            ))
        ));
        mailbox
    })
    .join()
    .unwrap();
    assert!(mailbox.resources().used.is_baseline());
}
