use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use rinzu_runtime::blocking::{
    BlockingBindingMetadata, BlockingPool, CancelHook, NativeOperationClass,
    NativeThreadRequirement,
};
use rinzu_runtime::scheduler::{Resolution, ResolveResult, TaskOutcome};
use rinzu_runtime::standalone::{
    ManagedTask, RunStatus, StandaloneOptions, StandaloneRuntime, TaskAction, TaskEvent,
};

struct BlockingMain;

impl ManagedTask for BlockingMain {
    fn resume(&mut self, event: TaskEvent) -> TaskAction {
        match event {
            TaskEvent::Start => TaskAction::Pending {
                label: "native-blocking".into(),
                cleanup_safe: false,
            },
            TaskEvent::Operation(Resolution::Cancelled) => TaskAction::Return,
            event => TaskAction::Fail(format!("unexpected blocking event: {event:?}")),
        }
    }
}

#[test]
fn binding_metadata_routes_offload_and_invokes_cancel_hook_once() {
    let owner = std::thread::current().id();
    let mut runtime = StandaloneRuntime::new(StandaloneOptions::default());
    runtime.start_main(Box::new(BlockingMain)).unwrap();
    assert!(matches!(
        runtime.run_until_stalled().unwrap(),
        RunStatus::Stalled {
            pending_operations: 1
        }
    ));
    let ticket = runtime.take_operation_tickets().pop().unwrap();
    let pool = BlockingPool::new();
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let hook_counter = Arc::clone(&hook_calls);
    let metadata = BlockingBindingMetadata {
        symbol: "native_wait".into(),
        operation_class: NativeOperationClass::OffloadBlocking,
        required_thread: NativeThreadRequirement::Any,
        has_cancel_hook: true,
    };
    let operation = pool
        .submit(
            ticket,
            &metadata,
            Some(CancelHook::new(move || {
                hook_counter.fetch_add(1, Ordering::Relaxed);
            })),
            |token| {
                while !token.is_cancel_requested() {
                    std::thread::yield_now();
                }
                Err("native call observed cancellation".into())
            },
        )
        .unwrap();

    assert_eq!(operation.request_cancel(), ResolveResult::Won);
    assert_eq!(operation.request_cancel(), ResolveResult::Late);
    let completion = pool.wait_completion(Duration::from_secs(1)).unwrap();
    assert_eq!(completion.resolution, ResolveResult::Late);
    assert_ne!(completion.thread, owner);
    assert_eq!(hook_calls.load(Ordering::Relaxed), 1);
    assert!(operation.take().unwrap().is_err());
    assert_eq!(
        runtime.run_until_stalled().unwrap(),
        RunStatus::Complete(TaskOutcome::Returned)
    );
    assert!(runtime.resources().used.is_baseline());
}
