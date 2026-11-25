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
use rinzu_runtime_model::native_lifetime::{Binding, SharedTopology, replay_shared};
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::Ordering};
use std::time::Duration;
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[path = "fixtures/coalesced_lifetime.rs"]
mod coalesced;

const VALUES: [i64; 3] = [7183, 29, 65537];
struct Environment(Arc<Probe>);
impl Drop for Environment {
    fn drop(&mut self) {
        self.0.record("env_free", 0, 0, 0);
    }
}
impl Environment {
    fn touch(&self) {
        self.0.record("context_touch", 0, 0, 0);
    }
}
fn check(binding: Binding, topology: SharedTopology, events: &[String]) {
    let payload: &[i64] = if topology == SharedTopology::Coalesced {
        &[7183, 0]
    } else {
        &VALUES[..topology.count()]
    };
    let replay = |e: &[String], limit| replay_shared(binding, topology, payload, e, limit);
    let r = replay(events, 100_000).unwrap_or_else(|e| panic!("{e}\n{}", events.join("\n")));
    println!(
        "SHARED TRACE {binding:?} {topology:?} events={} frontier={}",
        r.events, r.maximum_frontier
    );
    let mut mutations = 0;
    for prefix in [
        "receipt,",
        "register_retain,",
        "completion_claim,",
        "register_cas,",
        "sdk_prefix,",
        "sdk_exchange,",
        "producer_released,",
        "physical,",
        "generation_status,",
        "carrier_free,",
    ] {
        let mut e = events.to_vec();
        e.remove(e.iter().position(|e| e.starts_with(prefix)).unwrap());
        assert!(replay(&e, 100_000).is_err(), "accepted missing {prefix}");
        mutations += 1;
    }
    for prefix in ["producer_release,", "register_cas,", "receipt,"] {
        let mut e = events.to_vec();
        let i = e.iter().position(|e| e.starts_with(prefix)).unwrap();
        e.insert(i, e[i].clone());
        assert!(replay(&e, 100_000).is_err());
        mutations += 1;
    }
    for (prefix, column, value) in [
        ("registered,", 3, "9"),
        ("copied,", 2, "19"),
        ("sdk_exchange,", 1, "999"),
        ("notify_enter,", 1, "999"),
    ] {
        let mut e = events.to_vec();
        let row = e.iter_mut().find(|e| e.starts_with(prefix)).unwrap();
        let mut f: Vec<_> = row.split(',').collect();
        f[column] = value;
        *row = f.join(",");
        assert!(replay(&e, 100_000).is_err());
        mutations += 1;
    }
    let mut aggregate = events.to_vec();
    let free = aggregate
        .iter()
        .position(|e| e.starts_with("target_free,"))
        .unwrap();
    aggregate.remove(free);
    let first = aggregate
        .iter()
        .position(|e| e.starts_with("producer_release,"))
        .unwrap();
    let multiple = events
        .iter()
        .filter(|e| e.starts_with("producer_release,"))
        .count()
        >= 2;
    aggregate.insert(first + usize::from(multiple), "target_free,0,0,0".into());
    assert!(replay(&aggregate, 100_000).is_err());
    mutations += 1;
    let mut undersized = events.to_vec();
    let row = undersized
        .iter_mut()
        .find(|e| e.starts_with("registered,"))
        .unwrap();
    let f: Vec<_> = row.split(',').collect();
    *row = format!("registered,{},{},1", f[1], f[2]);
    assert!(replay(&undersized, 100_000).is_err());
    mutations += 1;
    let mut early = events.to_vec();
    early.insert(1, "target_free,0,0,0".into());
    assert!(replay(&early, 100_000).is_err());
    mutations += 1;
    let mut early = events.to_vec();
    early.insert(1, "env_free,0,0,0".into());
    assert!(replay(&early, 100_000).is_err());
    mutations += 1;
    let mut unknown = events.to_vec();
    unknown.insert(1, "invented,0,0,0".into());
    assert!(replay(&unknown, 100_000).is_err());
    mutations += 1;
    assert!(replay(&events[..events.len() - 1], 100_000).is_err());
    mutations += 1;
    assert!(replay(events, 1).is_err());
    mutations += 1;
    if topology == SharedTopology::Coalesced {
        let position = |prefix: &str| events.iter().position(|e| e.starts_with(prefix)).unwrap();
        let mut bad = events.to_vec();
        bad.remove(position("merge_wake,101,"));
        assert!(replay(&bad, 100_000).is_err(), "accepted a missing merge");
        let mut bad = events.to_vec();
        bad[position("merge_wake,101,")] = "merge_wake,101,102,0".into();
        assert!(
            replay(&bad, 100_000).is_err(),
            "accepted a foreign batch leader"
        );
        let mut bad = events.to_vec();
        let row = bad.remove(position("notify_enter,100,"));
        bad.insert(position("sdk_exchange,101,"), row);
        assert!(
            replay(&bad, 100_000).is_err(),
            "accepted a wake before the covered exchange"
        );
        let mut bad = events.to_vec();
        let notify = position("notify_enter,100,");
        bad.insert(notify, events[notify].replacen(",100,", ",101,", 1));
        assert!(replay(&bad, 100_000).is_err(), "accepted a second SDK wake");
        mutations += 4;
    }
    if topology.replacement().is_some() {
        for (prefix, column, value) in [("alias_result,", 2, "65537"), ("reuse,", 2, "101")] {
            let mut e = events.to_vec();
            let row = e.iter_mut().find(|e| e.starts_with(prefix)).unwrap();
            let mut f: Vec<_> = row.split(',').collect();
            f[column] = value;
            *row = f.join(",");
            assert!(replay(&e, 100_000).is_err());
            mutations += 1;
        }
        let mut e = events.to_vec();
        let row = e
            .iter_mut()
            .find(|e| e.starts_with("control_end,") && e.ends_with(",0,0"))
            .unwrap();
        *row = row.replace(",0,0", ",1,0");
        assert!(replay(&e, 100_000).is_err());
        mutations += 1;
    }
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
    assert!(replay(&wrong_claim, 100_000).is_err());
    mutations += 1;
    println!("SHARED REJECT {binding:?} {topology:?} mutations={mutations}");
}
fn publish(raw: usize, token: u64, probe: Arc<Probe>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let _scope = Probe::producer(token);
        probe.record("publish_begin", 0, 0, 0);
        // SAFETY: the caller pins through registration/control/copy; after exchange
        // this helper uses only its retained target and the independent observer.
        unsafe {
            NotifiedNativeCompletion::publish_with_lifetime_trace(
                raw as *mut NotifiedNativeCompletion,
                &probe,
            )
        };
        probe.record("publish_end", 0, 0, 0);
    })
}
#[test]
fn rust_shared_carriers_and_reused_generations_refine_the_model() {
    for topology in [
        SharedTopology::Carriers,
        SharedTopology::Reuse,
        SharedTopology::Mixed,
    ] {
        for scenario in 0..5 {
            let probe = Probe::new();
            let env = Environment(Arc::clone(&probe));
            let mut reactor = AsyncReactor::with_lifetime_trace(
                Arc::new(move || env.touch()),
                Arc::clone(&probe),
            );
            let mut mailbox = OperationMailbox::new(Default::default(), Arc::new(|| {}));
            let mut storage = [
                Box::new(TransferNativeCompletion::default()),
                Box::new(TransferNativeCompletion::default()),
            ];
            let mut receipts: Vec<(
                rinzu_runtime::scheduler::OperationId,
                rinzu_runtime::reactor::ReactorOperation,
            )> = Vec::new();
            let mut producers = Vec::new();
            for (g, &value) in VALUES.iter().enumerate().take(topology.count()) {
                let replacement = topology.replacement() == Some(g);
                let s = topology.storage(g);
                let token = 100 + u64::try_from(g).unwrap();
                *storage[s] = TransferNativeCompletion::default();
                storage[s].result = value;
                let raw = std::ptr::from_ref(&*storage[s]) as usize;
                probe.observe_carrier(raw, token);
                probe.record(
                    if replacement { "reuse" } else { "generation" },
                    token,
                    if replacement {
                        100
                    } else {
                        i64::try_from(s).unwrap()
                    },
                    i64::from(replacement),
                );
                // SAFETY: the Box stays at this address until all accesses in the generation end.
                let pointer = unsafe { CompletionPointer::new(raw) };
                let ticket = mailbox.admit(TaskId(1), "shared target".into()).unwrap();
                let id = ticket.operation;
                let early = scenario == 4 && g == topology.count() - 1;
                if early {
                    probe.block_for("register_begin", id.0);
                }
                probe.block_for("notify_enter", token);
                let op = reactor.submit_native_transfer(ticket, pointer).unwrap();
                if early {
                    probe.wait_for("register_begin", id.0, 1);
                    let thread = publish(raw, token, Arc::clone(&probe));
                    thread.join().unwrap();
                    probe.release_for("register_begin", id.0);
                    producers.push(None);
                } else {
                    probe.wait_for("registered", id.0, 1);
                }
                if replacement {
                    let (old_id, old) = &receipts[0];
                    assert_eq!(old.native_transfer_result(), Some(VALUES[0]));
                    probe.record("alias_result", old_id.0, VALUES[0], 0);
                    assert_eq!(old.request_cancel(), ResolveResult::Late);
                    assert_eq!(
                        storage[s].completion.base.cancelled.load(Ordering::Acquire),
                        0
                    );
                }
                if scenario == 3 && g == topology.count() - 1 {
                    assert_eq!(op.request_cancel(), ResolveResult::Won);
                }
                if !early {
                    let thread = publish(raw, token, Arc::clone(&probe));
                    probe.wait_for("notify_enter", token, 1);
                    producers.push(Some(thread));
                    reactor.trace_progress().unwrap();
                }
                reactor.wait_completion(Duration::from_secs(30)).unwrap();
                probe.wait_for("worker_wake_end", id.0, 1);
                assert_eq!(op.native_transfer_result(), Some(value));
                mailbox.take_resolutions().unwrap();
                mailbox.settle(id).unwrap();
                probe.record(
                    "generation_status",
                    token,
                    i64::from(storage[s].completion.base.cancelled.load(Ordering::Acquire) != 0),
                    i64::from(pointer.is_done()),
                );
                probe.record("close_admission", token, 0, 0);
                probe.record("carrier_free", token, 0, 0);
                receipts.push((id, op));
            }
            drop(storage);
            receipts.clear();
            reactor.shutdown();
            drop(reactor);
            probe.wait("env_free", 1);
            assert!(!probe.events().iter().any(|e| e.starts_with("target_free,")));
            if scenario == 2 {
                for g in 0..topology.count() {
                    probe.release_for("notify_enter", 100 + u64::try_from(g).unwrap());
                }
            }
            let order: Vec<_> = if scenario == 1 {
                (0..topology.count()).rev().collect()
            } else {
                (0..topology.count()).collect()
            };
            let captured = producers.iter().filter(|p| p.is_some()).count();
            let mut finished = 0;
            for g in order {
                if let Some(thread) = producers[g].take() {
                    if scenario != 2 {
                        probe.release_for("notify_enter", 100 + u64::try_from(g).unwrap());
                    }
                    thread.join().unwrap();
                    finished += 1;
                    if scenario != 2 && finished < captured {
                        assert!(!probe.events().iter().any(|e| e.starts_with("target_free,")));
                    }
                }
            }
            assert_eq!(
                probe
                    .events()
                    .iter()
                    .filter(|e| e.starts_with("target_free,"))
                    .count(),
                1
            );
            assert!(mailbox.resources().used.is_baseline());
            let e = probe.events();
            if scenario != 4 {
                let max = e
                    .iter()
                    .filter(|e| e.starts_with("registered,"))
                    .map(|e| e.split(',').nth(3).unwrap().parse::<usize>().unwrap())
                    .max()
                    .unwrap();
                assert_eq!(max, topology.count() + 1);
            }
            println!("SHARED Rust {topology:?} scenario={scenario}");
            check(Binding::RustChannel, topology, &e);
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
fn c_shared_carriers_and_reused_generations_refine_the_model_with_sanitizers() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let tmp =
        Temporary(std::env::temp_dir().join(format!("rinzu-shared-trace-{}", std::process::id())));
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
                .arg(root.join("crates/runtime/tests/fixtures/shared_target_lifetime_trace.c"))
                .arg("-o")
                .arg(&binary),
        );
        assert!(build.status.success(), "{build:?}");
        let result = process::bounded_tool(
            std::process::Command::new(&binary)
                .env("RINZU_NATIVE_COMPLETION_NOTIFY", "1")
                .env("RINZU_NATIVE_COMPLETION_DIRECT", "1")
                .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
                .env("UBSAN_OPTIONS", "halt_on_error=1"),
        );
        assert!(result.status.success(), "{result:?}");
        let mut topology = None;
        let mut events = Vec::new();
        let mut cases = 0;
        for line in String::from_utf8(result.stdout).unwrap().lines() {
            if let Some(start) = line.strip_prefix("BEGIN ") {
                assert!(topology.is_none());
                topology = Some(match start.split_whitespace().next().unwrap() {
                    "0" => SharedTopology::Carriers,
                    "1" => SharedTopology::Reuse,
                    "2" => SharedTopology::Mixed,
                    "3" => SharedTopology::Coalesced,
                    _ => panic!("invalid topology"),
                });
            } else if line == "END" {
                check(Binding::CDirect, topology.take().unwrap(), &events);
                events.clear();
                cases += 1;
            } else {
                events.push(line.to_owned());
            }
        }
        assert!(topology.is_none() && events.is_empty());
        assert_eq!(cases, 22);
    }
}
