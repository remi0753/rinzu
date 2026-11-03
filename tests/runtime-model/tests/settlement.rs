use rinzu_runtime_model::settlement::{OperationKind, Outcome, Resolver, SaturatedRuntime};
use std::sync::{Arc, Barrier};
use std::thread;

#[test]
fn completion_cancel_and_timeout_have_one_winner_for_every_operation_kind() {
    for kind in [
        OperationKind::Io,
        OperationKind::Timer,
        OperationKind::Blocking,
        OperationKind::Spawn,
        OperationKind::CallbackClose,
    ] {
        for _ in 0..500 {
            let resolver = Resolver::new(kind, 5);
            let start = Arc::new(Barrier::new(4));
            let mut racers = Vec::new();
            for outcome in [Outcome::Completed, Outcome::Cancelled, Outcome::TimedOut] {
                let resolver = resolver.clone();
                let start = Arc::clone(&start);
                racers.push(thread::spawn(move || {
                    start.wait();
                    resolver.try_settle(outcome)
                }));
            }
            start.wait();
            let winners = racers
                .into_iter()
                .map(|racer| usize::from(racer.join().unwrap()))
                .sum::<usize>();
            assert_eq!(winners, 1);
            let snapshot = resolver.snapshot();
            assert!(snapshot.settled);
            assert!(snapshot.outcome.is_some());
            assert_eq!(snapshot.future_settlements, 1);
            assert_eq!(snapshot.resource_releases, 5);
            assert_eq!(snapshot.late_result_releases, 2);
        }
    }
}

#[test]
fn one_reserved_cursor_control_item_recovers_every_saturated_limit() {
    let mut runtime = SaturatedRuntime::new(257, 17);
    runtime.begin_shutdown();
    while !runtime.is_shutdown_complete() {
        runtime.run_control_turn();
    }
    assert_eq!(runtime.control_turns(), 16);
    assert_eq!(runtime.max_control_items(), 1);
    assert_eq!(runtime.allocations_during_shutdown(), 0);
    assert_eq!(runtime.released_operation_slots(), 257);
}
