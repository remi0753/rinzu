#![cfg(all(
    feature = "test-native-lifetime-trace",
    any(target_os = "macos", target_os = "linux")
))]

#[path = "fixtures/domain_chain.rs"]
mod chain;
#[path = "fixtures/joined_domain_handoff.rs"]
mod joined;
#[path = "../../../tests/support/native.rs"]
mod native;
#[path = "../../../tests/support/process.rs"]
mod process;
#[path = "fixtures/domain_reuse.rs"]
mod reuse;

use rinzu_runtime::lifetime_trace::{DomainTrace, Probe};
use rinzu_runtime::native_completion::{
    CapturedNativeWake, CompletionPointer, NotifiedNativeCompletion, TransferNativeCompletion,
};
use rinzu_runtime::reactor::{AsyncReactor, ReactorOperation};
use rinzu_runtime::scheduler::{OperationId, Resolution, ResolveResult, TaskId};
use rinzu_runtime::standalone::OperationMailbox;
use rinzu_runtime_model::native_lifetime::{Binding, replay_domains};
use std::path::PathBuf;
use std::sync::{Arc, atomic::Ordering, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
struct Case {
    order: [usize; 4],
    cancelled_domain: usize,
    schedule: Schedule,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Schedule {
    Ready,
    HoldPeer,
    DetachedNotify,
}

fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    let ids = [0, 1, 2, 3];
    for a in ids {
        for b in ids.into_iter().filter(|&b| b != a) {
            for c in ids.into_iter().filter(|&c| c != a && c != b) {
                let d = ids
                    .into_iter()
                    .find(|&d| d != a && d != b && d != c)
                    .unwrap();
                cases.push(Case {
                    order: [a, b, c, d],
                    cancelled_domain: cases.len() % 3,
                    schedule: Schedule::Ready,
                });
            }
        }
    }
    for schedule in [Schedule::HoldPeer, Schedule::DetachedNotify] {
        for order in [[0, 1, 2, 3], [2, 3, 0, 1]] {
            cases.push(Case {
                order,
                cancelled_domain: order[0] / 2,
                schedule,
            });
        }
    }
    cases
}

fn check(binding: Binding, case: Case, events: &[String]) {
    let replay = |rows: &[String]| replay_domains(binding, rows, 100_000);
    let report = replay(events).unwrap_or_else(|error| panic!("{error}\n{}", events.join("\n")));
    assert!(report.concurrent_notifications, "{case:?}");
    assert_eq!(
        report.free_during_foreign_notification,
        case.schedule != Schedule::Ready,
        "{case:?}"
    );
    assert_eq!(
        report.notify_after_environment_free,
        case.schedule == Schedule::DetachedNotify,
        "{case:?}"
    );
    let actual: Vec<_> = report
        .settlement_order
        .iter()
        .map(|(d, g)| d.index() * 2 + g)
        .collect();
    assert_eq!(actual, case.order);
    if case.schedule != Schedule::Ready {
        let line = |prefix: &str| {
            events
                .iter()
                .position(|row| row.starts_with(prefix))
                .unwrap()
        };
        let target = |d| {
            events[line(&format!("{d},batch_target,100,"))]
                .split(',')
                .nth(3)
                .unwrap()
        };
        let mut foreign_release = events.to_vec();
        foreign_release[line("0,producer_refs,100,")] =
            format!("0,producer_refs,100,{},1", target(1));
        let mut foreign_free = events.to_vec();
        foreign_free[line("1,target_identity_free,0,")] =
            format!("1,target_identity_free,0,{},0", target(0));
        let mut incorrect_count = events.to_vec();
        incorrect_count[line("1,producer_refs,101,")] =
            format!("1,producer_refs,101,{},2", target(1));
        let mut missing_return_sample = events.to_vec();
        missing_return_sample.remove(line("0,runtime_refs,0,"));
        let mut missing_context_identity = events.to_vec();
        missing_context_identity.remove(
            events
                .iter()
                .position(|row| row.contains(",context_identity,0,"))
                .unwrap(),
        );
        let mut aliased_target = events.to_vec();
        for row in &mut aliased_target {
            if row.starts_with("1,") {
                *row = row.replace(&format!(",{},", target(1)), &format!(",{},", target(0)));
            }
        }
        for bad in [
            foreign_release,
            foreign_free,
            incorrect_count,
            missing_return_sample,
            missing_context_identity,
            aliased_target,
        ] {
            assert!(
                replay(&bad).is_err(),
                "accepted invalid cross-Domain evidence"
            );
        }
    }
    println!(
        "DOMAIN TRACE {binding:?} {case:?} events={} free_during_foreign_notification={}",
        events.len(),
        report.free_during_foreign_notification,
    );
}

struct WakeEnvironment {
    probe: Arc<Probe>,
    owner: usize,
}

impl WakeEnvironment {
    fn address(&self) -> i64 {
        i64::try_from(std::ptr::from_ref(self) as usize).unwrap()
    }
}

impl Drop for WakeEnvironment {
    fn drop(&mut self) {
        self.probe
            .record("environment_identity_free", 0, self.address(), 0);
        self.probe.record("env_free", 0, 0, 0);
    }
}

fn traced_reactor(probe: &Arc<Probe>, d: usize) -> AsyncReactor {
    let environment = Box::new(WakeEnvironment {
        probe: Arc::clone(probe),
        owner: d,
    });
    probe.record("domain_context", 0, environment.address(), 0);
    AsyncReactor::with_lifetime_trace(
        Arc::new(move || {
            assert_eq!(environment.owner, d);
            environment
                .probe
                .record("context_identity", 0, environment.address(), 0);
            environment.probe.record("context_touch", 0, 0, 0);
        }),
        Arc::clone(probe),
    )
}

struct RustDomain {
    probe: Arc<Probe>,
    reactor: AsyncReactor,
    mailbox: OperationMailbox,
    receipts: Option<[(OperationId, ReactorOperation); 2]>,
    scalar: Option<Box<TransferNativeCompletion>>,
    unit: Option<Box<NotifiedNativeCompletion>>,
    producer: Option<(mpsc::Sender<()>, JoinHandle<()>)>,
    remaining: usize,
    value: i64,
    controlled: bool,
}

impl RustDomain {
    fn new(timeline: &Arc<DomainTrace>, d: usize, controlled: bool) -> Self {
        Self::with_storage(timeline, d, controlled, None)
    }

    fn with_storage(
        timeline: &Arc<DomainTrace>,
        d: usize,
        controlled: bool,
        reused: Option<Box<TransferNativeCompletion>>,
    ) -> Self {
        let probe = timeline.probe(d);
        let _scope = Probe::install(Arc::clone(&probe));
        let value = 7183 + 97 * i64::try_from(d).unwrap();
        let mut scalar = if let Some(storage) = reused {
            assert_eq!(d, 1);
            assert_eq!(storage.completion.notification.load(Ordering::Acquire), 1);
            let address = std::ptr::from_ref(&storage.completion) as usize;
            probe.record("storage_handoff", 100, i64::try_from(address).unwrap(), 0);
            storage
        } else {
            Box::default()
        };
        *scalar = TransferNativeCompletion {
            result: value,
            ..Default::default()
        };
        let reactor = traced_reactor(&probe, d);
        let mut mailbox = OperationMailbox::new(Default::default(), Arc::new(|| {}));
        let unit = Box::new(NotifiedNativeCompletion::default());
        let carriers = [&scalar.completion, unit.as_ref()];
        let receipts = std::array::from_fn(|g| {
            let generation = 100 + u64::try_from(g).unwrap();
            let address = std::ptr::from_ref(carriers[g]) as usize;
            probe.observe_carrier(address, generation);
            probe.record("generation", generation, i64::try_from(g).unwrap(), 0);
            probe.record(
                "carrier_address",
                generation,
                i64::try_from(address).unwrap(),
                i64::from(carriers[g].base.version),
            );
            // SAFETY: the SDK Boxes stay pinned until these receipts have collected.
            let completion = unsafe { CompletionPointer::new(address) };
            let ticket = mailbox.admit(TaskId(1), "Domain receipt".into()).unwrap();
            let id = ticket.operation;
            let op = if g == 0 {
                reactor.submit_native_transfer(ticket, completion)
            } else {
                reactor.submit_native_completion(ticket, completion)
            }
            .unwrap();
            probe.wait_for("registered", id.0, 1);
            (id, op)
        });
        if controlled {
            assert_eq!(receipts[1].1.request_cancel(), ResolveResult::Won);
            assert!(reactor.try_completion().unwrap().is_none());
            assert_eq!(mailbox.resources().used.pending_operations, 2);
        }
        Self {
            probe,
            reactor,
            mailbox,
            receipts: Some(receipts),
            scalar: Some(scalar),
            unit: Some(unit),
            producer: None,
            remaining: 2,
            value,
            controlled,
        }
    }

    fn start_producer(&mut self, delayed: bool) {
        if delayed {
            self.probe.block_for("publish_end", 101);
        } else {
            self.probe.block_for("notify_enter", 100);
        }
        let scalar = std::ptr::from_ref(&self.scalar.as_ref().unwrap().completion) as usize;
        let unit = std::ptr::from_ref(self.unit.as_deref().unwrap()) as usize;
        let probe = Arc::clone(&self.probe);
        let (return_leader, owner_done) = mpsc::channel();
        self.producer = Some((
            return_leader,
            std::thread::spawn(move || {
                let leader = capture(&probe, 100, scalar);
                let duplicate = capture(&probe, 101, unit);
                assert_eq!(leader.address(), duplicate.address());
                probe.record("merge_wake", 101, 100, 0);
                return_reference(&probe, 101, duplicate);
                {
                    let _producer = Probe::producer(100);
                    leader.notify();
                }
                owner_done.recv().unwrap();
                return_reference(&probe, 100, leader);
            }),
        ));
    }

    fn resume_notification(&self) {
        self.probe.release_for("notify_enter", 100);
        self.probe.wait_for("notify_leave", 100, 1);
    }

    fn wait_for_packets(&mut self) {
        let mut received = [false; 2];
        for _ in 0..2 {
            let packet = self
                .reactor
                .wait_completion(Duration::from_secs(30))
                .unwrap();
            let g = self
                .receipts
                .as_ref()
                .unwrap()
                .iter()
                .position(|(id, _)| *id == packet.operation)
                .unwrap();
            assert!(!std::mem::replace(&mut received[g], true));
        }
        self.mailbox.take_resolutions().unwrap();
    }

    fn settle(&mut self, g: usize, keep_storage: bool) {
        let (id, operation) = &self.receipts.as_ref().unwrap()[g];
        let expected = if self.controlled && g == 1 {
            Resolution::Cancelled
        } else {
            Resolution::Completed(Ok(()))
        };
        assert_eq!(self.mailbox.settle(*id).unwrap(), expected);
        assert_eq!(
            operation.native_transfer_result(),
            (g == 0).then_some(self.value)
        );
        let carrier = if g == 0 {
            &self.scalar.as_ref().unwrap().completion
        } else {
            self.unit.as_deref().unwrap()
        };
        let generation = 100 + u64::try_from(g).unwrap();
        self.probe.record(
            "generation_status",
            generation,
            i64::from(carrier.base.cancelled.load(Ordering::Acquire)),
            i64::from(carrier.notification.load(Ordering::Acquire) == 1),
        );
        self.probe.record("close_admission", generation, 0, 0);
        if keep_storage {
            let address = i64::try_from(std::ptr::from_ref(carrier) as usize).unwrap();
            self.probe.record("carrier_retire", generation, address, 0);
        } else {
            if g == 0 {
                drop(self.scalar.take().unwrap());
            } else {
                drop(self.unit.take().unwrap());
            }
            self.probe.record("carrier_free", generation, 0, 0);
        }
        self.probe.wait_for("worker_wake_end", id.0, 1);
        self.remaining -= 1;
        assert_eq!(
            self.mailbox.resources().used.pending_operations,
            self.remaining
        );
    }

    fn dispose(&mut self) {
        let receipts = self.receipts.take().unwrap();
        assert_eq!(receipts[0].1.native_transfer_result(), Some(self.value));
        drop(receipts);
        self.probe.wait("record_release", 2);
        self.reactor.shutdown();
        self.probe.wait("env_free", 1);
        assert!(self.mailbox.resources().used.is_baseline());
    }

    fn return_sdk_reference(&mut self) {
        self.probe.release_for("publish_end", 101);
        let (release, publisher) = self.producer.take().unwrap();
        release.send(()).unwrap();
        publisher.join().unwrap();
        self.probe.wait("target_free", 1);
    }

    fn finish(&mut self) {
        self.dispose();
        self.return_sdk_reference();
    }
}

fn capture(probe: &Probe, generation: u64, raw: usize) -> CapturedNativeWake {
    let _producer = Probe::producer(generation);
    probe.record("publish_begin", generation, 0, 0);
    // SAFETY: the SDK owns this uniquely published carrier through its last atomic exchange.
    let wake = unsafe {
        NotifiedNativeCompletion::capture_with_lifetime_trace(
            raw as *mut NotifiedNativeCompletion,
            probe,
        )
    }
    .unwrap();
    probe.record(
        "batch_target",
        generation,
        i64::try_from(wake.address()).unwrap(),
        0,
    );
    wake
}

fn return_reference(probe: &Probe, generation: u64, reference: CapturedNativeWake) {
    let _producer = Probe::producer(generation);
    drop(reference);
    probe.record("publish_end", generation, 0, 0);
}

#[test]
fn rust_batches_are_independent_across_notification_domains() {
    for case in cases() {
        let timeline = Arc::new(DomainTrace::default());
        let mut owners = [
            RustDomain::new(&timeline, 0, case.cancelled_domain == 0),
            RustDomain::new(&timeline, 1, case.cancelled_domain == 1),
        ];
        let first = case.order[0] / 2;
        let late = case.schedule == Schedule::DetachedNotify;
        for (d, owner) in owners.iter_mut().enumerate() {
            owner.start_producer(late && d == first);
        }
        owners[1 - first].probe.wait_for("notify_enter", 100, 1);
        if late {
            owners[first].probe.wait_for("publish_end", 101, 1);
            // The real shutdown command lets the reactor copy both published
            // carriers before the SDK invokes its retained wake target.
            owners[first].reactor.shutdown();
        } else {
            owners[first].probe.wait_for("notify_enter", 100, 1);
            owners[first].resume_notification();
        }
        if case.schedule == Schedule::Ready {
            owners[1 - first].resume_notification();
        }
        owners[first].wait_for_packets();
        if case.schedule == Schedule::Ready {
            owners[1 - first].wait_for_packets();
        }
        for choice in case.order {
            let d = choice / 2;
            owners[d].settle(choice % 2, false);
            if owners[d].remaining == 0 {
                owners[d].finish();
                if case.schedule != Schedule::Ready && d == first {
                    owners[1 - d].resume_notification();
                    owners[1 - d].wait_for_packets();
                }
            }
        }
        check(Binding::RustChannel, case, &timeline.events());
    }
}

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn c_batches_are_independent_across_notification_domains() {
    with_c_binary("distinct", "domain_batches.c", |binary, sanitized| {
        for case in cases() {
            let schedule = match case.schedule {
                Schedule::Ready => 0,
                Schedule::HoldPeer => 1,
                Schedule::DetachedNotify => 2,
            };
            let events = c_trace(binary, case.order, case.cancelled_domain, schedule, false);
            println!("DOMAIN C sanitized={sanitized}");
            check(Binding::CDirect, case, &events);
        }
    });
}

fn with_c_binary(suite: &str, fixture: &str, test: impl Fn(&std::path::Path, bool)) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let tmp = Temporary(
        std::env::temp_dir().join(format!("rinzu-domain-{suite}-{}", std::process::id())),
    );
    std::fs::create_dir(&tmp.0).unwrap();
    for sanitized in [false, true] {
        let binary = tmp.0.join("domains");
        let mut compiler = native::clang();
        compiler.args(["-std=c11", "-pthread", "-Wall", "-Wextra", "-Werror"]);
        if sanitized {
            compiler.args([
                "-O0",
                "-g",
                "-fsanitize=address,undefined",
                "-fno-sanitize-recover=all",
            ]);
        } else {
            compiler.arg("-O1");
        }
        let build = process::bounded_tool(
            compiler
                .arg("-I")
                .arg(root.join("include"))
                .arg(root.join("crates/runtime/tests/fixtures").join(fixture))
                .arg("-o")
                .arg(&binary),
        );
        assert!(build.status.success(), "{build:?}");
        test(&binary, sanitized);
    }
}

fn c_trace(
    binary: &std::path::Path,
    order: [usize; 4],
    cancelled_domain: usize,
    schedule: u8,
    reuse: bool,
) -> Vec<String> {
    let order: String = order
        .into_iter()
        .map(|v| char::from(b'0' + u8::try_from(v).unwrap()))
        .collect();
    let mut args = vec![order, cancelled_domain.to_string(), schedule.to_string()];
    if reuse {
        args.push("reuse".into());
    }
    c_fixture_trace(binary, &args)
}

fn c_fixture_trace(binary: &std::path::Path, args: &[String]) -> Vec<String> {
    c_fixture_trace_on(binary, args, Binding::CDirect)
}

fn c_fixture_trace_on(binary: &std::path::Path, args: &[String], binding: Binding) -> Vec<String> {
    let mut command = std::process::Command::new(binary);
    command
        .args(args)
        .env("RINZU_NATIVE_COMPLETION_NOTIFY", "1")
        .env(
            "RINZU_NATIVE_COMPLETION_DIRECT",
            if binding == Binding::CJoined {
                "0"
            } else {
                "1"
            },
        )
        .env("ASAN_OPTIONS", native::asan_options_after_teardown())
        .env("UBSAN_OPTIONS", "halt_on_error=1");
    let run = process::bounded(&mut command);
    assert!(run.status.success(), "{args:?} {run:?}");
    String::from_utf8(run.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}
