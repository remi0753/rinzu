use std::path::PathBuf;
use std::time::Duration;

use rinzu_runtime::reactor::AsyncReactor;
use rinzu_runtime::scheduler::{Resolution, TaskOutcome};
use rinzu_runtime::standalone::{
    ManagedTask, RunStatus, StandaloneOptions, StandaloneRuntime, TaskAction, TaskEvent,
};
use rinzu_runtime::worker::CpuWorkerPool;

struct FileChild;

impl ManagedTask for FileChild {
    fn resume(&mut self, event: TaskEvent) -> TaskAction {
        match event {
            TaskEvent::Start => TaskAction::Pending {
                label: "concurrent-file".into(),
                cleanup_safe: false,
            },
            TaskEvent::Operation(Resolution::Completed(Ok(()))) => TaskAction::Return,
            event => TaskAction::Fail(format!("unexpected file event: {event:?}")),
        }
    }
}

struct CpuParent {
    child: Option<Box<dyn ManagedTask>>,
}

impl ManagedTask for CpuParent {
    fn resume(&mut self, event: TaskEvent) -> TaskAction {
        match event {
            TaskEvent::Start => TaskAction::Spawn(self.child.take().unwrap()),
            TaskEvent::ChildSpawned(_) => TaskAction::Pending {
                label: "concurrent-cpu".into(),
                cleanup_safe: false,
            },
            TaskEvent::Operation(Resolution::Completed(Ok(()))) => TaskAction::Return,
            event => TaskAction::Fail(format!("unexpected CPU event: {event:?}")),
        }
    }
}

#[test]
fn standalone_file_io_and_cpu_spawn_progress_concurrently_and_release_resources() {
    let mut runtime = StandaloneRuntime::new(StandaloneOptions::default());
    runtime
        .start_main(Box::new(CpuParent {
            child: Some(Box::new(FileChild)),
        }))
        .unwrap();
    assert_eq!(
        runtime.run_until_stalled().unwrap(),
        RunStatus::Stalled {
            pending_operations: 2
        }
    );
    let tickets = runtime.take_operation_tickets();
    let file_ticket = tickets
        .iter()
        .find(|ticket| ticket.label == "concurrent-file")
        .unwrap()
        .clone();
    let cpu_ticket = tickets
        .iter()
        .find(|ticket| ticket.label == "concurrent-cpu")
        .unwrap()
        .clone();

    let reactor = AsyncReactor::new();
    let workers = CpuWorkerPool::new();
    let file = reactor
        .submit_file_read(file_ticket, fixture_path())
        .unwrap();
    let cpu = workers
        .spawn(cpu_ticket, |_domain| {
            let sum = (0_u64..100_000).sum::<u64>();
            Ok(sum.to_le_bytes().to_vec())
        })
        .unwrap();
    reactor.wait_completion(Duration::from_secs(1)).unwrap();
    workers.wait_completion(Duration::from_secs(1)).unwrap();

    assert_eq!(
        runtime.run_until_stalled().unwrap(),
        RunStatus::Complete(TaskOutcome::Returned)
    );
    assert_eq!(file.take().unwrap().unwrap(), b"rinzu-async-fixture\n");
    let output = cpu.take().unwrap().unwrap();
    assert_eq!(
        u64::from_le_bytes(output.bytes.try_into().unwrap()),
        (0_u64..100_000).sum::<u64>()
    );
    assert!(runtime.resources().used.is_baseline());
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("async.txt")
}

struct FailedIoMain;

impl ManagedTask for FailedIoMain {
    fn resume(&mut self, event: TaskEvent) -> TaskAction {
        match event {
            TaskEvent::Start => TaskAction::Pending {
                label: "missing-file".into(),
                cleanup_safe: false,
            },
            TaskEvent::Operation(Resolution::Completed(Err(_))) => TaskAction::Return,
            event => TaskAction::Fail(format!("unexpected failed I/O event: {event:?}")),
        }
    }
}

#[test]
fn failed_file_io_releases_the_reserved_operation_once() {
    let mut runtime = StandaloneRuntime::new(StandaloneOptions::default());
    runtime.start_main(Box::new(FailedIoMain)).unwrap();
    assert!(matches!(
        runtime.run_until_stalled().unwrap(),
        RunStatus::Stalled {
            pending_operations: 1
        }
    ));
    let ticket = runtime.take_operation_tickets().pop().unwrap();
    let reactor = AsyncReactor::new();
    let result = reactor
        .submit_file_read(ticket, "/definitely/missing/rinzu-i4-fixture")
        .unwrap();
    reactor.wait_completion(Duration::from_secs(1)).unwrap();
    assert_eq!(
        runtime.run_until_stalled().unwrap(),
        RunStatus::Complete(TaskOutcome::Returned)
    );
    assert!(result.take().unwrap().is_err());
    assert!(runtime.resources().used.is_baseline());
}
