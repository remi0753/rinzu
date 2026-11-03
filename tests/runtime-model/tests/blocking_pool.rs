use rinzu_runtime_model::blocking_pool::{BlockingRegistry, BoundedPool, SubmitError};
use std::sync::{Arc, Barrier, mpsc};
use std::time::Duration;

#[test]
fn saturated_blocking_pool_does_not_starve_runtime_cpu_services() {
    let blocking = BoundedPool::new(2, 2);
    let cpu = BoundedPool::new(2, 16);
    let occupied = Arc::new(Barrier::new(3));
    let release = Arc::new(Barrier::new(3));
    for _ in 0..2 {
        let occupied = Arc::clone(&occupied);
        let release = Arc::clone(&release);
        blocking
            .try_submit(move || {
                occupied.wait();
                release.wait();
            })
            .unwrap();
    }
    occupied.wait();

    // Both bounded admission slots are now held behind the occupied workers.
    blocking.try_submit(|| {}).unwrap();
    blocking.try_submit(|| {}).unwrap();
    assert_eq!(blocking.try_submit(|| {}), Err(SubmitError::Full));

    let (progress_tx, progress_rx) = mpsc::channel();
    for service in ["cpu-spawn", "gc", "jit", "main-wake", "cancel-completion"] {
        let progress_tx = progress_tx.clone();
        cpu.try_submit(move || progress_tx.send(service).unwrap())
            .unwrap();
    }
    drop(progress_tx);
    let mut progressed = Vec::new();
    while let Ok(service) = progress_rx.recv_timeout(Duration::from_millis(250)) {
        progressed.push(service);
        if progressed.len() == 5 {
            break;
        }
    }
    progressed.sort_unstable();
    assert_eq!(
        progressed,
        ["cancel-completion", "cpu-spawn", "gc", "jit", "main-wake"]
    );

    release.wait();
    drop(blocking);
    drop(cpu);
}

#[test]
fn uninterruptible_native_call_is_reported_without_forced_unwind() {
    let pool = BoundedPool::new(1, 1);
    let registry = BlockingRegistry::default();
    let child_registry = registry.clone();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    pool.try_submit(move || {
        let _call = child_registry.enter("vendor_read_forever", false);
        entered_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    })
    .unwrap();
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();

    let blockers = registry.shutdown_blockers();
    assert_eq!(blockers.len(), 1);
    assert_eq!(blockers[0].label, "vendor_read_forever");
    assert!(!blockers[0].has_cancel_hook);
    assert_eq!(registry.active_count(), 1);

    // Shutdown reports the call but never unwinds its foreign stack.  The
    // fixture releases it explicitly so the test process can terminate.
    release_tx.send(()).unwrap();
    drop(pool);
    assert_eq!(registry.active_count(), 0);
}
