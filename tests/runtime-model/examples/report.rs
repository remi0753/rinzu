use rinzu_runtime_model::{Model, PostResult, explore};

const RESULTS: [PostResult; 3] = [
    PostResult::Accepted,
    PostResult::Closed,
    PostResult::Failure,
];

fn main() {
    let mut scenarios = 0_u32;
    let mut visited_states = 0_usize;
    let mut graceful_terminals = 0_usize;
    let mut fail_fast_terminals = 0_usize;
    let mut maximum_depth = 0_usize;

    for result in RESULTS {
        add_report(
            &mut scenarios,
            &mut visited_states,
            &mut graceful_terminals,
            &mut fail_fast_terminals,
            &mut maximum_depth,
            &explore(Model::active(vec![result]), 10_000)
                .unwrap_or_else(|error| panic!("{result:?} counterexample:\n{error}")),
        );
    }

    for first in RESULTS {
        for second in RESULTS {
            add_report(
                &mut scenarios,
                &mut visited_states,
                &mut graceful_terminals,
                &mut fail_fast_terminals,
                &mut maximum_depth,
                &explore(Model::active(vec![first, second]), 250_000).unwrap_or_else(|error| {
                    panic!("({first:?}, {second:?}) counterexample:\n{error}")
                }),
            );
        }
    }

    println!("scenarios: {scenarios}");
    println!("visited states (sum): {visited_states}");
    println!("graceful terminal states (sum): {graceful_terminals}");
    println!("fail-fast terminal states (sum): {fail_fast_terminals}");
    println!("maximum trace depth: {maximum_depth}");
}

#[allow(clippy::too_many_arguments)]
fn add_report(
    scenarios: &mut u32,
    visited_states: &mut usize,
    graceful_terminals: &mut usize,
    fail_fast_terminals: &mut usize,
    maximum_depth: &mut usize,
    report: &rinzu_runtime_model::ExplorationReport,
) {
    *scenarios += 1;
    *visited_states += report.visited_states;
    *graceful_terminals += report.graceful_terminals;
    *fail_fast_terminals += report.fail_fast_terminals;
    *maximum_depth = (*maximum_depth).max(report.maximum_depth);
}
