use rinzu_runtime_model::host_stress::{StressConfig, run_host_loop_stress};

#[test]
fn real_threads_preserve_host_loop_invariants() {
    let report = run_host_loop_stress(StressConfig::default());
    assert_eq!(report.iterations, 200);
    assert!(report.processed > 0);
    assert!(report.fail_fast_wins > 0);
}
