#[path = "../../../tests/support/native.rs"]
mod native;

use rinzu_runtime::native_completion::{
    CompletionPointer, NotifiedNativeCompletion, TransferNativeCompletion,
};
use rinzu_runtime::reactor::AsyncReactor;
use rinzu_runtime::scheduler::{Resolution, ResolveResult, TaskId};
use rinzu_runtime::standalone::OperationMailbox;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

const VECTORS: &str = include_str!("../../../tests/conformance/native-lifetime-vectors.csv");

#[test]
fn rust_reactor_replays_shared_native_lifetime_vectors() {
    for line in VECTORS.lines().skip(1) {
        let f: Vec<_> = line.split(',').collect();
        let payload: i64 = f[5].parse().unwrap();
        let mut mailbox = OperationMailbox::new(Default::default(), Arc::new(|| {}));
        let ticket = mailbox.admit(TaskId(1), f[0].into()).unwrap();
        let id = ticket.operation;
        let mut reactor = AsyncReactor::new();
        let mut carrier = Box::new(TransferNativeCompletion::default());
        carrier.result = payload;
        // SAFETY: this Box pins the carrier through every operation and control
        // access. Only its unique producer publishes; no payload writes follow.
        let pointer = unsafe { CompletionPointer::new(std::ptr::from_ref(&*carrier) as usize) };
        let operation = reactor.submit_native_transfer(ticket, pointer).unwrap();
        let publish = || {
            // SAFETY: the carrier is pinned, valid v3 storage, published once.
            unsafe {
                NotifiedNativeCompletion::publish(
                    std::ptr::from_ref(&carrier.completion).cast_mut(),
                )
            };
        };
        if f[2] == "1" {
            publish();
            reactor.wait_completion(Duration::from_secs(5)).unwrap();
        }
        let won = if f[1] == "1" {
            operation.request_cancel()
        } else {
            operation.request_timeout()
        };
        assert_eq!(won == ResolveResult::Won, f[3] == "1", "{}", f[0]);
        if f[2] == "0" {
            assert!(!pointer.is_done());
            assert!(reactor.try_completion().unwrap().is_none());
            publish();
            reactor.wait_completion(Duration::from_secs(5)).unwrap();
        }
        reactor.shutdown();
        assert_eq!(operation.native_transfer_result(), Some(payload));
        mailbox.take_resolutions().unwrap();
        let resolution = mailbox.settle(id).unwrap();
        let expected = match f[4] {
            "cancelled" => Resolution::Cancelled,
            "timed_out" => Resolution::TimedOut,
            "completed" => Resolution::Completed(Ok(())),
            _ => panic!("invalid vector"),
        };
        assert_eq!(resolution, expected, "{}", f[0]);
        drop(carrier);
        assert_eq!(operation.request_cancel(), ResolveResult::Late);
        assert_eq!(operation.request_timeout(), ResolveResult::Late);
        assert_eq!(operation.native_transfer_result(), Some(payload));
        assert!(mailbox.resources().used.is_baseline());
    }
}

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn generated_c_runtime_replays_shared_native_lifetime_vectors_with_sanitizers() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let temp = Temporary(std::env::temp_dir().join(format!(
        "rinzu-native-lifetime-vectors-{}",
        std::process::id()
    )));
    std::fs::create_dir_all(&temp.0).unwrap();
    for flags in [
        vec!["-O2"],
        vec![
            "-O1",
            "-g",
            "-fsanitize=address,undefined",
            "-fno-sanitize-recover=all",
        ],
    ] {
        let binary = temp.0.join("vectors");
        let output = native::clang()
            .args(["-std=c11", "-pthread", "-Wall", "-Wextra", "-Werror"])
            .args(&flags)
            .arg("-I")
            .arg(root.join("include"))
            .arg(root.join("crates/runtime/tests/fixtures/native_lifetime_vectors.c"))
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let output = Command::new(&binary)
            .arg(root.join("tests/conformance/native-lifetime-vectors.csv"))
            .env("RINZU_NATIVE_COMPLETION_DIRECT", "1")
            .env("RINZU_NATIVE_COMPLETION_NOTIFY", "0")
            .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
            .env("UBSAN_OPTIONS", "halt_on_error=1")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(String::from_utf8_lossy(&output.stdout).lines().count(), 4);
    }
}
