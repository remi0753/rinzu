use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::Duration;

use rinzu_runtime::heap::{FinalizerPolicy, HeapValue};
use rinzu_runtime::scheduler::{Resolution, ResolveResult, TaskOutcome};
use rinzu_runtime::standalone::{
    ManagedTask, RunStatus, StandaloneOptions, StandaloneRuntime, TaskAction, TaskEvent,
};
use rinzu_runtime::worker::{CpuWorkerPool, WorkerDomainId};

struct SpawnMain {
    stage: u8,
    resumes: Arc<Mutex<Vec<ThreadId>>>,
}

impl ManagedTask for SpawnMain {
    fn resume(&mut self, event: TaskEvent) -> TaskAction {
        self.resumes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(std::thread::current().id());
        match (self.stage, event) {
            (0, TaskEvent::Start) => {
                self.stage = 1;
                TaskAction::Pending {
                    label: "spawn-1".into(),
                    cleanup_safe: false,
                }
            }
            (1, TaskEvent::Operation(Resolution::Completed(Ok(())))) => {
                self.stage = 2;
                TaskAction::Pending {
                    label: "spawn-2".into(),
                    cleanup_safe: false,
                }
            }
            (2, TaskEvent::Operation(Resolution::Completed(Ok(())))) => TaskAction::Return,
            (_, event) => TaskAction::Fail(format!("unexpected worker event: {event:?}")),
        }
    }
}

#[test]
fn cpu_pool_uses_runtime_owned_workers_and_one_shot_domains() {
    let owner = std::thread::current().id();
    let resumes = Arc::new(Mutex::new(Vec::new()));
    let mut runtime = StandaloneRuntime::new(StandaloneOptions::default());
    let main = runtime
        .start_main(Box::new(SpawnMain {
            stage: 0,
            resumes: Arc::clone(&resumes),
        }))
        .unwrap();
    let workers = CpuWorkerPool::new();
    let mut domains = Vec::new();

    assert!(matches!(
        runtime.run_until_stalled().unwrap(),
        RunStatus::Stalled {
            pending_operations: 1
        }
    ));
    let first_ticket = runtime.take_operation_tickets().pop().unwrap();
    let first = workers
        .spawn(first_ticket, |domain| {
            let object = domain
                .heap_mut()
                .allocate(1, &[HeapValue::Integer(20)], FinalizerPolicy::None)
                .map_err(|error| error.to_string())?;
            let value = domain
                .heap_mut()
                .fields(object)
                .map_err(|error| error.to_string())?[0];
            let HeapValue::Integer(value) = value else {
                return Err("unexpected worker heap value".into());
            };
            Ok((value * 2).to_le_bytes().to_vec())
        })
        .unwrap();
    let completion = workers.wait_completion(Duration::from_secs(1)).unwrap();
    assert_eq!(completion.task, main);
    domains.push(completion.domain);
    assert!(matches!(
        runtime.run_until_stalled().unwrap(),
        RunStatus::Stalled {
            pending_operations: 1
        }
    ));

    let second_ticket = runtime.take_operation_tickets().pop().unwrap();
    let second = workers
        .spawn(second_ticket, |_domain| Ok(2_i64.to_le_bytes().to_vec()))
        .unwrap();
    let completion = workers.wait_completion(Duration::from_secs(1)).unwrap();
    assert_eq!(completion.task, main);
    domains.push(completion.domain);
    assert_eq!(
        runtime.run_until_stalled().unwrap(),
        RunStatus::Complete(TaskOutcome::Returned)
    );

    let first = first.take().unwrap().unwrap();
    let second = second.take().unwrap().unwrap();
    assert_eq!(i64::from_le_bytes(first.bytes.try_into().unwrap()), 40);
    assert_eq!(i64::from_le_bytes(second.bytes.try_into().unwrap()), 2);
    assert_ne!(first.evidence.thread, owner);
    assert_ne!(second.evidence.thread, owner);
    assert_eq!(
        domains,
        vec![WorkerDomainId(1), WorkerDomainId(2)],
        "every spawn receives a fresh one-shot Domain"
    );
    assert_eq!(first.evidence.domain, domains[0]);
    assert_eq!(second.evidence.domain, domains[1]);
    assert_eq!(
        *resumes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
        vec![owner, owner, owner]
    );
    assert!(runtime.resources().used.is_baseline());
}

struct CancelWorkerMain;

impl ManagedTask for CancelWorkerMain {
    fn resume(&mut self, event: TaskEvent) -> TaskAction {
        match event {
            TaskEvent::Start => TaskAction::Pending {
                label: "cancel-worker".into(),
                cleanup_safe: false,
            },
            TaskEvent::Operation(Resolution::Cancelled) => TaskAction::Return,
            event => TaskAction::Fail(format!("unexpected worker cancellation: {event:?}")),
        }
    }
}

#[test]
fn worker_cancel_wins_once_and_completion_becomes_late() {
    let mut runtime = StandaloneRuntime::new(StandaloneOptions::default());
    runtime.start_main(Box::new(CancelWorkerMain)).unwrap();
    assert!(matches!(
        runtime.run_until_stalled().unwrap(),
        RunStatus::Stalled {
            pending_operations: 1
        }
    ));
    let ticket = runtime.take_operation_tickets().pop().unwrap();
    let workers = CpuWorkerPool::new();
    let operation = workers
        .spawn(ticket, |domain| {
            while !domain.is_cancel_requested() {
                std::thread::yield_now();
            }
            Err("worker observed cancellation".into())
        })
        .unwrap();
    assert_eq!(operation.request_cancel(), ResolveResult::Won);
    assert_eq!(operation.request_timeout(), ResolveResult::Late);
    let completion = workers.wait_completion(Duration::from_secs(1)).unwrap();
    assert_eq!(completion.resolution, ResolveResult::Late);
    assert_eq!(
        runtime.run_until_stalled().unwrap(),
        RunStatus::Complete(TaskOutcome::Returned)
    );
    assert!(operation.take().unwrap().is_err());
    assert!(runtime.resources().used.is_baseline());
}
