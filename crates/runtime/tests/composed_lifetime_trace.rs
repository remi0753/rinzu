#![cfg(feature = "test-native-lifetime-trace")]
#[path = "../../../tests/support/native.rs"]
mod native;
#[path = "../../../tests/support/process.rs"]
#[allow(dead_code)]
mod process;
use rinzu_runtime::lifetime_trace::Probe;
use rinzu_runtime::native_completion::{
    CompletionPointer, NotifiedNativeCompletion, TransferNativeCompletion,
};
use rinzu_runtime::reactor::AsyncReactor;
use rinzu_runtime::scheduler::{ResolveResult, TaskId};
use rinzu_runtime::standalone::OperationMailbox;
use rinzu_runtime_model::native_lifetime::{Binding, replay};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

struct Environment(Arc<Probe>);
impl Environment {
    fn touch(&self) {
        self.0.record("context_touch", 0, 0, 0);
    }
}
impl Drop for Environment {
    fn drop(&mut self) {
        self.0.record("env_free", 0, 0, 0);
    }
}

fn check_trace(binding: Binding, count: usize, events: &[String]) {
    let report = replay(binding, count, 7183, events, 100_000)
        .unwrap_or_else(|e| panic!("{e}\n{}", events.join("\n")));
    println!(
        "TRACE {binding:?} waiters={count} events={} frontier={}",
        report.events, report.maximum_frontier
    );
    let mut early = events.to_vec();
    early.insert(0, "producer_released,0,0,0".into());
    assert!(replay(binding, count, 7183, &early, 100_000).is_err());
    let mut early = events.to_vec();
    early.insert(2, "env_free,0,0,0".into());
    assert!(replay(binding, count, 7183, &early, 100_000).is_err());
    let mut wrong = events.to_vec();
    let row = wrong.iter_mut().find(|s| s.starts_with("copied,")).unwrap();
    let f: Vec<_> = row.split(',').collect();
    *row = format!("copied,{},19,0", f[1]);
    assert!(replay(binding, count, 7183, &wrong, 100_000).is_err());
    let mut missing = events.to_vec();
    missing.remove(
        missing
            .iter()
            .position(|s| s.starts_with("physical,"))
            .unwrap(),
    );
    assert!(replay(binding, count, 7183, &missing, 100_000).is_err());
    let mut unknown = events.to_vec();
    unknown.insert(2, "invented_event,0,0,0".into());
    assert!(replay(binding, count, 7183, &unknown, 100_000).is_err());
    assert!(replay(binding, count, 7183, &events[..events.len() - 1], 100_000).is_err());
    assert!(replay(binding, count, 7183, events, 1).is_err());
    let copied = events
        .iter()
        .position(|s| s.starts_with("copied,"))
        .unwrap();
    let mut duplicate = events.to_vec();
    duplicate.insert(copied, events[copied].clone());
    assert!(replay(binding, count, 7183, &duplicate, 100_000).is_err());
    let invalid_bool = if events.iter().any(|e| e.starts_with("control_end,")) {
        ("control_end,", "control_end,{id},2,0")
    } else {
        ("registered,", "registered,{id},2,1")
    };
    for (prefix, replacement) in [
        invalid_bool,
        ("env_free,", "env_free,9,0,0"),
        ("copied,", "copied,{id},7183,1"),
        ("copied,", "copied,-1,7183,0"),
        ("copied,", "copied,18446744073709551615,7183,0"),
    ] {
        let mut invalid = events.to_vec();
        let row = invalid.iter_mut().find(|s| s.starts_with(prefix)).unwrap();
        let id = row.split(',').nth(1).unwrap();
        *row = replacement.replace("{id}", id);
        assert!(replay(binding, count, 7183, &invalid, 100_000).is_err());
    }
    let mut unregistered = events.to_vec();
    unregistered.remove(
        unregistered
            .iter()
            .rposition(|s| s.starts_with("registered,"))
            .unwrap(),
    );
    assert!(replay(binding, count, 7183, &unregistered, 100_000).is_err());
    for kind in [
        "register_retain,",
        "completion_claim,",
        "register_cas,",
        "sdk_prefix,",
        "sdk_exchange,",
    ] {
        let mut missing = events.to_vec();
        missing.remove(missing.iter().position(|e| e.starts_with(kind)).unwrap());
        assert!(
            replay(binding, count, 7183, &missing, 100_000).is_err(),
            "accepted missing {kind}"
        );
    }
    let registration = events
        .iter()
        .position(|e| e.starts_with("registered,"))
        .unwrap();
    let first_id = events[registration].split(',').nth(1).unwrap();
    let mut wrong_role = events.to_vec();
    wrong_role.insert(registration, format!("register_drop,{first_id},0,0"));
    assert!(replay(binding, count, 7183, &wrong_role, 100_000).is_err());
    let mut bad_cas = events.to_vec();
    let row = bad_cas
        .iter_mut()
        .find(|e| e.starts_with("register_cas,"))
        .unwrap();
    let f: Vec<_> = row.split(',').collect();
    *row = if f[2] == "1" {
        format!("register_cas,{},0,1", f[1])
    } else {
        format!("register_cas,{},1,0", f[1])
    };
    assert!(replay(binding, count, 7183, &bad_cas, 100_000).is_err());
    let mut bad_exchange = events.to_vec();
    let row = bad_exchange
        .iter_mut()
        .find(|e| e.starts_with("sdk_exchange,"))
        .unwrap();
    *row = if row == "sdk_exchange,0,1,0" {
        "sdk_exchange,0,0,0".into()
    } else {
        "sdk_exchange,0,1,0".into()
    };
    assert!(replay(binding, count, 7183, &bad_exchange, 100_000).is_err());
    let mut bad_count = events.to_vec();
    let row = &mut bad_count[registration];
    let f: Vec<_> = row.split(',').collect();
    *row = format!("registered,{},{},3", f[1], f[2]);
    assert!(replay(binding, count, 7183, &bad_count, 100_000).is_err());
    let mut reversed = events.to_vec();
    let prefix = reversed
        .iter()
        .position(|e| e.starts_with("sdk_prefix,"))
        .unwrap();
    let exchange = reversed
        .iter()
        .position(|e| e.starts_with("sdk_exchange,"))
        .unwrap();
    reversed.swap(prefix, exchange);
    assert!(replay(binding, count, 7183, &reversed, 100_000).is_err());
    let stripped: Vec<_> = events
        .iter()
        .filter(|e| {
            !matches!(
                e.split(',').next(),
                Some(
                    "register_begin"
                        | "register_retain"
                        | "register_cas"
                        | "register_drop"
                        | "sdk_prefix"
                        | "sdk_exchange"
                        | "notify_ready"
                )
            )
        })
        .cloned()
        .collect();
    assert!(
        replay(binding, count, 7183, &stripped, 100_000).is_err(),
        "startup trace fell back to post-registration replay"
    );
    let mut wrong_claim = events.to_vec();
    let row = wrong_claim
        .iter_mut()
        .find(|e| e.starts_with("completion_claim,"))
        .unwrap();
    let f: Vec<_> = row.split(',').collect();
    *row = format!(
        "completion_claim,{},{},0",
        f[1],
        if f[2] == "1" { 0 } else { 1 }
    );
    assert!(replay(binding, count, 7183, &wrong_claim, 100_000).is_err());
    println!("REJECT {binding:?} waiters={count} mutations=26");
}

fn submit_receipt(
    reactor: &AsyncReactor,
    mailbox: &mut OperationMailbox,
    pointer: CompletionPointer,
) -> (
    rinzu_runtime::scheduler::OperationId,
    rinzu_runtime::reactor::ReactorOperation,
) {
    let ticket = mailbox
        .admit(TaskId(1), "registration race".into())
        .unwrap();
    let id = ticket.operation;
    (id, reactor.submit_native_transfer(ticket, pointer).unwrap())
}
fn publisher(raw: usize, probe: Arc<Probe>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        probe.record("publish_begin", 0, 0, 0);
        // SAFETY: the caller pins the carrier through registration and every copy.
        unsafe {
            NotifiedNativeCompletion::publish_with_lifetime_trace(
                raw as *mut NotifiedNativeCompletion,
                &probe,
            )
        };
        probe.record("publish_end", 0, 0, 0);
    })
}
fn check_reference_roles(count: usize, events: &[String]) {
    let occurrences = |kind: &str| {
        events
            .iter()
            .filter(|e| e.starts_with(&format!("{kind},")))
            .count()
    };
    let wins = events
        .iter()
        .filter(|e| e.starts_with("register_cas,") && e.split(',').nth(2) == Some("1"))
        .count();
    assert!(wins <= 1);
    assert_eq!(occurrences("register_retain"), count);
    assert_eq!(occurrences("register_drop"), count - wins);
    assert_eq!(occurrences("notify_enter"), wins);
    assert_eq!(occurrences("producer_release"), wins);
    assert!(events.contains(&format!("sdk_exchange,0,{wins},0")));
}

#[test]
fn rust_registration_transfer_and_sdk_exchange_races_refine_the_model() {
    for count in [1, 2] {
        for scenario in 0..8 {
            let probe = Probe::new();
            let blocked = match scenario {
                1 => Some("register_begin"),
                2 | 7 => Some("register_retain"),
                3 => Some("register_cas"),
                _ => None,
            };
            if let Some(kind) = blocked {
                probe.block(kind);
            }
            if scenario == 6 || scenario == 7 {
                probe.block("sdk_prefix");
            }
            let environment = Environment(Arc::clone(&probe));
            let mut reactor = AsyncReactor::with_lifetime_trace(
                Arc::new(move || environment.touch()),
                Arc::clone(&probe),
            );
            let mut mailbox = OperationMailbox::new(Default::default(), Arc::new(|| {}));
            let mut carrier = Box::new(TransferNativeCompletion::default());
            carrier.result = 7183;
            let raw = std::ptr::from_ref(&*carrier) as usize;
            // SAFETY: Box remains pinned until all registrations, control uses and physical copies end.
            let pointer = unsafe { CompletionPointer::new(raw) };
            let mut producer = if scenario == 0 || scenario == 6 {
                Some(publisher(raw, Arc::clone(&probe)))
            } else {
                None
            };
            if scenario == 0 {
                producer.take().unwrap().join().unwrap();
            }
            if scenario == 6 {
                probe.wait("sdk_prefix", 1);
                assert!(!pointer.is_done());
            }
            let mut receipts = vec![submit_receipt(&reactor, &mut mailbox, pointer)];
            if let Some(kind) = blocked {
                probe.wait(kind, 1);
                producer = Some(publisher(raw, Arc::clone(&probe)));
                if scenario == 7 {
                    probe.wait("sdk_prefix", 1);
                    assert!(!pointer.is_done());
                    probe.release("register_retain");
                    probe.release("sdk_prefix");
                } else {
                    producer.take().unwrap().join().unwrap();
                    probe.release(kind);
                }
            }
            probe.wait("registered", 1);
            if scenario == 3 {
                let first = probe
                    .events()
                    .into_iter()
                    .find(|e| e.starts_with("registered,"))
                    .unwrap();
                assert!(
                    first.ends_with(",1,1"),
                    "successful registration must observe producer release: {first}"
                );
            }
            if scenario == 5 {
                publisher(raw, Arc::clone(&probe)).join().unwrap();
            }
            if count == 2 {
                receipts.push(submit_receipt(&reactor, &mut mailbox, pointer));
                probe.wait("registered", 2);
            }
            if scenario == 4 {
                producer = Some(publisher(raw, Arc::clone(&probe)));
            }
            if scenario == 6 {
                assert!(!pointer.is_done());
                assert!(reactor.try_completion().unwrap().is_none());
                probe.release("sdk_prefix");
            }
            if let Some(thread) = producer {
                thread.join().unwrap();
            }
            probe.record("close_admission", 0, 0, 0);
            for _ in &receipts {
                reactor.wait_completion(Duration::from_secs(10)).unwrap();
            }
            mailbox.take_resolutions().unwrap();
            for (id, _) in &receipts {
                mailbox.settle(*id).unwrap();
            }
            receipts.clear();
            drop(carrier);
            probe.record("carrier_free", 0, 0, 0);
            reactor.shutdown();
            drop(reactor);
            assert!(mailbox.resources().used.is_baseline());
            let events = probe.events();
            check_reference_roles(count, &events);
            if count == 2 && matches!(scenario, 4 | 6) {
                assert!(
                    events
                        .iter()
                        .any(|e| e.starts_with("register_cas,") && e.ends_with(",0,2"))
                );
            }
            println!("REGISTRATION Rust scenario={scenario} waiters={count}");
            check_trace(Binding::RustChannel, count, &events);
        }
    }
}

#[test]
fn rust_production_events_refine_the_composed_model() {
    for count in [1, 2] {
        for scenario in 0..3 {
            let probe = Probe::new();
            if scenario == 1 {
                probe.block("notify_enter");
            }
            if scenario == 2 {
                probe.block("worker_wake_begin");
            }
            let environment = Environment(Arc::clone(&probe));
            let wake = Arc::new(move || {
                environment.touch();
            });
            let mut reactor = AsyncReactor::with_lifetime_trace(wake, Arc::clone(&probe));
            let mut mailbox = OperationMailbox::new(Default::default(), Arc::new(|| {}));
            let mut carrier = Box::new(TransferNativeCompletion::default());
            carrier.result = 7183;
            let raw = std::ptr::from_ref(&*carrier) as usize;
            // SAFETY: the Box stays pinned until every receipt has physically copied.
            let pointer = unsafe { CompletionPointer::new(raw) };
            let mut ids = Vec::new();
            let mut operations = Vec::new();
            for i in 0..count {
                if i == 1 && scenario == 1 {
                    break;
                }
                let ticket = mailbox.admit(TaskId(1), "trace".into()).unwrap();
                ids.push(ticket.operation);
                operations.push(Some(
                    reactor.submit_native_transfer(ticket, pointer).unwrap(),
                ));
                if i == 0 {
                    probe.wait("registered", 1);
                }
            }
            probe.wait("registered", operations.len());
            if scenario != 0 {
                assert_eq!(
                    if scenario == 1 {
                        operations[0].as_ref().unwrap().request_cancel()
                    } else {
                        operations[0].as_ref().unwrap().request_timeout()
                    },
                    ResolveResult::Won
                );
            }
            let publisher_probe = Arc::clone(&probe);
            let producer = std::thread::spawn(move || {
                publisher_probe.record("publish_begin", 0, 0, 0);
                // SAFETY: unique producer, pinned through publication. Its later
                // notification uses only the transferred target, never the carrier.
                unsafe {
                    NotifiedNativeCompletion::publish_with_lifetime_trace(
                        raw as *mut NotifiedNativeCompletion,
                        &publisher_probe,
                    )
                };
                publisher_probe.record("publish_end", 0, 0, 0);
            });
            if scenario == 1 {
                probe.wait("notify_enter", 1);
                if count == 2 {
                    let ticket = mailbox.admit(TaskId(1), "late receipt".into()).unwrap();
                    ids.push(ticket.operation);
                    operations.push(Some(
                        reactor.submit_native_transfer(ticket, pointer).unwrap(),
                    ));
                }
                reactor.trace_progress().unwrap();
            }
            probe.record("close_admission", 0, 0, 0);
            if scenario == 2 {
                reactor.wait_completion(Duration::from_secs(10)).unwrap();
                probe.wait("worker_wake_begin", 1);
                mailbox.take_resolutions().unwrap();
                mailbox.settle(ids[0]).unwrap();
                drop(operations[0].take());
                assert!(
                    !probe
                        .events()
                        .iter()
                        .any(|e| e.starts_with("record_release,"))
                );
                let p = Arc::clone(&probe);
                let release = std::thread::spawn(move || {
                    p.wait("shutdown", 1);
                    p.release("worker_wake_begin");
                });
                reactor.shutdown();
                release.join().unwrap();
                for id in ids.iter().skip(1) {
                    reactor.wait_completion(Duration::from_secs(10)).unwrap();
                    mailbox.take_resolutions().unwrap();
                    mailbox.settle(*id).unwrap();
                }
            } else {
                for _ in &ids {
                    reactor.wait_completion(Duration::from_secs(10)).unwrap();
                }
                if scenario == 0 {
                    assert_eq!(
                        operations[0].as_ref().unwrap().request_cancel(),
                        ResolveResult::Late
                    );
                }
                mailbox.take_resolutions().unwrap();
                for id in &ids {
                    mailbox.settle(*id).unwrap();
                }
            }
            operations.clear();
            drop(carrier);
            probe.record("carrier_free", 0, 0, 0);
            if scenario != 2 {
                reactor.shutdown();
            }
            if scenario == 1 {
                probe.wait("env_free", 1);
                probe.release("notify_enter");
            }
            producer.join().unwrap();
            drop(reactor);
            assert!(mailbox.resources().used.is_baseline());
            check_trace(Binding::RustChannel, count, &probe.events());
        }
    }
}

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn c_production_events_refine_the_composed_model_with_sanitizers() {
    c_fixture_refines("composed_lifetime_trace", 6);
}
#[test]
fn c_registration_transfer_and_sdk_exchange_races_refine_the_model_with_sanitizers() {
    c_fixture_refines("registration_lifetime_trace", 16);
}
fn c_fixture_refines(fixture: &str, expected_cases: usize) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let tmp =
        Temporary(std::env::temp_dir().join(format!("rinzu-{fixture}-{}", std::process::id())));
    std::fs::create_dir_all(&tmp.0).unwrap();
    for flags in [
        vec!["-O2"],
        vec![
            "-O1",
            "-g",
            "-fsanitize=address,undefined",
            "-fno-sanitize-recover=all",
        ],
    ] {
        let binary = tmp.0.join("trace");
        let build = process::bounded_tool(
            native::clang()
                .args(["-std=c11", "-pthread", "-Wall", "-Wextra", "-Werror"])
                .args(&flags)
                .arg("-I")
                .arg(root.join("include"))
                .arg(root.join(format!("crates/runtime/tests/fixtures/{fixture}.c")))
                .arg("-o")
                .arg(&binary),
        );
        assert!(build.status.success(), "{build:?}");
        let result = process::bounded_tool(
            Command::new(&binary)
                .env("RINZU_NATIVE_COMPLETION_NOTIFY", "1")
                .env("RINZU_NATIVE_COMPLETION_DIRECT", "1")
                .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
                .env("UBSAN_OPTIONS", "halt_on_error=1"),
        );
        assert!(result.status.success(), "{result:?}");
        let mut count = None;
        let mut events = Vec::new();
        let mut cases = 0;
        for line in String::from_utf8(result.stdout).unwrap().lines() {
            if let Some(start) = line.strip_prefix("BEGIN ") {
                assert!(count.is_none(), "nested C trace");
                count = Some(start.split_whitespace().next().unwrap().parse().unwrap());
            } else if line == "END" {
                let receipts = count.take().expect("END without BEGIN");
                check_reference_roles(receipts, &events);
                check_trace(Binding::CDirect, receipts, &events);
                events.clear();
                cases += 1;
            } else {
                events.push(line.to_owned());
            }
        }
        assert!(count.is_none() && events.is_empty(), "truncated C trace");
        assert_eq!(cases, expected_cases);
    }
}

#[test]
fn rust_rejects_control_in_the_forced_claim_to_physical_window() {
    for count in [1, 2] {
        for timeout in [false, true] {
            let probe = Probe::new();
            let environment = Environment(Arc::clone(&probe));
            let mut reactor = AsyncReactor::with_lifetime_trace(
                Arc::new(move || environment.touch()),
                Arc::clone(&probe),
            );
            let mut mailbox = OperationMailbox::new(Default::default(), Arc::new(|| {}));
            let mut carrier = Box::new(TransferNativeCompletion::default());
            carrier.result = 7183;
            let raw = std::ptr::from_ref(&*carrier) as usize;
            // SAFETY: pinned until every copy and control use has returned.
            let pointer = unsafe { CompletionPointer::new(raw) };
            let mut receipts = Vec::new();
            for _ in 0..count {
                receipts.push(submit_receipt(&reactor, &mut mailbox, pointer));
            }
            probe.wait("registered", count);
            let first = receipts[0].0;
            probe.block_for("completion_claim", first.0);
            publisher(raw, Arc::clone(&probe)).join().unwrap();
            probe.wait_for("completion_claim", first.0, 1);
            assert_eq!(
                if timeout {
                    receipts[0].1.request_timeout()
                } else {
                    receipts[0].1.request_cancel()
                },
                ResolveResult::Late
            );
            assert!(reactor.try_completion().unwrap().is_none());
            assert!(mailbox.take_resolutions().unwrap().is_empty());
            assert_eq!(
                carrier
                    .completion
                    .base
                    .cancelled
                    .load(std::sync::atomic::Ordering::Acquire),
                0
            );
            assert!(
                !probe
                    .events()
                    .iter()
                    .any(|e| e.starts_with("physical,") || e.starts_with("env_free,"))
            );
            probe.release_for("completion_claim", first.0);
            probe.record("close_admission", 0, 0, 0);
            for _ in 0..count {
                reactor.wait_completion(Duration::from_secs(30)).unwrap();
            }
            mailbox.take_resolutions().unwrap();
            for (id, _) in &receipts {
                mailbox.settle(*id).unwrap();
            }
            receipts.clear();
            drop(carrier);
            probe.record("carrier_free", 0, 0, 0);
            reactor.shutdown();
            drop(reactor);
            assert!(mailbox.resources().used.is_baseline());
            let events = probe.events();
            let claim = events
                .iter()
                .position(|e| e.starts_with("completion_claim,"))
                .unwrap();
            let control = events
                .iter()
                .position(|e| e.starts_with("control_end,"))
                .unwrap();
            let physical = events
                .iter()
                .position(|e| e.starts_with("physical,"))
                .unwrap();
            assert!(claim < control && control < physical);
            let mut invalid = events.clone();
            invalid.swap(claim, physical);
            assert!(replay(Binding::RustChannel, count, 7183, &invalid, 100_000).is_err());
            println!("CLAIM Rust waiters={count} timeout={timeout}");
            check_trace(Binding::RustChannel, count, &events);
        }
    }
}
