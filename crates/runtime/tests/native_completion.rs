use rinzu_runtime::native_completion::{
    CompletionPointer, NativeCompletion, NotifiedNativeCompletion, TransferNativeCompletion,
};
use rinzu_runtime::reactor::{AsyncReactor, ReactorOperationKind, ReactorTracePhase};
use rinzu_runtime::scheduler::{ResolveResult, TaskId};
use rinzu_runtime::standalone::OperationMailbox;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

#[test]
fn scalar_native_receipts_are_copied_before_carriers_retire() {
    assert_eq!(size_of::<TransferNativeCompletion>(), 32);
    let mut mailbox = OperationMailbox::new(Default::default(), Arc::new(|| {}));
    let mut reactor = AsyncReactor::new();
    let values = [i64::MIN, -7509 * 131072 + 65536, -1, 0, 1, 65536, i64::MAX];
    let mut carriers = Vec::new();
    let mut results = Vec::new();
    let mut ids = Vec::new();
    for before in [false, true] {
        for value in values {
            let carrier = Box::into_raw(Box::new(TransferNativeCompletion::default()));
            // SAFETY: this unique native producer owns result through its last
            // publication. Every carrier is pinned until reactor shutdown.
            unsafe {
                (*carrier).result = value;
                if before {
                    NotifiedNativeCompletion::publish(std::ptr::addr_of_mut!(
                        (*carrier).completion
                    ));
                }
            }
            // SAFETY: valid v3 storage stays alive through every admitted use.
            let pointer = unsafe { CompletionPointer::new(carrier as usize) };
            for alias in 0..2 {
                let ticket = mailbox.admit(TaskId(1), "scalar receipt".into()).unwrap();
                ids.push(ticket.operation);
                let operation = reactor.submit_native_transfer(ticket, pointer).unwrap();
                if !before && alias == 0 {
                    assert_eq!(operation.request_cancel(), ResolveResult::Won);
                }
                results.push((operation, value));
            }
            let ticket = mailbox.admit(TaskId(1), "unit receipt".into()).unwrap();
            ids.push(ticket.operation);
            reactor.submit_native_completion(ticket, pointer).unwrap();
            carriers.push((carrier, before));
        }
    }
    for &(carrier, before) in &carriers {
        if !before {
            // SAFETY: pinned once-published carrier, no producer access after
            // the final exchange. Payload was written before this release.
            unsafe {
                NotifiedNativeCompletion::publish(std::ptr::addr_of_mut!((*carrier).completion));
            }
        }
    }
    reactor.shutdown();
    for &(carrier, _) in &carriers {
        // SAFETY: shutdown joined all physical observations and result copies.
        drop(unsafe { Box::from_raw(carrier) });
    }
    for (operation, expected) in results {
        assert_eq!(operation.native_transfer_result(), Some(expected));
        assert_eq!(operation.native_transfer_result(), Some(expected));
    }
    for _ in &ids {
        let completion = reactor.wait_completion(Duration::from_secs(2)).unwrap();
        mailbox.take_resolutions().unwrap();
        mailbox.settle(completion.operation).unwrap();
    }
    assert!(mailbox.resources().used.is_baseline());
}

#[test]
fn notified_waits_share_registration_and_shutdown_drains_physical_completion() {
    assert_eq!(size_of::<NotifiedNativeCompletion>(), 24);
    let mut mailbox = OperationMailbox::new(Default::default(), Arc::new(|| {}));
    let mut reactor = AsyncReactor::new();
    let values: Vec<_> = (0..64)
        .map(|_| Arc::new(NotifiedNativeCompletion::default()))
        .collect();
    let mut ids = Vec::new();
    for (index, value) in values.iter().enumerate() {
        if index % 2 == 0 {
            // SAFETY: the Arc pins this unique producer's carrier through its
            // final exchange; the following admission also retains the Arc.
            unsafe { NotifiedNativeCompletion::publish(Arc::as_ptr(value).cast_mut()) };
        }
        // SAFETY: all Arcs remain pinned until reactor shutdown below.
        let pointer = unsafe { CompletionPointer::new(Arc::as_ptr(value) as usize) };
        for repeat in 0..2 {
            let ticket = mailbox
                .admit(TaskId(1), format!("v2 {index} {repeat}"))
                .unwrap();
            ids.push(ticket.operation);
            let operation = reactor.submit_native_completion(ticket, pointer).unwrap();
            if index % 2 != 0 {
                assert_eq!(operation.request_cancel(), ResolveResult::Won);
                assert_eq!(value.base.cancelled.load(Ordering::Acquire), 1);
            }
        }
    }
    let legacy = Arc::new(NativeCompletion {
        version: 1,
        size: 16,
        done: AtomicU32::new(0),
        cancelled: AtomicU32::new(0),
    });
    let ticket = mailbox.admit(TaskId(1), "mixed legacy".into()).unwrap();
    ids.push(ticket.operation);
    // SAFETY: this Arc also stays pinned through shutdown.
    let pointer = unsafe { CompletionPointer::new(Arc::as_ptr(&legacy) as usize) };
    reactor.submit_native_completion(ticket, pointer).unwrap();
    for (index, value) in values.iter().enumerate() {
        if index % 2 != 0 {
            // A v2 prefix alone is not physical completion.
            value.base.done.store(1, Ordering::Release);
            // SAFETY: the Arc pins a valid, once-published carrier.
            let pointer = unsafe { CompletionPointer::new(Arc::as_ptr(value) as usize) };
            assert!(!pointer.is_done());
            // SAFETY: this producer publishes its final payload once.
            unsafe { NotifiedNativeCompletion::publish(Arc::as_ptr(value).cast_mut()) };
        }
    }
    legacy.done.store(1, Ordering::Release);
    // Shutdown must process queued admissions and notifications, not disconnect
    // prematurely or deadlock on the Sender retained by each native target.
    reactor.shutdown();
    for _ in &ids {
        let completion = reactor.wait_completion(Duration::from_secs(2)).unwrap();
        assert_eq!(completion.kind, ReactorOperationKind::NativeCompletion);
        mailbox.take_resolutions().unwrap();
        mailbox.settle(completion.operation).unwrap();
    }
    assert!(mailbox.resources().used.is_baseline());
}

#[test]
fn many_native_waits_share_reactor_and_cancel_does_not_publish_physical_completion() {
    assert_eq!(size_of::<NativeCompletion>(), 16);
    let mut mailbox = OperationMailbox::new(Default::default(), Arc::new(|| {}));
    let reactor = AsyncReactor::new();
    let values: Vec<_> = (0..64)
        .map(|_| {
            Arc::new(NativeCompletion {
                version: 1,
                size: 16,
                done: AtomicU32::new(0),
                cancelled: AtomicU32::new(0),
            })
        })
        .collect();
    let mut operations = Vec::new();
    for (index, value) in values.iter().enumerate() {
        // SAFETY: each Arc's allocation stays pinned until reactor shutdown below.
        let pointer = unsafe { CompletionPointer::new(Arc::as_ptr(value) as usize) };
        let ticket = mailbox
            .admit(TaskId(1), "native completion".into())
            .unwrap();
        operations.push(reactor.submit_native_completion(ticket, pointer).unwrap());
        if index % 2 == 0 {
            assert_eq!(operations[index].request_cancel(), ResolveResult::Won);
            assert_eq!(value.cancelled.load(Ordering::Acquire), 1);
        }
    }
    assert!(reactor.try_completion().unwrap().is_none());
    let timer = mailbox.admit(TaskId(1), "timer".into()).unwrap();
    reactor
        .submit_timer(timer, Duration::from_millis(2))
        .unwrap();
    let timer = reactor.wait_completion(Duration::from_secs(2)).unwrap();
    assert_eq!(timer.kind, ReactorOperationKind::Timer);
    for value in &values {
        value.done.store(1, Ordering::Release);
    }
    let mut ids = vec![timer.operation];
    for _ in &values {
        let completion = reactor.wait_completion(Duration::from_secs(2)).unwrap();
        assert_eq!(completion.kind, ReactorOperationKind::NativeCompletion);
        ids.push(completion.operation);
    }
    mailbox.take_resolutions().unwrap();
    for id in ids {
        mailbox.settle(id).unwrap();
    }
    assert!(mailbox.resources().used.is_baseline());
    let helpers: std::collections::HashSet<_> = reactor
        .trace()
        .iter()
        .filter(|event| event.phase == ReactorTracePhase::Started)
        .map(|event| event.thread)
        .collect();
    assert_eq!(helpers.len(), 1);
    drop(reactor);
    drop(values);
}

#[test]
fn c_shared_reactor_preserves_publication_and_settles_cancelled_metadata() {
    let directory = std::env::temp_dir().join(format!("rinzu-completion-c-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let source = directory.join("completion.c");
    std::fs::write(&source, include_str!("fixtures/native_completion.c")).unwrap();
    let binary = directory.join("completion");
    let output = std::process::Command::new("clang")
        .args([
            "-std=c11",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pthread",
            "-fsanitize=address,undefined",
        ])
        .arg("-I")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/../../include"))
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    for direct in ["0", "1"] {
        let output = std::process::Command::new(&binary)
            .env("RINZU_NATIVE_COMPLETION_DIRECT", direct)
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .env("UBSAN_OPTIONS", "halt_on_error=1")
            .output()
            .unwrap();
        assert!(output.status.success(), "direct={direct}: {output:?}");
    }
    std::fs::remove_dir_all(directory).unwrap();
}
