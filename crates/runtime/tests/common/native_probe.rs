use std::process::{Command, Output, Stdio};

pub fn run(command: &mut Command, intentional_memory_fault: bool) -> Output {
    // Darwin's external symbolizer can stall while reporting intentional bad
    // native reads, also with the old runtime headers. Keep ASan detection and
    // its fatal report, omitting symbol lookup only for those fault probes.
    command.env(
        "ASAN_OPTIONS",
        if intentional_memory_fault {
            "detect_leaks=0:symbolize=0"
        } else {
            "detect_leaks=0"
        },
    );
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if started.elapsed() > std::time::Duration::from_secs(30) {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!("native probe timed out: {command:?}: {output:?}");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
