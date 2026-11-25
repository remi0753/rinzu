#![cfg(all(
    feature = "test-native-lifetime-trace",
    any(target_os = "macos", target_os = "linux")
))]

#[path = "../../../tests/support/native.rs"]
mod native;
#[path = "../../../tests/support/process.rs"]
mod process;

use rinzu_runtime::lifetime_trace::{DomainTrace, Probe};
use rinzu_runtime::native_completion::{
    CompletionPointer, NotifiedNativeCompletion, TransferNativeCompletion,
};
use rinzu_runtime::reactor::{AsyncReactor, ReactorError};
use rinzu_runtime::scheduler::{Resolution, TaskId};
use rinzu_runtime::standalone::OperationMailbox;
use std::process::Command;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

#[test]
fn rust_copies_late_payload_before_any_sdk_notification_or_join() {
    let trace = Arc::new(DomainTrace::default());
    let worker = trace.probe(0);
    let reactor = AsyncReactor::with_lifetime_trace(Arc::new(|| {}), Arc::clone(&worker));
    let mut mailbox = OperationMailbox::new(Default::default(), Arc::new(|| {}));
    for round in 1..=64 {
        let carrier = Box::into_raw(Box::new(TransferNativeCompletion::default()));
        let raw = carrier as usize;
        // SAFETY: the v3 allocation stays pinned through the final exchange and the worker's copy.
        let pointer = unsafe { CompletionPointer::new(raw) };
        let ticket = mailbox.admit(TaskId(1), "v3 late payload".into()).unwrap();
        let id = ticket.operation;
        let receipt = reactor.submit_native_transfer(ticket, pointer).unwrap();
        worker.wait_for("registered", id.0, 1);
        let value = 0x46ab3907_i64 * round + round;
        let (copied, copy_observed) = mpsc::channel();
        let sdk = std::thread::spawn(move || {
            let carrier = raw as *mut TransferNativeCompletion;
            // SAFETY: this is the unique writer. The worker reads the payload only after
            // the final release exchange on the separate notification word.
            unsafe { (&raw mut (*carrier).result).write(value) };
            // The SDK probe is private to this thread, so no test mutex can publish
            // the payload to the worker or the Owner before the SDK exchange.
            let trace = Probe::new();
            // SAFETY: pins are still held and this carrier is published just once.
            let captured = unsafe {
                NotifiedNativeCompletion::capture_with_lifetime_trace(
                    carrier.cast::<NotifiedNativeCompletion>(),
                    &trace,
                )
            }
            .unwrap();
            // This message travels from the Owner to the SDK after the copy.
            let _ = copy_observed.recv();
            captured.notify();
        });
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            // Only the Owner sends this nudge. There is no SDK-to-worker message.
            reactor.trace_progress().unwrap();
            match reactor.wait_completion(Duration::from_millis(5)) {
                Ok(packet) => {
                    assert_eq!(packet.operation, id);
                    break;
                }
                Err(ReactorError::CompletionTimeout) => {
                    assert!(
                        Instant::now() < deadline,
                        "reactor did not acquire publication"
                    );
                }
                Err(error) => panic!("completion failed in round {round}: {error:?}"),
            }
        }
        mailbox.take_resolutions().unwrap();
        assert_eq!(mailbox.settle(id).unwrap(), Resolution::Completed(Ok(())));
        assert_eq!(receipt.native_transfer_result(), Some(value));
        // SAFETY: this restores the original Box once, after the copy and the SDK's final exchange.
        drop(unsafe { Box::from_raw(carrier) });
        copied.send(()).unwrap();
        sdk.join().unwrap();
    }
    println!("PUBLISHED NATIVE PAYLOAD Rust rounds=64");
}

struct Scratch(std::path::PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn c_acquires_the_final_exchange_before_copying_late_payload() {
    let scratch = Scratch(
        std::env::temp_dir().join(format!("rinzu-publication-order-{}", std::process::id())),
    );
    std::fs::create_dir(&scratch.0).unwrap();
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for mode in ["plain", "address,undefined"] {
        let binary = scratch.0.join("publication");
        let mut compiler = native::clang();
        compiler
            .args([
                "-std=c11", "-O2", "-g", "-pthread", "-Wall", "-Wextra", "-Werror",
            ])
            .arg("-I")
            .arg(root.join("../../include"))
            .arg(root.join("tests/fixtures/published_payload_order.c"))
            .arg("-o")
            .arg(&binary);
        if mode != "plain" {
            compiler
                .arg(format!("-fsanitize={mode}"))
                .arg("-fno-sanitize-recover=all");
        }
        let built = process::bounded_tool(&mut compiler);
        assert!(built.status.success(), "{mode}: {built:?}");
        let result = process::bounded(
            Command::new(&binary)
                .env("RINZU_NATIVE_COMPLETION_NOTIFY", "1")
                .env("RINZU_NATIVE_COMPLETION_DIRECT", "1")
                .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
                .env("UBSAN_OPTIONS", "halt_on_error=1")
                .env("TSAN_OPTIONS", "halt_on_error=1"),
        );
        assert!(result.status.success(), "{mode}: {result:?}");
        assert_eq!(result.stdout, b"PUBLISHED NATIVE PAYLOAD C rounds=64\n");
        assert!(result.stderr.is_empty(), "{mode}: {result:?}");
        println!("PUBLISHED NATIVE PAYLOAD C mode={mode} rounds=64");
    }
}
