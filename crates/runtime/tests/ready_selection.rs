#![cfg(any(target_os = "macos", target_os = "linux"))]
#[path = "../../../tests/support/native.rs"]
mod native;
#[path = "../../../tests/support/process.rs"]
mod process;

use rinzu_runtime_model::native_lifetime::{
    Protocol, ReadyAction as A, ReadyInput, ReadySelection,
};
use std::path::PathBuf;
use std::process::Command;

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn replay(stdout: &str) -> usize {
    let mut model = ReadySelection::new(ReadyInput::Candidates);
    for line in stdout.lines() {
        let fields = line.split(',').collect::<Vec<_>>();
        let [kind, id, value] = fields[..] else {
            panic!("unexpected trace {line}");
        };
        let id = id.parse::<usize>().unwrap();
        let action = match kind {
            "physical" => A::Publish(id),
            "read" => A::Observe(id),
            "settled" => A::Select,
            _ => panic!("unexpected trace {line}"),
        };
        assert!(model.actions().contains(&action), "{line}\n{stdout}");
        let before = model.clone();
        model.apply(action);
        before.check_step(action, &model).unwrap();
        model.check().unwrap();
        if kind == "read" {
            assert_eq!(model.observed(id), value == "1", "{line}\n{stdout}");
        }
        if kind == "settled" {
            assert_eq!(model.selection(), Some(id), "{stdout}");
            assert!(model.actions().contains(&A::Finish));
            model.apply(A::Finish);
            return id;
        }
    }
    panic!("no settlement\n{stdout}");
}

#[test]
fn c_candidate_scan_uses_observations_made_during_the_scan() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let scratch =
        Scratch(std::env::temp_dir().join(format!("rinzu-ready-scan-{}", std::process::id())));
    std::fs::create_dir(&scratch.0).unwrap();
    for sanitized in [false, true] {
        let binary = scratch.0.join("ready-scan");
        let mut compiler = native::clang();
        compiler
            .args([
                "-std=c11", "-O1", "-g", "-pthread", "-Wall", "-Wextra", "-Werror",
            ])
            .arg("-I")
            .arg(root.join("../../include"))
            .arg(root.join("tests/fixtures/ready_selection.c"))
            .arg("-o")
            .arg(&binary);
        if sanitized {
            compiler.args(["-fsanitize=address,undefined", "-fno-sanitize-recover=all"]);
        }
        let build = process::bounded_tool(&mut compiler);
        assert!(build.status.success(), "{build:?}");
        for late in 0..2 {
            for after in 0..3 {
                let output = process::bounded(
                    Command::new(&binary)
                        .args([late.to_string(), after.to_string()])
                        .env("RINZU_NATIVE_COMPLETION_NOTIFY", "1")
                        .env("RINZU_NATIVE_COMPLETION_DIRECT", "0")
                        .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
                        .env("UBSAN_OPTIONS", "halt_on_error=1"),
                );
                assert!(output.status.success(), "{late}/{after} {output:?}");
                assert!(output.stderr.is_empty(), "{output:?}");
                let selected = replay(std::str::from_utf8(&output.stdout).unwrap());
                assert_eq!(selected, if late < after { late } else { 2 });
                println!(
                    "READY SELECTION C sanitized={sanitized} late={late} after={after} selected={selected}"
                );
            }
        }
    }
}
