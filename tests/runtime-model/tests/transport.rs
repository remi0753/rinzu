use rinzu_runtime_model::transport::{
    InjectFailure, ResourceLedger, TransportBudget, TransportFailure, spawn_round_trip,
    transport_bytes,
};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

fn assert_balanced(ledger: &ResourceLedger) {
    let resources = ledger.snapshot();
    assert_eq!(resources.slots_acquired, resources.slots_released);
    assert_eq!(resources.roots_acquired, resources.roots_released);
    assert_eq!(resources.buffers_acquired, resources.buffers_released);
}

#[test]
fn measures_one_kib_one_mib_and_one_hundred_mib_round_trips() {
    for size in [1024, 1024 * 1024, 100 * 1024 * 1024] {
        let budget = TransportBudget::new(size);
        let ledger = ResourceLedger::default();
        let input = vec![0x5a; size];
        let result = vec![0xa5; size];
        let metrics = spawn_round_trip(&input, &result, &budget, &ledger).unwrap();
        assert_eq!(metrics.input.exact_bytes, size);
        assert_eq!(metrics.result.exact_bytes, size);
        assert_eq!(metrics.input.copied_bytes, size * 2);
        assert_eq!(metrics.result.copied_bytes, size * 2);
        let snapshot = budget.snapshot();
        assert_eq!(snapshot.used, 0);
        assert_eq!(snapshot.peak, size);
        assert_eq!(snapshot.reservations, 2);
        assert_eq!(snapshot.releases, 2);
        assert_balanced(&ledger);
        eprintln!(
            "transport size={size} input_pause_us={} result_pause_us={} input_copy={} result_copy={} peak={} credit_wait_us={}",
            metrics.input.pause.as_micros(),
            metrics.result.pause.as_micros(),
            metrics.input.copied_bytes,
            metrics.result.copied_bytes,
            snapshot.peak,
            (metrics.input.credit_wait + metrics.result.credit_wait).as_micros(),
        );
    }
}

#[test]
fn full_budget_input_releases_credit_before_non_empty_result() {
    let budget = TransportBudget::new(4096);
    let ledger = ResourceLedger::default();
    let metrics = spawn_round_trip(&vec![1; 4096], &vec![2; 2048], &budget, &ledger).unwrap();
    assert_eq!(metrics.result.exact_bytes, 2048);
    assert_eq!(budget.snapshot().used, 0);
    assert_balanced(&ledger);
}

#[test]
fn credit_wait_wakes_after_the_previous_all_or_nothing_reservation_releases() {
    let budget = Arc::new(TransportBudget::new(1024));
    let (held, _) = budget.reserve(1024, Duration::from_secs(1)).unwrap();
    let waiter_budget = Arc::clone(&budget);
    let waiter = thread::spawn(move || {
        let started = std::time::Instant::now();
        let (reservation, measured) = waiter_budget.reserve(1024, Duration::from_secs(2)).unwrap();
        drop(reservation);
        (started.elapsed(), measured)
    });
    thread::sleep(Duration::from_millis(25));
    drop(held);
    let (elapsed, measured) = waiter.join().unwrap();
    assert!(elapsed >= Duration::from_millis(20));
    assert!(measured >= Duration::from_millis(20));
    let snapshot = budget.snapshot();
    assert!(snapshot.waits > 0);
    assert_eq!(snapshot.reservations, snapshot.releases);
}

#[test]
fn every_failure_path_releases_only_what_it_acquired() {
    for inject in [
        InjectFailure::Encode,
        InjectFailure::Decode,
        InjectFailure::Cancel,
    ] {
        let budget = TransportBudget::new(1024);
        let ledger = ResourceLedger::default();
        assert!(
            transport_bytes(
                &[7; 512],
                &budget,
                &ledger,
                inject,
                Duration::from_millis(50),
            )
            .is_err()
        );
        assert_eq!(budget.snapshot().used, 0);
        assert_eq!(budget.snapshot().reservations, 1);
        assert_eq!(budget.snapshot().releases, 1);
        assert_balanced(&ledger);
    }

    let budget = TransportBudget::new(8);
    let ledger = ResourceLedger::default();
    let error = transport_bytes(
        &[0; 9],
        &budget,
        &ledger,
        InjectFailure::None,
        Duration::from_millis(10),
    )
    .unwrap_err();
    assert_eq!(error, TransportFailure::ResourceLimit);
    assert_eq!(budget.snapshot().reservations, 0);
    assert_balanced(&ledger);

    let budget = TransportBudget::new(8);
    let (held, _) = budget.reserve(8, Duration::from_millis(10)).unwrap();
    let ledger = ResourceLedger::default();
    let error = transport_bytes(
        &[0; 1],
        &budget,
        &ledger,
        InjectFailure::None,
        Duration::from_millis(5),
    )
    .unwrap_err();
    assert_eq!(error, TransportFailure::CreditTimeout);
    drop(held);
    assert_eq!(budget.snapshot().reservations, budget.snapshot().releases);
    assert_balanced(&ledger);
}
