#![cfg(unix)]
use rinzu_runtime::{failure::SourcePosition, native_diagnostics::ActivityMonitor};
fn source() -> SourcePosition {
    SourcePosition {
        file: 11,
        start_byte: 31,
        end_byte: 49,
    }
}
#[test]
fn receipt_before_physical_notification_never_recreates_pending_work() {
    let monitor = ActivityMonitor::new();
    let receipt = monitor.prepare_ready(c"early.packet", source(), 41, 2);
    let publication = receipt.publication();
    receipt.receive();
    let late = std::thread::spawn(move || publication.publish());
    late.join().unwrap();
    assert!(monitor.stats().is_quiescent());
}
#[test]
fn cancelled_worker_publishes_readiness_only_after_real_packet_and_settles_on_owner() {
    use rinzu_runtime::{
        resources::ResourceLimits, scheduler::TaskId, standalone::OperationMailbox,
        worker::CpuWorkerPool,
    };
    use std::sync::{Arc, Barrier, mpsc};
    let monitor = ActivityMonitor::new();
    let receipt = monitor.prepare_ready(c"worker.packet", source(), 41, 2);
    let mut mailbox = OperationMailbox::new(ResourceLimits::default(), Arc::new(|| {}));
    let mut ticket = mailbox.admit(TaskId(1), "worker".into()).unwrap();
    let operation = ticket.operation;
    ticket.set_readiness(receipt.publication());
    let published = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let wake = {
        let published = published.clone();
        let release = release.clone();
        Arc::new(move || {
            published.wait();
            release.wait();
        }) as Arc<dyn Fn() + Send + Sync>
    };
    let pool = CpuWorkerPool::with_completion_wake(wake);
    let (finished, physical) = mpsc::channel();
    let (entered, body) = mpsc::channel();
    let result = pool
        .spawn(ticket, move |_| {
            entered.send(()).unwrap();
            physical.recv().unwrap();
            Ok(vec![23])
        })
        .unwrap();
    body.recv().unwrap();
    result.request_cancel();
    assert_eq!(mailbox.take_resolutions().unwrap().len(), 1);
    assert_eq!(monitor.stats().active, 0);
    finished.send(()).unwrap();
    published.wait();
    let packet = pool
        .wait_completion(std::time::Duration::from_secs(5))
        .unwrap();
    assert_eq!(packet.operation, operation);
    assert_eq!(monitor.stats().active, 1);
    receipt.receive();
    assert_eq!(monitor.stats().active, 0);
    assert_eq!(result.take().unwrap().unwrap().bytes, vec![23]);
    mailbox.settle(operation).unwrap();
    drop(result);
    release.wait();
    drop(pool);
    assert!(monitor.stats().is_quiescent());
    assert!(mailbox.resources().used.is_baseline());
}
#[test]
fn real_file_packet_receipt_and_late_publication_return_all_native_readiness() {
    use rinzu_runtime::{
        reactor::AsyncReactor, resources::ResourceLimits, scheduler::TaskId,
        standalone::OperationMailbox,
    };
    use std::sync::Arc;
    let monitor = ActivityMonitor::new();
    let receipt = monitor.prepare_ready(c"file.packet", source(), 41, 2);
    let mut mailbox = OperationMailbox::new(ResourceLimits::default(), Arc::new(|| {}));
    let mut ticket = mailbox.admit(TaskId(1), "file".into()).unwrap();
    let operation = ticket.operation;
    ticket.set_readiness(receipt.publication());
    let path = std::env::temp_dir().join(format!("rinzu-ready-file-{}", std::process::id()));
    std::fs::write(&path, b"native packet").unwrap();
    let reactor = AsyncReactor::new();
    let result = reactor.submit_file_read(ticket, path.clone()).unwrap();
    let packet = reactor
        .wait_completion(std::time::Duration::from_secs(5))
        .unwrap();
    assert_eq!(packet.operation, operation);
    assert_eq!(result.take().unwrap().unwrap(), b"native packet");
    receipt.receive();
    mailbox.take_resolutions().unwrap();
    mailbox.settle(operation).unwrap();
    drop(result);
    drop(reactor);
    assert!(monitor.stats().is_quiescent());
    assert!(mailbox.resources().used.is_baseline());
    std::fs::remove_file(path).unwrap();
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn signed_and_sanitized_ready_records_distinguish_progress_from_blocked_owner() {
    use std::{path::Path, process::Command};
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = std::env::temp_dir().join(format!("rinzu-ready-work-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    for sanitized in [false, true] {
        let binary = root.join(if sanitized { "sanitized" } else { "signed" });
        let mut clang = Command::new("clang");
        clang
            .args([
                "-std=c11", "-O1", "-g", "-Wall", "-Wextra", "-Werror", "-pthread",
            ])
            .arg(manifest.join("tests/fixtures/ready_work.c"))
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
        let warnings = String::from_utf8_lossy(&normal.stderr);
        assert_eq!(
            warnings
                .matches("owner-stall symbol=physical.packet source=11:31:49 ")
                .count(),
            1,
            "{warnings}"
        );
        assert!(
            warnings.contains("stack=owner.loop") && !warnings.contains("runtime error:"),
            "{warnings}"
        );
        for (mode, status) in [
            ("oom-ready", 71),
            ("oom-interval", 71),
            ("wrong-receive", 86),
            ("wrong-turn", 86),
            ("double-publish", 86),
            ("double-receive", 86),
            ("unreceived-dispose", 86),
            ("pinned-board-dispose", 86),
        ] {
            let negative = run(mode);
            assert_eq!(negative.status.code(), Some(status), "{mode}: {negative:?}");
            assert!(
                negative.stdout.is_empty() && negative.stderr.is_empty(),
                "{negative:?}"
            );
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
