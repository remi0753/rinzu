#![cfg(unix)]
use rinzu_runtime::{
    failure::SourcePosition,
    native_diagnostics::{ActivityKind, ActivityMonitor},
};
#[test]
fn rust_intervals_copy_native_metadata_and_settle_prepared_and_running_records() {
    let monitor = ActivityMonitor::new();
    let prepared = monitor.prepare(
        ActivityKind::Native,
        c"rust.native",
        SourcePosition {
            file: 7,
            start_byte: 9,
            end_byte: 12,
        },
        41,
        3,
    );
    assert_eq!(monitor.stats().active, 0);
    drop(prepared);
    assert!(monitor.stats().is_quiescent());
    let running = monitor.prepare(
        ActivityKind::Managed,
        c"rust.managed",
        SourcePosition {
            file: 8,
            start_byte: 3,
            end_byte: 4,
        },
        41,
        2,
    );
    running.start();
    std::thread::sleep(std::time::Duration::from_millis(90));
    let stats = monitor.stats();
    assert_eq!(stats.warnings, [1, 0, 0, 0, 0]);
    assert_eq!(stats.active, 1);
    drop(running);
    drop(monitor);
}
#[test]
fn prepared_native_pair_abandons_without_start_and_settles_cross_thread_request() {
    let monitor = ActivityMonitor::new();
    let source = SourcePosition {
        file: 17,
        start_byte: 31,
        end_byte: 49,
    };
    let native = monitor.prepare_native(c"rust.paired", source, 41, 2);
    assert_eq!(monitor.stats().prepared, 2);
    assert_eq!(monitor.stats().active, 0);
    drop(native);
    assert!(monitor.stats().is_quiescent());
    let native = monitor.prepare_native(c"rust.running", source, 41, 2);
    native.start();
    assert_eq!(monitor.stats().active, 1);
    let requester = monitor.clone();
    std::thread::spawn(move || {
        requester.request_shutdown();
        requester.request_shutdown();
    })
    .join()
    .unwrap();
    assert_eq!(monitor.stats().active, 2);
    drop(native);
    assert!(monitor.stats().is_quiescent());
    monitor.request_shutdown();
    assert!(monitor.stats().is_quiescent());
    monitor.owner_turn();
    let next = monitor.prepare_native(c"rust.next-task", source, 41, 2);
    next.start();
    assert_eq!(monitor.stats().active, 1);
    drop(next);
    assert!(monitor.stats().is_quiescent());
}
#[test]
fn worker_ticket_delivers_native_requests_before_registration_and_during_physical_call() {
    use rinzu_runtime::{
        resources::ResourceLimits, scheduler::TaskId, standalone::OperationMailbox,
        worker::CpuWorkerPool,
    };
    use std::sync::{Arc, mpsc};
    for early in [false, true] {
        let mut mailbox = OperationMailbox::new(ResourceLimits::default(), Arc::new(|| {}));
        let ticket = mailbox.admit(TaskId(1), "native Worker".into()).unwrap();
        let operation = ticket.operation;
        let observer = ticket.clone();
        let pool = CpuWorkerPool::new();
        let (registered, registration) = mpsc::channel();
        let (started, running) = mpsc::channel();
        let (proceed, input) = mpsc::channel();
        let (entered, entry) = mpsc::channel();
        let (returned, physical) = mpsc::channel();
        let result = pool
            .spawn(ticket, move |_| {
                started.send(()).unwrap();
                input.recv().unwrap();
                let monitor = ActivityMonitor::new();
                let registration = observer.observe_native_cancellation(monitor.clone());
                let native = monitor.prepare_native(
                    c"worker.blocked",
                    SourcePosition {
                        file: 17,
                        start_byte: 31,
                        end_byte: 49,
                    },
                    41,
                    2,
                );
                native.start();
                registered.send(monitor.clone()).unwrap();
                entered.send(()).unwrap();
                physical.recv().unwrap();
                drop(native);
                drop(registration);
                Ok(vec![41])
            })
            .unwrap();
        running.recv().unwrap();
        if early {
            result.request_timeout();
        }
        proceed.send(()).unwrap();
        entry.recv().unwrap();
        let monitor = registration.recv().unwrap();
        if !early {
            result.request_cancel();
        }
        for _ in 0..64 {
            result.request_cancel();
        }
        assert_eq!(monitor.stats().active, 2);
        assert!(result.take().is_none());
        assert_eq!(mailbox.take_resolutions().unwrap().len(), 1);
        assert!(!mailbox.resources().used.is_baseline());
        returned.send(()).unwrap();
        let packet = pool
            .wait_completion(std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(packet.operation, operation);
        result.request_cancel(); // The detached target cannot rearm a returned Worker.
        assert!(monitor.stats().is_quiescent());
        assert_eq!(result.take().unwrap().unwrap().bytes, vec![41]);
        mailbox.settle(operation).unwrap();
        assert!(mailbox.resources().used.is_baseline());
        drop(pool);
    }
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn signed_and_sanitized_worker_requests_serialize_with_registration_and_detachment() {
    use std::{path::Path, process::Command};
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root =
        std::env::temp_dir().join(format!("rinzu-worker-native-cancel-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    for sanitized in [false, true] {
        let binary = root.join(if sanitized { "sanitized" } else { "signed" });
        let mut clang = Command::new("clang");
        clang
            .args([
                "-std=c11", "-O1", "-g", "-Wall", "-Wextra", "-Werror", "-pthread",
            ])
            .arg(manifest.join("tests/fixtures/worker_native_cancellation.c"))
            .arg("-I")
            .arg(manifest.join("../../include"));
        if sanitized {
            clang.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
        }
        let compiled = clang.arg("-o").arg(&binary).output().unwrap();
        assert!(compiled.status.success(), "{compiled:?}");
        if !sanitized {
            for args in [
                ["--force", "--sign", "-", "--options", "runtime"].as_slice(),
                ["--verify", "--strict"].as_slice(),
            ] {
                let signed = Command::new("codesign")
                    .args(args)
                    .arg(&binary)
                    .output()
                    .unwrap();
                assert!(signed.status.success(), "{signed:?}");
            }
            let entitlements = Command::new("codesign")
                .args(["-d", "--entitlements", "-"])
                .arg(&binary)
                .output()
                .unwrap();
            assert!(entitlements.status.success() && entitlements.stdout.is_empty());
        }
        let run = Command::new(&binary)
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .env("UBSAN_OPTIONS", "halt_on_error=1")
            .output()
            .unwrap();
        assert!(run.status.success(), "{run:?}");
        assert!(String::from_utf8_lossy(&run.stdout).contains("PASS native-only Worker"));
        assert!(!String::from_utf8_lossy(&run.stderr).contains("runtime error:"));
    }
    std::fs::remove_dir_all(root).unwrap();
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn signed_and_sanitized_watchdog_warns_while_owner_is_blocked_and_pins_after_settlement() {
    use std::{path::Path, process::Command};
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root =
        std::env::temp_dir().join(format!("rinzu-native-diagnostics-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    for sanitized in [false, true] {
        let binary = root.join(if sanitized { "sanitized" } else { "signed" });
        let mut clang = Command::new("clang");
        clang
            .args([
                "-std=c11",
                "-O1",
                "-g",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-pthread",
                "-DRINZU_ACTIVITY_TESTING",
            ])
            .arg(manifest.join("tests/fixtures/native_diagnostics.c"))
            .arg("-I")
            .arg(manifest.join("../../include"));
        if sanitized {
            clang.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
        }
        let compiled = clang.arg("-o").arg(&binary).output().unwrap();
        assert!(compiled.status.success(), "{compiled:?}");
        if !sanitized {
            for args in [
                ["--force", "--sign", "-", "--options", "runtime"].as_slice(),
                ["--verify", "--strict"].as_slice(),
            ] {
                let signed = Command::new("codesign")
                    .args(args)
                    .arg(&binary)
                    .output()
                    .unwrap();
                assert!(signed.status.success(), "{signed:?}");
            }
            let entitlements = Command::new("codesign")
                .args(["-d", "--entitlements", "-"])
                .arg(&binary)
                .output()
                .unwrap();
            assert!(entitlements.status.success() && entitlements.stdout.is_empty());
        }
        let run = |mode| {
            Command::new(&binary)
                .arg(mode)
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .env("UBSAN_OPTIONS", "halt_on_error=1")
                .output()
                .unwrap()
        };
        let normal = run("normal");
        assert!(normal.status.success(), "{normal:?}");
        let progress = run("recent-progress");
        assert!(progress.status.success(), "{progress:?}");
        let boundary = run("owner-turn-boundaries");
        assert!(boundary.status.success(), "{boundary:?}");
        let adjusted = run("adjusted-clock");
        assert!(adjusted.status.success(), "{adjusted:?}");
        let warnings = String::from_utf8_lossy(&normal.stderr);
        for name in [
            "long-managed",
            "long-native",
            "foreign-result-wait",
            "owner-stall",
            "shutdown-blocker",
        ] {
            assert_eq!(
                warnings.matches(&format!("rinzu warning: {name} ")).count(),
                1,
                "{warnings}"
            );
        }
        assert_eq!(
            warnings
                .matches("source=3:12:19 owner=0x1234 depth=7")
                .count(),
            5
        );
        assert!(!warnings.contains("runtime error:"));
        let pin = run("pin");
        assert!(pin.status.success(), "{pin:?}");
        assert!(String::from_utf8_lossy(&pin.stdout).contains("warning pin"));
        let paired = run("paired");
        assert!(paired.status.success(), "{paired:?}");
        let warnings = String::from_utf8_lossy(&paired.stderr);
        let blocker = warnings
            .lines()
            .find(|line| {
                line.contains(
                    "shutdown-blocker symbol=pair.blocked source=17:31:49 owner=0x29 depth=2 ",
                )
            })
            .unwrap_or_else(|| panic!("{warnings}"));
        assert!(blocker.contains("stack=pair.blocked"), "{blocker}");
        assert_eq!(
            warnings.matches("shutdown-blocker ").count(),
            1,
            "{warnings}"
        );
        for (mode, code) in [
            ("oom-board", 71),
            ("oom-record", 71),
            ("oom-pair-first", 71),
            ("oom-pair-second", 71),
            ("active-dispose", 86),
            ("double-start", 86),
            ("wrong-thread", 86),
            ("invalid-kind", 86),
        ] {
            let negative = run(mode);
            assert_eq!(negative.status.code(), Some(code), "{mode}: {negative:?}");
            assert!(
                negative.stdout.is_empty() && negative.stderr.is_empty(),
                "{negative:?}"
            );
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn retired_record_storage_is_explicit_and_distinct_from_quiescence() {
    let monitor = ActivityMonitor::new();
    assert!(monitor.stats().is_baseline());
    let intervals: Vec<_> = (0..128)
        .map(|_| {
            monitor.prepare(
                ActivityKind::Managed,
                c"cache.bound",
                SourcePosition {
                    file: 0,
                    start_byte: 0,
                    end_byte: 0,
                },
                41,
                2,
            )
        })
        .collect();
    assert_eq!(monitor.stats().live, 128);
    drop(intervals);
    let stats = monitor.stats();
    assert!(stats.is_quiescent());
    assert!(!stats.is_baseline());
    assert_eq!(stats.cached_records, 64);
    assert_eq!(stats.cached_bytes, 64 * 768);
    #[cfg(target_pointer_width = "64")]
    {
        assert_eq!(std::mem::size_of_val(&stats), 136);
        assert_eq!(
            std::mem::offset_of!(
                rinzu_runtime::native_diagnostics::ActivityStats,
                cached_records
            ),
            120
        );
        assert_eq!(
            std::mem::offset_of!(
                rinzu_runtime::native_diagnostics::ActivityStats,
                cached_bytes
            ),
            128
        );
    }
}
