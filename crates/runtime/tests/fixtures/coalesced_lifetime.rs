use super::*;
use rinzu_runtime::scheduler::Resolution;
use std::ffi::{CStr, CString, c_char, c_void};
use std::os::unix::ffi::OsStrExt;

#[repr(C)]
struct WakeTarget {
    notify: unsafe extern "C" fn(*mut Self),
    release: unsafe extern "C" fn(*mut Self),
}

type Capture = unsafe extern "C" fn(
    *mut NotifiedNativeCompletion,
    unsafe extern "C" fn(*const c_char, i64),
) -> *mut WakeTarget;

struct ProducerLibrary(*mut c_void);
impl Drop for ProducerLibrary {
    fn drop(&mut self) {
        // SAFETY: this is the single handle returned by a successful dlopen.
        unsafe { libc::dlclose(self.0) };
    }
}

unsafe extern "C" fn observe_publication(kind: *const c_char, arg: i64) {
    // SAFETY: the producer shim passes a static NUL-terminated event name.
    let kind = match unsafe { CStr::from_ptr(kind) }.to_bytes() {
        b"sdk_prefix" => "sdk_prefix",
        b"sdk_exchange" => "sdk_exchange",
        _ => unreachable!("unexpected publication event"),
    };
    Probe::current().unwrap().record(kind, 0, arg, 0);
}

fn capture(
    publish: Capture,
    carrier: &NotifiedNativeCompletion,
    probe: &Probe,
    generation: u64,
) -> *mut WakeTarget {
    let _producer = Probe::producer(generation);
    probe.record("publish_begin", generation, 0, 0);
    // SAFETY: the live SDK object is published once. The C shim only writes
    // atomic fields. The library and observer remain live for this call.
    let target = unsafe { publish(std::ptr::from_ref(carrier).cast_mut(), observe_publication) };
    assert!(
        !target.is_null(),
        "generation {generation} did not transfer a target"
    );
    target
}

fn run(publish: Capture, reverse: bool, controlled: bool) {
    let probe = Probe::new();
    let _scope = Probe::install(Arc::clone(&probe));
    let env = Environment(Arc::clone(&probe));
    let mut reactor =
        AsyncReactor::with_lifetime_trace(Arc::new(move || env.touch()), Arc::clone(&probe));
    let mut mailbox = OperationMailbox::new(Default::default(), Arc::new(|| {}));
    let scalar = Box::new(TransferNativeCompletion {
        result: 7183,
        ..Default::default()
    });
    let unit = Box::new(NotifiedNativeCompletion::default());
    let carriers = [
        std::ptr::from_ref(&scalar.completion),
        std::ptr::from_ref(&*unit),
    ];
    let receipts = std::array::from_fn::<_, 2, _>(|g| {
        let token = 100 + u64::try_from(g).unwrap();
        let address = carriers[g] as usize;
        probe.observe_carrier(address, token);
        probe.record("generation", token, i64::try_from(g).unwrap(), 0);
        // SAFETY: the stable Box remains pinned through all receipt accesses.
        let completion = unsafe { CompletionPointer::new(address) };
        let ticket = mailbox.admit(TaskId(1), "batch receipt".into()).unwrap();
        let id = ticket.operation;
        let operation = if g == 0 {
            reactor.submit_native_transfer(ticket, completion)
        } else {
            reactor.submit_native_completion(ticket, completion)
        }
        .unwrap();
        probe.wait_for("registered", id.0, 1);
        (id, operation)
    });
    let (unit_id, unit_receipt) = &receipts[1];
    if controlled {
        assert_eq!(unit_receipt.request_cancel(), ResolveResult::Won);
        assert_eq!(mailbox.resources().used.pending_operations, 2);
        assert!(reactor.try_completion().unwrap().is_none());
    }
    let leader = capture(publish, &scalar.completion, &probe, 100);
    let covered = capture(publish, &unit, &probe, 101);
    assert_eq!(leader, covered);
    probe.record("merge_wake", 101, 100, 0);
    {
        let _producer = Probe::producer(101);
        // SAFETY: the second exchange supplied a distinct retained reference.
        // The first exchange's reference keeps the shared target alive.
        unsafe { ((*covered).release)(covered) };
        probe.record("publish_end", 101, 0, 0);
    }
    {
        let _producer = Probe::producer(100);
        // SAFETY: this reference survives the callback and is consumed once.
        unsafe {
            ((*leader).notify)(leader);
            ((*leader).release)(leader);
        }
        probe.record("publish_end", 100, 0, 0);
    }

    let first = reactor.wait_completion(Duration::from_secs(30)).unwrap();
    let second = reactor.wait_completion(Duration::from_secs(30)).unwrap();
    assert_ne!(first.operation, second.operation);
    mailbox.take_resolutions().unwrap();
    let order = if reverse { [1, 0] } else { [0, 1] };
    for (index, g) in order.into_iter().enumerate() {
        let (id, operation) = &receipts[g];
        assert!(id == &first.operation || id == &second.operation);
        let decision = mailbox.settle(*id).unwrap();
        if controlled && id == unit_id {
            assert_eq!(decision, Resolution::Cancelled);
        } else {
            assert_eq!(decision, Resolution::Completed(Ok(())));
        }
        assert_eq!(operation.native_transfer_result(), (g == 0).then_some(7183));
        assert_eq!(mailbox.resources().used.pending_operations, 1 - index);
        let carrier = if g == 0 { &scalar.completion } else { &unit };
        let cancelled = carrier.base.cancelled.load(Ordering::Acquire);
        assert_eq!(cancelled, u32::from(controlled && g == 1));
        probe.record(
            "generation_status",
            100 + u64::try_from(g).unwrap(),
            i64::from(cancelled),
            i64::from(carrier.notification.load(Ordering::Acquire) == 1),
        );
        probe.record("close_admission", 100 + u64::try_from(g).unwrap(), 0, 0);
    }
    drop((scalar, unit));
    for (g, (id, _)) in receipts.iter().enumerate() {
        probe.record("carrier_free", 100 + u64::try_from(g).unwrap(), 0, 0);
        probe.wait_for("worker_wake_end", id.0, 1);
    }
    assert_eq!(receipts[0].1.native_transfer_result(), Some(7183));
    drop(receipts);
    reactor.shutdown();
    drop(reactor);
    assert!(mailbox.resources().used.is_baseline());
    let events = probe.events();
    println!("COALESCED Rust reverse={reverse} controlled={controlled}");
    check(Binding::RustChannel, SharedTopology::Coalesced, &events);
}

#[test]
fn two_completions_share_one_native_notification_and_allow_either_consume_order() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let tmp = Temporary(
        std::env::temp_dir().join(format!("rinzu-batch-producer-{}", std::process::id())),
    );
    std::fs::create_dir(&tmp.0).unwrap();
    let library = tmp.0.join("producer");
    let built = process::bounded_tool(
        native::clang()
            .args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror"])
            .arg(if cfg!(target_os = "macos") {
                "-dynamiclib"
            } else {
                "-shared"
            })
            .arg("-fPIC")
            .arg("-I")
            .arg(root.join("include"))
            .arg(root.join("crates/runtime/tests/fixtures/coalesced_producer.c"))
            .arg("-o")
            .arg(&library),
    );
    assert!(built.status.success(), "{built:?}");
    let name = CString::new(library.as_os_str().as_bytes()).unwrap();
    // SAFETY: the fixture is built above and initialized by the system dynamic loader.
    let handle = ProducerLibrary(unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW) });
    assert!(!handle.0.is_null(), "failed to load {library:?}");
    // SAFETY: this handle stays live throughout the test.
    let symbol = unsafe { libc::dlsym(handle.0, c"capture_publication".as_ptr()) };
    assert!(!symbol.is_null());
    // SAFETY: the test C fixture exports this exact signature.
    let publish = unsafe { std::mem::transmute::<*mut c_void, Capture>(symbol) };
    for reverse in [false, true] {
        for controlled in [false, true] {
            run(publish, reverse, controlled);
        }
    }
}
