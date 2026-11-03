#![cfg(unix)]
use rinzu_runtime::foreign_queue::ForeignQueue;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

unsafe extern "C" fn notify(context: *mut core::ffi::c_void) {
    // SAFETY: tests pin these native atomics until every producer joins.
    unsafe { &*context.cast::<AtomicUsize>() }.fetch_add(1, Ordering::Relaxed);
}

#[test]
fn final_record_wake_pins_queue_until_notification_returns() {
    struct Wake {
        calls: AtomicUsize,
        receipt: std::sync::Barrier,
        receipt_release: std::sync::Barrier,
        final_wake: std::sync::Barrier,
        release: std::sync::Barrier,
    }
    unsafe extern "C" fn blocked_final(context: *mut core::ffi::c_void) {
        // SAFETY: native synchronization metadata stays pinned until join.
        let wake = unsafe { &*context.cast::<Wake>() };
        let call = wake.calls.fetch_add(1, Ordering::AcqRel);
        if call == 1 {
            wake.receipt.wait();
            wake.receipt_release.wait();
        } else if call == 2 {
            wake.final_wake.wait();
            wake.release.wait();
        }
    }
    let wake = Box::new(Wake {
        calls: AtomicUsize::new(0),
        receipt: std::sync::Barrier::new(2),
        receipt_release: std::sync::Barrier::new(2),
        final_wake: std::sync::Barrier::new(2),
        release: std::sync::Barrier::new(2),
    });
    // SAFETY: wake references only thread-safe native synchronization objects.
    let queue = unsafe {
        ForeignQueue::new(
            Some(blocked_final),
            std::ptr::from_ref(&*wake).cast_mut().cast(),
        )
    };
    let producer = queue.producer();
    let thread = std::thread::spawn(move || {
        assert_eq!(
            // SAFETY: model supplies admission and immutable inline ABI bits.
            unsafe { producer.invoke(17, 42, &[7], None, std::ptr::null_mut()) },
            41
        );
    });
    let invocation = loop {
        if let Some(invocation) = queue.pop() {
            break invocation;
        }
        std::thread::yield_now();
    };
    invocation.complete(41);
    wake.receipt.wait();
    wake.receipt_release.wait();
    wake.final_wake.wait();
    let stats = queue.stats();
    assert_eq!(
        (stats.live, stats.bytes, stats.queued, stats.wake_pins),
        (0, 0, 0, 1)
    );
    assert!(
        !stats.is_baseline(),
        "a dispatcher may still be touched by native wake"
    );
    wake.release.wait();
    thread.join().unwrap();
    assert!(queue.stats().is_baseline());
}

#[test]
fn native_producers_park_until_fifo_owner_results_and_admission_receipts() {
    let wake = Box::new(AtomicUsize::new(0));
    let receipts = Box::new(AtomicUsize::new(0));
    // SAFETY: atomics are thread-safe native metadata, pinned through queue Drop.
    let queue =
        unsafe { ForeignQueue::new(Some(notify), std::ptr::from_ref(&*wake).cast_mut().cast()) };
    let receipt = std::ptr::from_ref(&*receipts) as usize;
    let workers = (0..8)
        .map(|producer| {
            let queue = queue.producer();
            std::thread::spawn(move || {
                for round in 0..64 {
                    let native = Box::new([0u8, 0xAA, 0, 0x7F]);
                    let id = producer * 1000 + round;
                    let words = [id, native.as_ptr() as usize as u64, 0x7ff8_1234_5678_9abc];
                    // SAFETY: model admission is held through synchronous receipt;
                    // original native storage remains alive until invoke returns.
                    let result = unsafe {
                        queue.invoke(
                            producer + 17,
                            42,
                            &words,
                            Some(notify),
                            receipt as *mut core::ffi::c_void,
                        )
                    };
                    assert_eq!(result, if round % 3 == 0 { 0 } else { id ^ 0x55AA });
                    assert_eq!(*native, [0, 0xAA, 0, 0x7F]);
                }
            })
        })
        .collect::<Vec<_>>();
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut sequence = 0;
    while sequence < 512 {
        if let Some(invocation) = queue.pop() {
            sequence += 1;
            assert_eq!(invocation.sequence(), sequence);
            assert!(invocation.identity() > 0);
            let words = invocation.words();
            let id = words[0];
            let producer = id / 1000;
            let round = id % 1000;
            assert_eq!(invocation.callback(), producer + 17);
            assert_eq!(invocation.generation(), 42);
            assert_eq!(words[2], 0x7ff8_1234_5678_9abc);
            // SAFETY: the synchronous native producer pins this exact buffer.
            let native = unsafe { std::slice::from_raw_parts(words[1] as usize as *const u8, 4) };
            assert_eq!(native, [0, 0xAA, 0, 0x7F]);
            invocation.complete(if round % 3 == 0 { 0 } else { id ^ 0x55AA });
        } else {
            assert!(
                Instant::now() < deadline,
                "owner queue stalled: {:?}",
                queue.stats()
            );
            std::thread::yield_now();
        }
    }
    for worker in workers {
        worker.join().unwrap();
    }
    let stats = queue.stats();
    assert!(stats.is_baseline(), "{stats:?}");
    assert_eq!(
        (
            stats.created,
            stats.published,
            stats.received,
            stats.retired
        ),
        (512, 512, 512, 512)
    );
    assert_eq!(receipts.load(Ordering::Relaxed), 512);
    assert_eq!(wake.load(Ordering::Relaxed), 1536);
    assert!(queue.pop().is_none());
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn c_records_pass_unbounded_growth_reference_races_signed_and_sanitizer_gates() {
    use std::process::Command;
    let root = std::env::temp_dir().join(format!("rinzu-foreign-queue-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for sanitized in [false, true] {
        let binary = root.join(if sanitized { "sanitized" } else { "signed" });
        let mut clang = Command::new("clang");
        clang
            .args([
                "-std=c11", "-O2", "-g", "-Wall", "-Wextra", "-Werror", "-pthread",
            ])
            .arg(manifest.join("tests/fixtures/foreign_queue.c"))
            .arg("-I")
            .arg(manifest.join("../../include"));
        if sanitized {
            clang.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
        }
        let compiled = clang.arg("-o").arg(&binary).output().unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        if !sanitized {
            let signed = Command::new("codesign")
                .args(["--force", "--sign", "-", "--options", "runtime"])
                .arg(&binary)
                .output()
                .unwrap();
            assert!(signed.status.success(), "{signed:?}");
            assert!(
                Command::new("codesign")
                    .args(["--verify", "--strict"])
                    .arg(&binary)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
        let run = |mode: &str| {
            Command::new(&binary)
                .arg(mode)
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .output()
                .unwrap()
        };
        let success = run("normal");
        assert!(success.status.success(), "{success:?}");
        assert!(
            String::from_utf8_lossy(&success.stdout)
                .contains("PASS 4096 admitted records and FIFO/result/reference settlement")
        );
        for mode in [
            "foreign-pop",
            "foreign-complete",
            "duplicate-publish",
            "duplicate-complete",
            "duplicate-receive",
            "prepared-release",
            "queued-release",
            "drop-live",
            "bad-identity",
            "bad-generation",
        ] {
            let rejected = run(mode);
            assert_eq!(rejected.status.code(), Some(86), "{mode}: {rejected:?}");
            assert!(rejected.stderr.is_empty(), "{mode}: {rejected:?}");
        }
        for mode in ["allocate-queue", "allocate-record", "overflow"] {
            let fatal = run(mode);
            assert_eq!(fatal.status.code(), Some(71), "{mode}: {fatal:?}");
            assert!(fatal.stderr.is_empty(), "{mode}: {fatal:?}");
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
