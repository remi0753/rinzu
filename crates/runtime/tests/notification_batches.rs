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
    CapturedNativeWake, CompletionPointer, NotifiedNativeCompletion, TransferNativeCompletion,
};
use rinzu_runtime::reactor::AsyncReactor;
use rinzu_runtime::scheduler::{Resolution, TaskId};
use rinzu_runtime::standalone::OperationMailbox;
use rinzu_runtime_model::native_lifetime::{
    Binding, NotificationAction as A, NotificationBatches, Protocol,
};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

const BATCHES: usize = 3;
const MEMBERS: usize = BATCHES * 2;
type Model = NotificationBatches<BATCHES>;
const WAIT: Duration = Duration::from_secs(30);

fn cases() -> impl Iterator<Item = ([usize; MEMBERS], usize, bool)> {
    (0..MEMBERS)
        .flat_map(|offset| {
            [false, true].map(|reverse| {
                std::array::from_fn(|i| (offset + if reverse { MEMBERS - i } else { i }) % MEMBERS)
            })
        })
        .flat_map(|order| {
            [0, 1].into_iter().flat_map(move |leader| {
                [false, true].map(move |sdk_returns_before_runtime| {
                    (order, leader, sdk_returns_before_runtime)
                })
            })
        })
}

fn advance(model: &mut Model, action: A) {
    assert!(model.actions().contains(&action), "{action:?} {model:?}");
    let before = model.clone();
    model.apply(action);
    before.check_step(action, model).unwrap();
    model.check().unwrap();
}

fn compare_c(
    output: &str,
    order: [usize; MEMBERS],
    leader: usize,
    sdk_returns_before_runtime: bool,
) {
    let mut model = Model::new(Binding::CDirect);
    let mut actual_order = Vec::new();
    for line in output.lines() {
        let fields = line.split(',').collect::<Vec<_>>();
        let [event, batch, member, references] = fields[..] else {
            panic!("malformed trace {line}");
        };
        let [batch, member, references] =
            [batch, member, references].map(|n| n.parse::<usize>().unwrap());
        let before = model.references();
        let action = match event {
            "registered" => {
                assert_eq!(before, references);
                continue;
            }
            "capture" => A::Capture(batch, member),
            "merge" => {
                assert_eq!(member, 1 - leader);
                A::Merge(batch)
            }
            "settle" => {
                actual_order.push(batch * 2 + member);
                A::Settle(batch, member)
            }
            "begin" => {
                assert_eq!(member, usize::from(model.environment_attached()));
                A::Begin(batch)
            }
            "end" => A::End(batch),
            "return" => {
                assert_eq!(member, leader);
                A::Return(batch)
            }
            "detach" => A::Detach,
            "runtime_return" => {
                assert_eq!(before == 1, sdk_returns_before_runtime);
                A::ReturnRuntime
            }
            "destroy" => A::Destroy,
            _ => panic!("unknown event {event}"),
        };
        advance(&mut model, action);
        let expected = if matches!(action, A::Return(_) | A::ReturnRuntime) {
            before
        } else {
            model.references()
        };
        assert_eq!(references, expected, "{line}\n{output}");
    }
    assert_eq!(actual_order, order);
    assert!(model.terminal(), "{output}");
}

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn c_three_merged_batches_share_a_target_without_consuming_other_leases() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let scratch = Scratch(
        std::env::temp_dir().join(format!("rinzu-three-wake-batches-{}", std::process::id())),
    );
    std::fs::create_dir(&scratch.0).unwrap();
    for sanitized in [false, true] {
        let binary = scratch.0.join("three-batches");
        let mut cc = native::clang();
        cc.args([
            "-std=c11", "-O1", "-g", "-pthread", "-Wall", "-Wextra", "-Werror",
        ])
        .arg("-I")
        .arg(root.join("../../include"))
        .arg(root.join("tests/fixtures/notification_batches.c"))
        .arg("-o")
        .arg(&binary);
        if sanitized {
            cc.args(["-fsanitize=address,undefined", "-fno-sanitize-recover=all"]);
        }
        let build = process::bounded_tool(&mut cc);
        assert!(build.status.success(), "{build:?}");
        for (order, leader, sdk_returns_before_runtime) in cases() {
            let plan = order.iter().map(usize::to_string).collect::<String>();
            let result = process::bounded(
                Command::new(&binary)
                    .args([
                        plan,
                        leader.to_string(),
                        u8::from(sdk_returns_before_runtime).to_string(),
                    ])
                    .env("RINZU_NATIVE_COMPLETION_NOTIFY", "1")
                    .env("RINZU_NATIVE_COMPLETION_DIRECT", "1")
                    .env("ASAN_OPTIONS", native::asan_options_after_teardown())
                    .env("UBSAN_OPTIONS", "halt_on_error=1"),
            );
            assert!(
                result.status.success(),
                "{order:?} leader={leader} {result:?}"
            );
            assert!(result.stderr.is_empty(), "{result:?}");
            compare_c(
                std::str::from_utf8(&result.stdout).unwrap(),
                order,
                leader,
                sdk_returns_before_runtime,
            );
            println!(
                "THREE BATCHES C sanitized={sanitized} order={order:?} leader={leader} sdk_returns_before_runtime={sdk_returns_before_runtime}"
            );
        }
    }
}

enum SdkCommand {
    Capture(usize),
    Merge,
    Notify,
    Return,
}

struct Publisher {
    command: mpsc::Sender<SdkCommand>,
    reply: mpsc::Receiver<usize>,
    worker: JoinHandle<()>,
}

fn publisher(batch: usize, leader: usize, carriers: [usize; 2], probe: Arc<Probe>) -> Publisher {
    let (command, commands) = mpsc::channel();
    let (reply, replies) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut leases: [Option<CapturedNativeWake>; 2] = [None, None];
        for command in commands {
            match command {
                SdkCommand::Capture(member) => {
                    let _scope = Probe::producer(100 + batch as u64);
                    // SAFETY: the Owner pins both allocations through their sole final exchange and copies.
                    let lease = unsafe {
                        NotifiedNativeCompletion::capture_with_lifetime_trace(
                            carriers[member] as *mut NotifiedNativeCompletion,
                            &probe,
                        )
                    }
                    .unwrap();
                    let address = lease.address();
                    assert!(leases[member].replace(lease).is_none());
                    reply.send(address).unwrap();
                    continue;
                }
                SdkCommand::Merge => {
                    let _scope = Probe::producer(200 + batch as u64);
                    assert_eq!(
                        leases[0].as_ref().unwrap().address(),
                        leases[1].as_ref().unwrap().address()
                    );
                    drop(leases[1 - leader].take().unwrap());
                }
                SdkCommand::Notify => {
                    let _scope = Probe::producer(100 + batch as u64);
                    leases[leader].as_ref().unwrap().notify();
                }
                SdkCommand::Return => {
                    let _scope = Probe::producer(100 + batch as u64);
                    drop(leases[leader].take().unwrap());
                    reply.send(0).unwrap();
                    return;
                }
            }
            reply.send(0).unwrap();
        }
    });
    Publisher {
        command,
        reply: replies,
        worker,
    }
}

fn check_rust_case(order: [usize; MEMBERS], leader: usize, sdk_returns_before_runtime: bool) {
    let timeline = Arc::new(DomainTrace::default());
    let probe = timeline.probe(0);
    let environment = Environment(Arc::clone(&probe));
    let reactor = AsyncReactor::with_lifetime_trace(
        Arc::new(move || {
            std::hint::black_box(&environment);
        }),
        Arc::clone(&probe),
    );
    let mut mailbox = OperationMailbox::new(Default::default(), Arc::new(|| {}));
    let carriers: [_; MEMBERS] = std::array::from_fn(|i| {
        let mut data = TransferNativeCompletion {
            result: i as i64 + 7183,
            ..Default::default()
        };
        if i % 2 == 1 {
            data.completion = NotifiedNativeCompletion::default();
        }
        Box::new(data)
    });
    let addresses = carriers
        .each_ref()
        .map(|carrier| std::ptr::from_ref(&carrier.completion) as usize);
    let mut ids = Vec::new();
    let receipts: [_; MEMBERS] = std::array::from_fn(|i| {
        let ticket = mailbox.admit(TaskId(1), "shared batch".into()).unwrap();
        ids.push(ticket.operation);
        // SAFETY: the provider owns these pinned backing records through the physical drains.
        let pointer = unsafe { CompletionPointer::new(addresses[i]) };
        if i % 2 == 0 {
            reactor.submit_native_transfer(ticket, pointer)
        } else {
            reactor.submit_native_completion(ticket, pointer)
        }
        .unwrap()
    });
    probe.wait("registered", MEMBERS);
    reactor.trace_progress().unwrap();
    probe.wait("trace_progress_refs", 1);
    let mut model = Model::new(Binding::RustChannel);
    let sample = probe
        .events()
        .into_iter()
        .find(|row| row.starts_with("trace_progress_refs,0,"))
        .unwrap();
    let fields: Vec<_> = sample.split(',').collect();
    let target = fields[2].parse::<usize>().unwrap();
    assert_eq!(fields[3].parse::<usize>().unwrap(), model.references());

    let providers: [_; BATCHES] = std::array::from_fn(|b| {
        publisher(
            b,
            leader,
            [addresses[b * 2], addresses[b * 2 + 1]],
            Arc::clone(&probe),
        )
    });
    let capture = |model: &mut Model, b, m| {
        advance(model, A::Capture(b, m));
        providers[b].command.send(SdkCommand::Capture(m)).unwrap();
        assert_eq!(providers[b].reply.recv_timeout(WAIT).unwrap(), target);
    };
    let merge = |model: &mut Model, b| {
        let before = model.references();
        advance(model, A::Merge(b));
        providers[b].command.send(SdkCommand::Merge).unwrap();
        providers[b].reply.recv_timeout(WAIT).unwrap();
        assert!(
            probe
                .events()
                .contains(&format!("producer_refs,{},{target},{before}", 200 + b))
        );
    };
    for b in 0..BATCHES {
        capture(&mut model, b, 0);
    }
    probe.block_for("notify_enter", 100);
    for (b, provider) in providers.iter().enumerate() {
        capture(&mut model, b, 1);
        merge(&mut model, b);
        if b < 2 {
            advance(&mut model, A::Begin(b));
            provider.command.send(SdkCommand::Notify).unwrap();
            probe.wait_for("notify_enter", 100 + b as u64, 1);
            if b == 1 {
                provider.reply.recv_timeout(WAIT).unwrap();
                advance(&mut model, A::End(b));
            }
        }
    }
    assert!(
        !probe
            .events()
            .iter()
            .any(|row| row.starts_with("notify_leave,100,"))
    );
    reactor.trace_progress().unwrap();
    let mut seen = [false; MEMBERS];
    for _ in &ids {
        let packet = reactor.wait_completion(WAIT).unwrap();
        let index = ids.iter().position(|&id| id == packet.operation).unwrap();
        assert!(!std::mem::replace(&mut seen[index], true));
    }
    mailbox.take_resolutions().unwrap();
    for i in order {
        assert_eq!(
            mailbox.settle(ids[i]).unwrap(),
            Resolution::Completed(Ok(()))
        );
        assert_eq!(
            receipts[i].native_transfer_result(),
            (i % 2 == 0).then_some(i as i64 + 7183)
        );
        advance(&mut model, A::Settle(i / 2, i % 2));
    }
    drop(receipts);
    drop(carriers);
    let resume_first = |model: &mut Model| {
        probe.release_for("notify_enter", 100);
        providers[0].reply.recv_timeout(WAIT).unwrap();
        advance(model, A::End(0));
    };
    if !sdk_returns_before_runtime {
        resume_first(&mut model);
    }
    let before = model.references();
    probe.block_for("runtime_reference_release", 0);
    let shutdown = std::thread::spawn(move || drop(reactor));
    probe.wait_for("runtime_reference_release", 0, 1);
    advance(&mut model, A::Detach);
    assert!(
        probe
            .events()
            .contains(&format!("runtime_refs,0,{target},{before}"))
    );
    assert!(probe.events().contains(&"detach,0,0,0".to_owned()));
    if sdk_returns_before_runtime {
        assert!(
            !probe
                .events()
                .iter()
                .any(|row| row.starts_with("notify_leave,100,"))
        );
        resume_first(&mut model);
    }

    let return_sdk = |model: &mut Model| {
        for (b, publisher) in providers.into_iter().enumerate().rev() {
            if b == 2 {
                advance(model, A::Begin(b));
                publisher.command.send(SdkCommand::Notify).unwrap();
                publisher.reply.recv_timeout(WAIT).unwrap();
                probe.wait_for("notify_leave", 100 + b as u64, 1);
                advance(model, A::End(b));
            }
            let before = model.references();
            publisher.command.send(SdkCommand::Return).unwrap();
            publisher.reply.recv_timeout(WAIT).unwrap();
            publisher.worker.join().unwrap();
            assert!(
                probe
                    .events()
                    .contains(&format!("producer_refs,{},{target},{before}", 100 + b))
            );
            advance(model, A::Return(b));
        }
    };
    let return_runtime = |model: &mut Model| {
        probe.release_for("runtime_reference_release", 0);
        shutdown.join().unwrap();
        probe.wait("env_free", 1);
        advance(model, A::ReturnRuntime);
    };
    if sdk_returns_before_runtime {
        return_sdk(&mut model);
        assert_eq!(model.references(), 1);
        assert!(
            !probe
                .events()
                .iter()
                .any(|row| row.starts_with("target_free,"))
        );
        return_runtime(&mut model);
    } else {
        return_runtime(&mut model);
        return_sdk(&mut model);
    }
    probe.wait("target_free", 1);
    let events = probe.events();
    let (_, after_detach) =
        events.split_at(events.iter().position(|row| row == "detach,0,0,0").unwrap());
    assert!(
        !after_detach
            .iter()
            .any(|row| row.starts_with("sdk_progress,"))
    );
    assert!(after_detach.contains(&format!("target_identity_free,0,{target},0")));
    advance(&mut model, A::Destroy);
    assert!(model.terminal());
}

#[test]
fn rust_three_merged_batches_overlap_on_one_reactor_target() {
    for (order, leader, sdk_returns_before_runtime) in cases() {
        check_rust_case(order, leader, sdk_returns_before_runtime);
        println!(
            "THREE BATCHES Rust order={order:?} leader={leader} sdk_returns_before_runtime={sdk_returns_before_runtime}"
        );
    }
}
