use rinzu_runtime_model::{Model, PostResult, explore};

const RESULTS: [PostResult; 3] = [
    PostResult::Accepted,
    PostResult::Closed,
    PostResult::Failure,
];

#[test]
fn exhausts_every_single_producer_result() {
    for result in RESULTS {
        let report = explore(Model::active(vec![result]), 10_000)
            .unwrap_or_else(|error| panic!("{result:?} produced a counterexample:\n{error}"));
        assert!(report.visited_states > 0);
        assert!(report.graceful_terminals + report.fail_fast_terminals > 0);

        if result != PostResult::Failure {
            assert_eq!(report.fail_fast_terminals, 0);
        }
    }
}

#[test]
fn exhausts_all_two_producer_result_pairs() {
    for first in RESULTS {
        for second in RESULTS {
            let report =
                explore(Model::active(vec![first, second]), 250_000).unwrap_or_else(|error| {
                    panic!("({first:?}, {second:?}) produced a counterexample:\n{error}")
                });
            assert!(report.visited_states > 0);
            assert!(report.graceful_terminals + report.fail_fast_terminals > 0);
        }
    }
}

#[test]
fn accepted_and_closed_never_fail_fast() {
    let report = explore(
        Model::active(vec![PostResult::Accepted, PostResult::Closed]),
        250_000,
    )
    .unwrap_or_else(|error| panic!("unexpected counterexample:\n{error}"));

    assert_eq!(report.fail_fast_terminals, 0);
    assert!(report.graceful_terminals > 0);
}

#[test]
fn active_failure_has_a_single_fail_fast_winner() {
    let report = explore(
        Model::active(vec![PostResult::Failure, PostResult::Failure]),
        250_000,
    )
    .unwrap_or_else(|error| panic!("unexpected counterexample:\n{error}"));

    // Some schedules let normal leave win before either failure returns. The
    // schedules where Failure wins must still have exactly one fail-fast owner;
    // this is checked after every transition by the model invariant.
    assert!(report.fail_fast_terminals > 0);
}
