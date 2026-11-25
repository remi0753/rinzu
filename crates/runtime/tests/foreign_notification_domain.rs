#![cfg(all(
    feature = "test-native-lifetime-trace",
    any(target_os = "macos", target_os = "linux")
))]

#[path = "../../../tests/support/native.rs"]
mod native;
#[path = "fixtures/parallel_registration.rs"]
mod parallel;
#[path = "../../../tests/support/process.rs"]
mod process;

use rinzu_runtime::lifetime_trace::DomainTrace;
use rinzu_runtime::native_completion::{
    CompletionPointer, NotifiedNativeCompletion, TransferNativeCompletion,
};
use rinzu_runtime::reactor::AsyncReactor;
use rinzu_runtime::scheduler::TaskId;
use rinzu_runtime::standalone::OperationMailbox;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, atomic::Ordering};
use std::time::Duration;

const CHILD: &str = "RINZU_TEST_FOREIGN_NOTIFICATION_DOMAIN_VERSION";

fn try_foreign_registration(transfer: bool) {
    let trace = Arc::new(DomainTrace::default());
    let probes = [trace.probe(0), trace.probe(1)];
    let reactors = probes
        .each_ref()
        .map(|p| AsyncReactor::with_lifetime_trace(Arc::new(|| {}), Arc::clone(p)));
    let mut mailboxes =
        [(); 2].map(|()| OperationMailbox::new(Default::default(), Arc::new(|| {})));
    let scalar = Box::new(TransferNativeCompletion::default());
    let unit = Box::new(NotifiedNativeCompletion::default());
    let local = Box::new(NotifiedNativeCompletion::default());
    let foreign = if transfer {
        &scalar.completion
    } else {
        unit.as_ref()
    };
    // SAFETY: both provider-owned allocations remain pinned for the child process.
    let pointers = [foreign, local.as_ref()]
        .map(|c| unsafe { CompletionPointer::new(std::ptr::from_ref(c) as usize) });
    let tickets = std::array::from_fn::<_, 2, _>(|d| {
        mailboxes[d]
            .admit(TaskId(1), "local registration".into())
            .unwrap()
    });
    let ids = tickets.each_ref().map(|ticket| ticket.operation.0);
    let [first, second] = tickets;
    let _foreign_receipt = if transfer {
        reactors[0].submit_native_transfer(first, pointers[0])
    } else {
        reactors[0].submit_native_completion(first, pointers[0])
    }
    .unwrap();
    let _local_receipt = reactors[1]
        .submit_native_completion(second, pointers[1])
        .unwrap();
    for d in 0..2 {
        probes[d].wait_for("registered", ids[d], 1);
    }
    let old_target = foreign.notification.load(Ordering::Acquire);
    let local_target = local.notification.load(Ordering::Acquire);
    assert!(old_target > 1 && local_target > 1 && old_target != local_target);

    let candidate = mailboxes[1]
        .admit(TaskId(1), "foreign registration".into())
        .unwrap();
    let id = candidate.operation.0;
    probes[1].block_for("register_drop", id);
    let _rejected = if transfer {
        reactors[1].submit_native_transfer(candidate, pointers[0])
    } else {
        reactors[1].submit_native_completion(candidate, pointers[0])
    }
    .unwrap();
    probes[1].wait_for("register_drop", id, 1);
    assert_eq!(foreign.notification.load(Ordering::Acquire), old_target);
    assert_eq!(local.notification.load(Ordering::Acquire), local_target);
    let events = trace.events();
    assert!(events.contains(&format!("1,register_cas,{id},0,2")));
    assert!(events.contains(&format!("1,rollback_refs,{id},{local_target},2")));
    assert!(
        !events
            .iter()
            .any(|row| row.starts_with(&format!("1,registered,{id},")))
    );
    println!("foreign registration returned only the candidate");
    std::io::stdout().flush().unwrap();
    probes[1].release_for("register_drop", id);
    let _ = reactors[1].wait_completion(Duration::from_secs(10));
    std::process::exit(99);
}

#[test]
fn rust_rejects_registration_on_another_live_notification_domain() {
    if let Some(version) = std::env::var_os(CHILD) {
        try_foreign_registration(version == "3");
        return;
    }
    for version in ["2", "3"] {
        let result = process::bounded(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "rust_rejects_registration_on_another_live_notification_domain",
                    "--nocapture",
                ])
                .env(CHILD, version),
        );
        assert_eq!(result.status.code(), Some(86), "{result:?}");
        assert!(
            String::from_utf8_lossy(&result.stdout)
                .contains("foreign registration returned only the candidate")
        );
        println!("FOREIGN DOMAIN Rust v{version} candidate returned, exit=86");
    }
}

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn c_rejects_registration_on_another_live_notification_domain() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let temp = Temporary(
        std::env::temp_dir().join(format!("rinzu-foreign-domain-{}", std::process::id())),
    );
    std::fs::create_dir(&temp.0).unwrap();
    let binary = temp.0.join("foreign");
    let build = process::bounded_tool(
        native::clang()
            .args(["-std=c11", "-O0", "-pthread", "-Wall", "-Wextra", "-Werror"])
            .arg("-I")
            .arg(root.join("include"))
            .arg(root.join("crates/runtime/tests/fixtures/foreign_notification_domain.c"))
            .arg("-o")
            .arg(&binary),
    );
    assert!(build.status.success(), "{build:?}");
    for version in ["2", "3"] {
        let result = process::bounded(
            Command::new(&binary)
                .arg(version)
                .env("RINZU_NATIVE_COMPLETION_NOTIFY", "1")
                .env("RINZU_NATIVE_COMPLETION_DIRECT", "1"),
        );
        assert_eq!(result.status.code(), Some(70), "{result:?}");
        let stdout = String::from_utf8_lossy(&result.stdout);
        assert!(
            stdout
                .lines()
                .any(|row| row.starts_with("1,register_cas,") && row.ends_with(",0,2"))
        );
        assert!(stdout.contains("foreign registration returned only the candidate"));
        println!("FOREIGN DOMAIN C v{version} candidate returned, exit=70");
    }
}
