#![cfg(all(
    feature = "test-native-lifetime-trace",
    any(target_os = "macos", target_os = "linux")
))]

#[path = "../../../tests/support/native.rs"]
mod native;
#[path = "../../../tests/support/process.rs"]
mod process;

use rinzu_runtime::lifetime_trace::{DomainTrace, Environment, Probe};
use rinzu_runtime::native_completion::{
    CompletionPointer, NotifiedNativeCompletion, TransferNativeCompletion,
};
use rinzu_runtime::reactor::AsyncReactor;
use rinzu_runtime::scheduler::{Resolution, ResolveResult, TaskId};
use rinzu_runtime::standalone::OperationMailbox;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, mpsc};
use std::time::Duration;

#[test]
fn rust_shutdown_retains_unpublished_receipts_until_provider_completion() {
    let trace = Arc::new(DomainTrace::default());
    let probe = trace.probe(0);
    let environment = Environment(Arc::clone(&probe));
    let reactor = AsyncReactor::with_lifetime_trace(
        Arc::new(move || {
            std::hint::black_box(&environment);
        }),
        Arc::clone(&probe),
    );
    let mut mailbox = OperationMailbox::new(Default::default(), Arc::new(|| {}));
    let carrier = Box::new(TransferNativeCompletion {
        result: 4456,
        ..Default::default()
    });
    let address = std::ptr::from_ref(&carrier.completion) as usize;
    // SAFETY: the test retains this provider allocation through both physical copies and shutdown.
    let pointer = unsafe { CompletionPointer::new(address) };
    let mut ids = Vec::new();
    let receipts: [_; 2] = std::array::from_fn(|_| {
        let ticket = mailbox.admit(TaskId(1), "shutdown drain".into()).unwrap();
        ids.push(ticket.operation);
        reactor.submit_native_transfer(ticket, pointer).unwrap()
    });
    probe.wait("registered", 2);
    assert_eq!(receipts[0].request_cancel(), ResolveResult::Won);
    assert_eq!(receipts[1].request_timeout(), ResolveResult::Won);

    let (finished_sender, finished) = mpsc::channel();
    let owner = std::thread::spawn(move || {
        let mut reactor = reactor;
        reactor.shutdown();
        finished_sender.send(reactor).unwrap();
    });
    probe.wait("shutdown", 1);
    assert!(matches!(
        finished.recv_timeout(Duration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    assert!(!pointer.is_done());
    assert!(probe.events().iter().all(|row| {
        ["physical,", "joined,", "env_free,", "target_free,"]
            .iter()
            .all(|prefix| !row.starts_with(prefix))
    }));

    let _sdk = Probe::producer(100);
    // SAFETY: the SDK completes once, after the wait checkpoint, while both receipts remain pinned.
    let captured = unsafe {
        NotifiedNativeCompletion::capture_with_lifetime_trace(
            address as *mut NotifiedNativeCompletion,
            &probe,
        )
    }
    .unwrap();
    captured.notify();
    let reactor = finished.recv_timeout(Duration::from_secs(30)).unwrap();
    owner.join().unwrap();
    assert!(pointer.is_done());
    let mut received = [false; 2];
    for _ in &ids {
        let id = reactor
            .wait_completion(Duration::from_secs(30))
            .unwrap()
            .operation;
        let index = ids.iter().position(|&expected| expected == id).unwrap();
        assert!(!std::mem::replace(&mut received[index], true));
        probe.wait_for("physical", id.0, 1);
    }
    mailbox.take_resolutions().unwrap();
    assert_eq!(mailbox.settle(ids[0]).unwrap(), Resolution::Cancelled);
    assert_eq!(mailbox.settle(ids[1]).unwrap(), Resolution::TimedOut);
    for receipt in receipts {
        assert_eq!(receipt.native_transfer_result(), Some(4456));
    }
    drop(carrier);
    drop(reactor);
    probe.wait("env_free", 1);
    assert!(
        !probe
            .events()
            .iter()
            .any(|row| row.starts_with("target_free,"))
    );
    let target = captured.address();
    captured.notify();
    drop(captured);
    probe.wait("target_free", 1);
    assert!(
        probe
            .events()
            .contains(&format!("producer_refs,100,{target},1"))
    );
    println!("UNPUBLISHED DRAIN Rust joins only after SDK v3 publication");
}

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn c_owner_wait_retains_unpublished_polling_and_notified_receipts() {
    let scratch = Scratch(
        std::env::temp_dir().join(format!("rinzu-unpublished-drain-{}", std::process::id())),
    );
    std::fs::create_dir(&scratch.0).unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for sanitized in [false, true] {
        let binary = scratch.0.join("unpublished-drain");
        let mut compiler = native::clang();
        compiler
            .args([
                "-std=c11", "-O1", "-g", "-pthread", "-Wall", "-Wextra", "-Werror",
            ])
            .arg("-I")
            .arg(root.join("../../include"))
            .arg(root.join("tests/fixtures/unpublished_drain.c"))
            .arg("-o")
            .arg(&binary);
        if sanitized {
            compiler.args(["-fsanitize=address,undefined", "-fno-sanitize-recover=all"]);
        }
        let built = process::bounded_tool(&mut compiler);
        assert!(built.status.success(), "{built:?}");
        let result = process::bounded(
            Command::new(&binary)
                .env("RINZU_NATIVE_COMPLETION_NOTIFY", "1")
                .env("RINZU_NATIVE_COMPLETION_DIRECT", "0")
                .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
                .env("UBSAN_OPTIONS", "halt_on_error=1"),
        );
        assert!(result.status.success(), "{result:?}");
        assert_eq!(
            result.stdout,
            b"UNPUBLISHED DRAIN C waits for SDK v1/v3 publication\n"
        );
        assert!(result.stderr.is_empty(), "{result:?}");
        println!("UNPUBLISHED DRAIN C sanitized={sanitized} waits for SDK v1/v3 publication");
    }
}
