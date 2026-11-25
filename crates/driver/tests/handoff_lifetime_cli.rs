#![cfg(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "x86_64", target_env = "gnu")
))]
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
#[allow(dead_code)]
#[path = "support/source_platform.rs"]
mod platform;
#[allow(dead_code)]
#[path = "support/scratch.rs"]
mod scratch;
#[path = "support/target.rs"]
mod target;
use rinzu_runtime_model::native_lifetime::{Binding, replay_handoff};
use std::path::Path;
use std::process::Command;
#[path = "../../../tests/support/handoff.rs"]
mod adversarial;
fn stage(root: &Path) {
    for dir in ["app/src", "handoff/src"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    std::fs::write(root.join("app/rinzu.toml"),"[package]\nname=\"handoff_app\"\nlanguage=\"0.5\"\n[dependencies]\nhandoff={path=\"../handoff\"}\n").unwrap();
    std::fs::write(root.join("handoff/rinzu.toml"),"[package]\nname=\"handoff\"\nlanguage=\"0.5\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n[native]\nsources=[\"api.c\"]\nheaders=[\"observer.h\",\"api.h\",\"completion.h\"]\n").unwrap();
    let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/handoff_sdk");
    for file in ["api.c", "api.h", "observer.h"] {
        std::fs::copy(from.join(file), root.join("handoff").join(file)).unwrap();
    }
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include/rinzu/native_completion.h"),
        root.join("handoff/completion.h"),
    )
    .unwrap();
    std::fs::copy(from.join("api.rinzu"), root.join("handoff/src/api.rinzu")).unwrap();
    std::fs::copy(from.join("main.rinzu"), root.join("app/src/main.rinzu")).unwrap();
}
#[test]
fn actual_c_native_call_hands_off_to_completion_and_wake() {
    let scratch = scratch::Scratch::new("rinzu-handoff-cli");
    let root = scratch.path();
    stage(root);
    let app = root.join("app");
    platform::success(&platform::bounded_tool(
        Command::new(frozen_cli::path()).arg("build").arg(&app),
    ));
    let original = target::build_directory(&app).join("handoff_app");
    for sanitized in [false, true] {
        let binary = original.with_file_name(if sanitized {
            "handoff-san"
        } else {
            "handoff-plain"
        });
        let mut compiler = platform::clang();
        compiler
            .args([
                "-std=c11",
                "-O1",
                "-g",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-Wno-unused-label",
                "-pthread",
                "-include",
            ])
            .arg(root.join("handoff/observer.h"))
            .arg(original.with_extension("o"))
            .arg(original.with_extension("runtime.c"));
        if sanitized {
            compiler.args(["-fsanitize=address,undefined", "-fno-sanitize-recover=all"]);
        }
        platform::link_runtime(&mut compiler);
        if cfg!(target_os = "linux") {
            compiler.args(["-lcrypto", "-ldl"]);
        }
        platform::success(&platform::bounded_tool(compiler.arg("-o").arg(&binary)));
        for extension in ["source-packages", "resources"] {
            if original.with_extension(extension).exists() {
                std::os::unix::fs::symlink(
                    original.with_extension(extension),
                    binary.with_extension(extension),
                )
                .unwrap();
            }
        }
        for scenario in 0..4 {
            let mut run = Command::new(&binary);
            run.env("RINZU_TRANSPORT_EVIDENCE", "1")
                .env("RINZU_NATIVE_COMPLETION_DIRECT", "1")
                .env("RINZU_NATIVE_COMPLETION_NOTIFY", "1")
                .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
                .env("UBSAN_OPTIONS", "halt_on_error=1");
            if scenario == 0 {
                run.env("HANDOFF_EARLY", "1");
            }
            if scenario >= 2 {
                run.env("HANDOFF_CONTROL", (scenario - 1).to_string());
            }
            let output = platform::bounded(&mut run);
            assert_eq!(
                output.status.code(),
                Some(i32::from(scenario >= 2)),
                "{output:?}"
            );
            let report = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                report.contains(if scenario >= 2 {
                    "RINZU_FAILURE=cancelled"
                } else {
                    "RINZU_FAILURE=none"
                }),
                "{report}"
            );
            assert!(report.contains("\"resources\":\"baseline\""), "{report}");
            assert!(
                report.contains("RINZU_NATIVE_CALLS=3,3,3,3,3,3,1,0,0,0,0"),
                "{report}"
            );
            assert!(
                !report.contains("runtime error:") && !report.contains("ERROR: AddressSanitizer"),
                "{report}"
            );
            let events: Vec<_> = String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .filter(|s| s.split(',').count() == 4 && !s.starts_with("RINZU_"))
                .map(str::to_owned)
                .collect();
            let r = replay_handoff(Binding::CDirect, false, 7183, &events, 100_000)
                .unwrap_or_else(|e| panic!("{e}\n{}", events.join("\n")));
            let rejected = adversarial::reject_corruptions(Binding::CDirect, false, &events);
            println!(
                "HANDOFF C sanitized={sanitized} scenario={scenario} events={} frontier={} rejected={rejected}",
                r.events, r.maximum_frontier
            );
        }
    }
}
