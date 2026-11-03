#![cfg(unix)]
use rinzu_runtime::foreign_queue::ForeignQueue;
use rinzu_runtime::reactor::AsyncReactor;
use rinzu_runtime::resources::ResourceLimits;
use rinzu_runtime::scheduler::{ResolveResult, TaskId};
use rinzu_runtime::standalone::OperationMailbox;
use rinzu_runtime::worker::CpuWorkerPool;
use std::sync::{Arc, Barrier};
use std::time::Duration;

#[test]
fn io_and_worker_publish_before_wake_and_pin_native_target_until_notification_finishes() {
    struct Wake {
        entered: Barrier,
        release: Barrier,
    }
    unsafe extern "C" fn blocked(raw: *mut core::ffi::c_void) {
        // SAFETY: native synchronization metadata is pinned through helper join.
        let wake = unsafe { &*raw.cast::<Wake>() };
        wake.entered.wait();
        wake.release.wait();
    }
    let wake = Box::new(Wake {
        entered: Barrier::new(2),
        release: Barrier::new(2),
    });
    // SAFETY: the callback uses only barriers; no managed heap or Owner access.
    let queue =
        unsafe { ForeignQueue::new(Some(blocked), std::ptr::from_ref(&*wake).cast_mut().cast()) };
    let producer = queue.producer();
    let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || producer.wake());
    let reactor = AsyncReactor::with_completion_wake(Arc::clone(&notify));
    let workers = CpuWorkerPool::with_completion_wake(notify);
    let mut mailbox = OperationMailbox::new(ResourceLimits::default(), Arc::new(|| {}));
    assert!(reactor.try_completion().unwrap().is_none());
    assert!(workers.try_completion().unwrap().is_none());
    let file = reactor
        .submit_file_read(
            mailbox.admit(TaskId(1), "wake file".into()).unwrap(),
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/async.txt"),
        )
        .unwrap();
    wake.entered.wait();
    assert_eq!(queue.stats().wake_pins, 1);
    assert!(!queue.stats().is_baseline());
    let completion = reactor.try_completion().unwrap().unwrap();
    assert_eq!(file.take().unwrap().unwrap(), b"rinzu-async-fixture\n");
    mailbox.take_resolutions().unwrap();
    mailbox.settle(completion.operation).unwrap();
    wake.release.wait();
    drop(reactor);
    let (started, start) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let result = workers
        .spawn(
            mailbox
                .admit(TaskId(2), "cancelled wake Worker".into())
                .unwrap(),
            move |_domain| {
                started.send(()).unwrap();
                released.recv().unwrap();
                Ok(41_i64.to_le_bytes().to_vec())
            },
        )
        .unwrap();
    start.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(result.request_cancel(), ResolveResult::Won);
    assert!(
        workers.try_completion().unwrap().is_none(),
        "cancel is separate from physical completion"
    );
    release.send(()).unwrap();
    wake.entered.wait();
    let completion = workers.try_completion().unwrap().unwrap();
    assert_eq!(completion.resolution, ResolveResult::Late);
    assert_eq!(result.take().unwrap().unwrap().bytes, 41_i64.to_le_bytes());
    assert_eq!(queue.stats().wake_pins, 1);
    mailbox.take_resolutions().unwrap();
    mailbox.settle(completion.operation).unwrap();
    assert!(mailbox.resources().used.is_baseline());
    wake.release.wait();
    drop(workers);
    assert!(queue.stats().is_baseline());
    assert_eq!(
        queue.stats().created,
        0,
        "completion wakes allocate no callback records"
    );
}

#[test]
#[cfg(target_os = "macos")]
fn c_completion_wake_and_nonblocking_collection_preserve_physical_return_boundary() {
    use std::process::Command;
    let root = std::env::temp_dir().join(format!("rinzu-completion-wake-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for sanitized in [false, true] {
        let binary = root.join(if sanitized { "sanitized" } else { "signed" });
        let mut clang = Command::new("clang");
        clang
            .args([
                "-std=c11", "-O2", "-g", "-Wall", "-Wextra", "-Werror", "-pthread", "-I",
            ])
            .arg(manifest.join("../../include"))
            .arg(manifest.join("tests/fixtures/completion_wake.c"));
        if sanitized {
            clang.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
        }
        let output = clang.arg("-o").arg(&binary).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        if !sanitized {
            let output = Command::new("codesign")
                .args(["--force", "--sign", "-", "--options", "runtime"])
                .arg(&binary)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
        }
        for mode in ["normal", "late-config", "foreign-poll"] {
            let output = Command::new(&binary)
                .arg(mode)
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .env("UBSAN_OPTIONS", "halt_on_error=1")
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(if mode == "normal" { 0 } else { 70 }),
                "{mode}: {output:?}"
            );
            assert!(output.stderr.is_empty(), "{mode}: {output:?}");
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
