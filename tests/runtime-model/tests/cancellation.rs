use rinzu_runtime_model::cancellation::{
    Backend, CancellationToken, StructuredScope, TaskRegistry, Workload, run_until_cancelled,
};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn jit_and_aot_poll_cpu_bound_code_and_run_finally() {
    for backend in [Backend::BaselineJit, Backend::Aot] {
        for workload in [
            Workload::InfiniteBackedge,
            Workload::DeepRecursion,
            Workload::LongArithmetic,
        ] {
            let token = CancellationToken::default();
            let child_token = token.clone();
            let (started_tx, started_rx) = mpsc::channel();
            let child = thread::spawn(move || {
                run_until_cancelled(backend, workload, &child_token, &started_tx)
            });
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
            thread::sleep(Duration::from_millis(2));
            let cancelled_at = Instant::now();
            token.cancel();
            let report = child.join().unwrap();
            assert!(cancelled_at.elapsed() < Duration::from_millis(250));
            assert!(report.cancelled);
            assert_eq!(report.finally_runs, 1);
            assert!(report.polls > 0);
        }
    }
}

fn finish_parent(registry: TaskRegistry, fail: bool) -> Result<(), &'static str> {
    let mut scope = StructuredScope::new(registry);
    let future = scope.spawn();
    drop(future);
    if fail { Err("parent failed") } else { Ok(()) }
}

#[test]
fn discarded_future_parent_return_failure_and_shutdown_leave_no_orphan() {
    for fail in [false, true] {
        let registry = TaskRegistry::default();
        let _ = finish_parent(registry.clone(), fail);
        assert_eq!(registry.active(), 0);
    }

    let registry = TaskRegistry::default();
    let mut scope = StructuredScope::new(registry.clone());
    for _ in 0..8 {
        drop(scope.spawn());
    }
    scope.close();
    assert_eq!(registry.active(), 0);
    // Idempotent shutdown must neither join twice nor resurrect admission.
    scope.close();
    assert_eq!(registry.active(), 0);
}
