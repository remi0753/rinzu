pub fn progress(stderr: &str, runs: usize) {
    for marker in [
        "RESET",
        "NATIVE_ENTER",
        "ENTRIES_OBSERVED",
        "ORIGIN_FINALLY",
        "DELAY_ENTER",
        "DELAY_RETURN",
        "MAIN_STOP",
        "NATIVE_RETURN",
        "WORKER_FINALLY",
    ] {
        assert_eq!(
            stderr
                .lines()
                .filter(|line| *line == format!("PUMP_{marker}"))
                .count(),
            runs,
            "{marker}: {stderr}"
        );
    }
    let blockers = stderr
        .lines()
        .filter(|line| line.contains("shutdown-blocker symbol=pump_block "))
        .collect::<Vec<_>>();
    assert_eq!(blockers.len(), runs, "{stderr}");
    for warning in blockers {
        assert!(
            !warning.contains("source=0:")
                && warning.contains("owner=0x")
                && warning.contains("stack=pump_block"),
            "{warning}"
        );
    }
    for run in stderr.split("PUMP_RESET\n").skip(1) {
        let delay = run.find("PUMP_DELAY_RETURN").unwrap();
        let stop = run.find("PUMP_MAIN_STOP").unwrap();
        let returned = run.find("PUMP_NATIVE_RETURN").unwrap();
        let cleanup = run.find("PUMP_WORKER_FINALLY").unwrap();
        let warning = run.find("shutdown-blocker symbol=pump_block ").unwrap();
        let origin = run.find("PUMP_ORIGIN_FINALLY").unwrap();
        assert!(
            warning < stop
                && delay < stop
                && stop < returned
                && returned < cleanup
                && cleanup < origin,
            "{run}"
        );
    }
    assert!(
        !stderr.contains("ERROR: AddressSanitizer") && !stderr.contains("runtime error:"),
        "{stderr}"
    );
}
