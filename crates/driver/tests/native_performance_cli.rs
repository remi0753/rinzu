#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
use std::path::Path;
use std::process::{Command, Output};
#[path = "fixtures/native_performance_sdk/setup.rs"]
mod fixture;
#[path = "support/source_platform.rs"]
mod platform;

fn measurements(output: &Output, jit: bool) -> serde_json::Value {
    platform::success(output);
    let stdout = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("\"resources\": \"baseline\"")
            || stdout.contains("\"resources\":\"baseline\""),
        "{stdout}"
    );
    let mut values = std::collections::BTreeMap::new();
    for line in stdout
        .lines()
        .filter_map(|line| line.strip_prefix("RINZU_PERF="))
    {
        let fields: Vec<u64> = line
            .split(',')
            .map(|value| value.parse().unwrap())
            .collect();
        assert_eq!(fields.len(), 3);
        assert!(fields[1] > 0);
        assert!(values.insert(fields[0], serde_json::json!({"totalNanoseconds":fields[1],"samples":fields[2],"meanNanoseconds":fields[1] as f64/fields[2] as f64})).is_none());
    }
    assert_eq!(
        values.keys().copied().collect::<Vec<_>>(),
        [1, 2, 3, 4, 5, 6]
    );
    for (kind, count) in [(1, 1024), (2, 1024), (3, 1024), (4, 512), (5, 512), (6, 1)] {
        assert_eq!(values[&kind]["samples"], count);
    }
    assert!(
        values[&5]["totalNanoseconds"].as_u64().unwrap()
            >= values[&4]["totalNanoseconds"].as_u64().unwrap()
    );
    assert_eq!(stdout.matches("RINZU_OWNER_MANAGED_WAKE=1\n").count(), 1);
    let resources: Vec<_> = stdout
        .lines()
        .filter(|line| {
            line.starts_with("RINZU_")
                && !line.starts_with("RINZU_PERF=")
                && !line.starts_with("RINZU_TRACE=")
        })
        .collect();
    if jit {
        assert!(
            stdout.contains("compiled JIT continuation runtime"),
            "{stdout}"
        );
    } else {
        assert!(
            stdout.contains("RINZU_FAILURE=none")
                && stdout.contains("RINZU_FOREIGN=512,512,512,512,0,0,0,0"),
            "{stdout}"
        );
        let calls: Vec<usize> = stdout
            .lines()
            .find_map(|line| line.strip_prefix("RINZU_NATIVE_CALLS="))
            .unwrap()
            .split(',')
            .map(|field| field.parse().unwrap())
            .collect();
        assert_eq!(calls[0], 1546);
        assert!(calls[..6].iter().all(|count| *count == calls[0]));
        assert_eq!(calls[6], 2561);
        assert!(stdout.contains("RINZU_CALLBACK_TEMPORARY=0,0"), "{stdout}");
        assert!(
            stdout.contains("RINZU_CALLBACK_PROCESS=4,608,24"),
            "{stdout}"
        );
        assert!(
            stdout.contains("RINZU_NATIVE_ENTRIES=4,1,32768,16992,0,0"),
            "{stdout}"
        );
    }
    serde_json::json!({"measurements":values,"resourceBaseline":true,"resourceLedger":resources})
}
fn preserve(name: &str, output: &Output, value: &serde_json::Value) {
    let directory = std::env::var_os("RINZU_GATE_ARTIFACTS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../artifacts/i7.6-r6/performance")
        });
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join(format!("{name}.stdout.log")), &output.stdout).unwrap();
    std::fs::write(directory.join(format!("{name}.stderr.log")), &output.stderr).unwrap();
    std::fs::write(
        directory.join(format!("{name}.json")),
        serde_json::to_vec_pretty(value).unwrap(),
    )
    .unwrap();
    println!("{name}: {}", value.get("measurements").unwrap_or(value));
}
#[test]
fn native_transition_callback_and_owner_completion_costs_are_measured_in_jit_and_aot() {
    let root = std::env::temp_dir().join(format!("rinzu-native-perf-{}", std::process::id()));
    let app = fixture::create(&root);
    let jit = platform::bounded(
        Command::new(frozen_cli::path())
            .arg("run")
            .arg(&app)
            .arg("--trace"),
    );
    preserve("managed-jit", &jit, &measurements(&jit, true));
    platform::success(&platform::bounded(
        Command::new(frozen_cli::path()).arg("build").arg(&app),
    ));
    let binary = app.join("build/native_perf_app");
    let aot = platform::bounded(Command::new(&binary).env("RINZU_TRANSPORT_EVIDENCE", "1"));
    preserve("managed-aot", &aot, &measurements(&aot, false));
    platform::sign(&binary, &root);
    let signed = platform::bounded(Command::new(&binary).env("RINZU_TRANSPORT_EVIDENCE", "1"));
    preserve("managed-signed-aot", &signed, &measurements(&signed, false));
    let sanitized = platform::sanitize(&binary);
    let sanitized_run = platform::bounded(
        Command::new(&sanitized)
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .env("UBSAN_OPTIONS", "halt_on_error=1"),
    );
    preserve(
        "managed-sanitized",
        &sanitized_run,
        &measurements(&sanitized_run, false),
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn private_publish_receipt_runloop_wake_and_template_growth_are_measured_and_accounted() {
    let root = std::env::temp_dir().join(format!("rinzu-private-perf-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    for (name, sanitizer) in [
        ("protocol-signed", None),
        ("protocol-sanitized", Some("address,undefined")),
        ("protocol-tsan", Some("thread")),
    ] {
        let binary = root.join(name);
        let mut compiler = Command::new("clang");
        compiler.args([
            "-std=c11", "-O2", "-g", "-Wall", "-Wextra", "-Werror", "-pthread",
        ]);
        if let Some(sanitizer) = sanitizer {
            compiler
                .arg(format!("-fsanitize={sanitizer}"))
                .arg("-fno-omit-frame-pointer");
        }
        platform::success(
            &compiler
                .arg(manifest.join("tests/fixtures/native_performance_sdk/protocol.c"))
                .arg(manifest.join("../runtime/src/owner_dispatcher.c"))
                .arg(manifest.join("../runtime/src/callback_entry.c"))
                .arg(manifest.join("../runtime/src/callback_template.S"))
                .arg("-I")
                .arg(manifest.join("../../include"))
                .args(["-framework", "CoreFoundation", "-o"])
                .arg(&binary)
                .output()
                .unwrap(),
        );
        if sanitizer.is_none() {
            platform::success(
                &Command::new("codesign")
                    .args(["--force", "--sign", "-", "--options", "runtime"])
                    .arg(&binary)
                    .output()
                    .unwrap(),
            );
            let entitlements = Command::new("codesign")
                .args(["-d", "--entitlements", "-"])
                .arg(&binary)
                .output()
                .unwrap();
            platform::success(&entitlements);
            assert!(entitlements.stdout.is_empty());
        }
        let output = platform::bounded(
            Command::new(&binary)
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .env("UBSAN_OPTIONS", "halt_on_error=1")
                .env("TSAN_OPTIONS", "halt_on_error=1"),
        );
        platform::success(&output);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let numbers = |prefix: &str| -> Vec<u64> {
            stdout
                .lines()
                .find_map(|line| line.strip_prefix(prefix))
                .unwrap()
                .split(',')
                .map(|field| field.parse().unwrap())
                .collect()
        };
        let queue = numbers("RINZU_PROTOCOL_PERF=");
        assert_eq!(queue.len(), 10);
        assert_eq!(queue[2], 1024);
        assert!(queue[0] > 0 && queue[1] > 0 && queue[3] > 0 && queue[4] > 0 && queue[5] > 0);
        assert_eq!(&queue[6..], &[0, 0, 0, 0]);
        let wake = numbers("RINZU_OWNER_WAKE_PERF=");
        assert!(wake[0] > 0);
        assert_eq!(wake[1], 128);
        assert!(wake[2] >= 128 && wake[3] <= wake[2] && wake[4] > 0);
        let template = numbers("RINZU_TEMPLATE_PERF=");
        assert!(template[0] > 0);
        assert_eq!(template[1], 1537);
        assert!(
            template[2] > 1
                && template[3] > 0
                && template[4] > 0
                && template[6] > 0
                && template[7] > 0
        );
        assert_eq!(template[5], 1537);
        assert_eq!(&template[8..], &[0, 0, 0, 0]);
        preserve(
            name,
            &output,
            &serde_json::json!({"queue":queue,"ownerWake":wake,"template":template,"temporaryBaseline":true,"hasJitEntitlements":false}),
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}
