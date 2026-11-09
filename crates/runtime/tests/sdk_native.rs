#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

#[path = "../../../tests/support/process.rs"]
mod process;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const SANITIZERS: &[&str] = &["", "address,undefined", "thread"];
const PLAIN: &[&str] = &[""];

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let cleanup = std::fs::remove_dir_all(&self.0);
        if !std::thread::panicking() {
            cleanup.unwrap();
        }
    }
}

fn replay(
    sdk: &str,
    fixture: &str,
    extra_inputs: &[&str],
    sanitizers: &[&str],
    marker: &str,
    guards: (i32, &[&str]),
) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let sdk = root.join("sdk").join(sdk);
    let directory =
        std::env::temp_dir().join(format!("rinzu-sdk-{fixture}-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let work = Scratch(directory);
    let binary = work.0.join(fixture);
    let source = sdk.join("tests").join(format!("{fixture}.c"));
    for sanitizer in sanitizers {
        let mut build = Command::new("clang");
        build
            .current_dir(&root)
            .args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread"])
            .arg("-I")
            .arg(root.join("include"))
            .arg("-I")
            .arg(sdk.join("native/include"))
            .arg(&source)
            .args(extra_inputs)
            .arg("-o")
            .arg(&binary);
        if !sanitizer.is_empty() {
            build
                .arg(format!("-fsanitize={sanitizer}"))
                .arg("-fno-sanitize-recover=all");
        }
        let built = process::bounded_tool(&mut build);
        assert!(built.status.success(), "{fixture} ({sanitizer}): {built:?}");

        let run = |mode, expected| {
            let mut command = Command::new(&binary);
            command
                .current_dir(&work.0)
                .stdin(Stdio::null())
                .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
                .env("UBSAN_OPTIONS", "halt_on_error=1")
                .env("TSAN_OPTIONS", "halt_on_error=1")
                .args(mode);
            let result = process::bounded(&mut command);
            let context = format!("{fixture} ({sanitizer}) {mode:?}: {result:?}");
            assert_eq!(result.status.code(), Some(expected), "{context}");
            if expected == 0 {
                assert!(
                    String::from_utf8_lossy(&result.stdout).contains(marker),
                    "{context}"
                );
            }
            let errors = String::from_utf8_lossy(&result.stderr);
            assert!(
                !errors.contains("Sanitizer") && !errors.contains("runtime error:"),
                "{context}"
            );
        };
        run(&[][..], 0);
        for mode in guards.1 {
            run(std::slice::from_ref(mode), guards.0);
        }
    }
}

fn socket(fixture: &str, sanitizers: &[&str], marker: &str, guards: &[&str]) {
    replay("socket", fixture, &[], sanitizers, marker, (86, guards));
}

#[test]
fn socket_owner_io_preserves_leases_and_completion() {
    socket(
        "owner-read",
        SANITIZERS,
        "owner-read:singlecall;binary;partial;transient;",
        &[],
    );
    socket(
        "owner-write",
        SANITIZERS,
        "owner-write:singlecall;snapshot;partial;transient;",
        &[],
    );
    socket(
        "io-outside",
        SANITIZERS,
        "io-outside:48:pending-lease:",
        &["pending-finish", "0", "1", "2", "3", "4", "5"],
    );
}

#[test]
fn socket_wake_coalescing_preserves_receipt_and_queue_generations() {
    socket("wake-coalescing", SANITIZERS, "N08:wake:queued:", &[]);
}

#[test]
fn socket_accounting_and_lookup_enforce_capacity_and_identity() {
    socket("accounting", PLAIN, "accounting:ready-retains:", &[]);
    socket("bounds", PLAIN, "bounds:owners=4,handles=256,", &[]);
    socket(
        "lookup",
        PLAIN,
        "lookup:256-handles:",
        &["exhausted", "slot", "operation", "stale"],
    );
    socket("pending", PLAIN, "pending:operations=768:", &[]);
}

#[test]
fn socket_reads_and_receipts_enforce_length_kind_and_owner() {
    socket(
        "prefix",
        PLAIN,
        "prefix:19;",
        &[
            "small",
            "medium",
            "eight",
            "sixteen",
            "thirty-two",
            "full",
            "pending",
            "owner",
            "stale",
            "kind",
        ],
    );
    socket("read-padding", PLAIN, "read-padding:36;", &[]);
    socket(
        "read-view",
        PLAIN,
        "read-view:19;",
        &["length", "owner", "stale", "pending", "kind"],
    );
    socket(
        "transfer-receipt",
        PLAIN,
        "transfer-receipt:112;",
        &["owner", "stale", "kind"],
    );
    socket(
        "transfer-result",
        PLAIN,
        "transfer-result:112;",
        &["length", "owner", "stale", "pending", "kind"],
    );
}

#[test]
fn socket_loopback_exercises_deadlines_cancellation_and_transfer() {
    socket("boundaries", PLAIN, "connect-pending-cancel:", &[]);
    socket("partial", PLAIN, "partial-prefix:", &[]);
    replay(
        "socket",
        "transport",
        &["sdk/socket/native/src/socket.c"],
        PLAIN,
        "native-binary-cleanup:65536;",
        (0, &[]),
    );
}

#[test]
fn io_standard_streams_and_temporary_files_preserve_ownership() {
    replay("io", "standard", &[], PLAIN, "io-standard-ok", (0, &[]));
    replay(
        "io",
        "temporary",
        &[],
        PLAIN,
        "PASS:io-temporary:",
        (0, &[]),
    );
}

#[test]
fn io_whole_file_preserves_metadata_and_settles_installation_races() {
    replay(
        "io",
        "whole_file",
        &[],
        SANITIZERS,
        "whole-file-native-ok",
        (0, &[]),
    );
}

#[test]
fn process_environment_snapshots_are_thread_local() {
    replay(
        "process",
        "snapshot",
        &["sdk/process/native/process.c"],
        PLAIN,
        "process-snapshot-ok",
        (0, &[]),
    );
}

#[test]
fn sqlite_owners_release_storage_after_deadlines_and_failures() {
    for (fixture, marker) in [
        ("owner-drain", "OWNER_DRAIN_PASS"),
        ("resource", "SQLITE_BASELINE="),
        ("rows-deadline", "ROWS_DEADLINE_PASS"),
    ] {
        replay(
            "sqlite",
            fixture,
            &["sdk/sqlite/native/src/sqlite_source.c", "-lsqlite3"],
            PLAIN,
            marker,
            (0, &[]),
        );
    }
    replay(
        "sqlite",
        "owned-fault",
        &["-lsqlite3"],
        PLAIN,
        "\"live\":0",
        (70, &["oom"]),
    );
}

#[test]
fn shutdown_restores_signal_handlers_and_rejects_foreign_owners() {
    replay(
        "shutdown",
        "control",
        &["-Isdk/shutdown/native", "sdk/shutdown/native/control.c"],
        PLAIN,
        "signals:20,restored:20,",
        (0, &[]),
    );
}
