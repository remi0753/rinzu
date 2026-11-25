use super::{Temporary, native, process};
use rinzu_runtime::lifetime_trace::{DomainTrace, Probe};
use rinzu_runtime::native_completion::{
    CapturedNativeWake, CompletionPointer, NotifiedNativeCompletion, TransferNativeCompletion,
};
use rinzu_runtime::reactor::AsyncReactor;
use rinzu_runtime::scheduler::{Resolution, TaskId};
use rinzu_runtime::standalone::OperationMailbox;
use rinzu_runtime_model::native_lifetime::{
    Domain, ParallelRegistration, ParallelRegistrationAction as A, Protocol,
};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, atomic::Ordering};
use std::time::Duration;

const CHILD: &str = "RINZU_TEST_PARALLEL_REGISTRATION";
const EARLY: usize = 0;
const BETWEEN: usize = 1;
const FOREIGN: usize = 2;
const FOREIGN_THEN_PUBLISH: usize = 3;

fn cases() -> impl Iterator<Item = [usize; 3]> {
    [2, 3].into_iter().flat_map(|version| {
        (0..2).flat_map(move |first| {
            (EARLY..=FOREIGN_THEN_PUBLISH).map(move |schedule| [version, first, schedule])
        })
    })
}

fn field(value: &str) -> i64 {
    value.parse().unwrap()
}

fn replay(stdout: &str) -> Result<ParallelRegistration, String> {
    let mut model = ParallelRegistration::default();
    let mut targets = [0; 2];
    let mut carriers = [0; 2];
    let mut destroyed = [false; 2];
    let mut returned = [false; 2];
    let mut cas_returns = [false; 2];
    let mut capture_observed = false;
    let mut reference_sample = false;
    for row in stdout.lines() {
        let fields: Vec<_> = row.split(',').collect();
        let [index @ ("0" | "1"), kind, _, a, b] = fields[..] else {
            continue;
        };
        let i = usize::try_from(field(index)).unwrap();
        let d = Domain::ALL[i];
        let step = |model: &mut ParallelRegistration, action| {
            if !model.actions().contains(&action) {
                return Err(format!("disabled action {action:?} at {row}"));
            }
            let before = model.clone();
            model.apply(action);
            before
                .check_step(action, model)
                .map_err(|error| format!("{error:?}"))?;
            model.check().map_err(|error| format!("{error:?}"))
        };
        let bound = || field(a) > 1 && field(a) == targets[i];
        match kind {
            "carrier_bind" | "receipt_carrier" => {
                if carriers[i] != 0
                    || field(a) <= 1
                    || (carriers[1 - i] != 0 && field(a) != carriers[1 - i])
                {
                    return Err(format!("distinct or repeated carrier binding {row}"));
                }
                carriers[i] = field(a);
            }
            "wake_bind" => {
                if targets[i] != 0 || field(a) <= 1 || field(a) == targets[1 - i] {
                    return Err(format!("aliased or repeated wake binding {row}"));
                }
                targets[i] = field(a);
            }
            "register_retain" if targets[i] != 0 => step(&mut model, A::Retain(d))?,
            "register_cas" => {
                step(&mut model, A::Cas(d))?;
                if model.cas_result(d) != Some((field(a), field(b))) {
                    return Err(format!("CAS outcome {row}"));
                }
            }
            "sdk_exchange" => {
                step(&mut model, A::Exchange)?;
                if field(a) != i64::from(model.captured().is_some()) {
                    return Err(format!("SDK exchange outcome {row}"));
                }
            }
            "capture_target" => {
                if capture_observed
                    || field(a) != model.captured().map_or(0, |target| targets[target.index()])
                {
                    return Err(format!("wrong captured target {row}"));
                }
                capture_observed = true;
            }
            "rollback_refs" => {
                step(&mut model, A::Rollback(d))?;
                if !bound() || field(b) != i64::from(model.references(d)) {
                    return Err(format!("wrong rollback count or target {row}"));
                }
                returned[i] = true;
            }
            "register_drop" if !returned[i] => return Err(format!("unobserved rollback {row}")),
            "registered" => {
                if cas_returns[i]
                    || field(a) != i64::from(!returned[i])
                    || field(b) != i64::from(model.references(d))
                {
                    return Err(format!("wrong registration return {row}"));
                }
                cas_returns[i] = true;
            }
            "notify_enter" if model.captured() != Some(d) => {
                return Err(format!("notify reached wrong Domain {row}"));
            }
            "producer_refs" => {
                if reference_sample
                    || !bound()
                    || model.captured() != Some(d)
                    || field(b) != i64::from(model.references(d))
                {
                    return Err(format!("wrong SDK reference {row}"));
                }
                reference_sample = true;
            }
            "producer_released" => {
                if !reference_sample || model.captured() != Some(d) {
                    return Err(format!("release of an unobserved SDK reference {row}"));
                }
                step(&mut model, A::ReleaseSdk)?;
            }
            "target_identity_free" => {
                if destroyed[i] || !bound() || !model.terminal() || model.references(d) != 1 {
                    return Err(format!("wrong target destroyed {row}"));
                }
                destroyed[i] = true;
            }
            _ => {}
        }
    }
    if !model.terminal()
        || targets.contains(&0)
        || carriers.contains(&0)
        || model.published() != capture_observed
    {
        return Err("incomplete registration race".into());
    }
    if model.rejected() {
        if cas_returns.iter().filter(|&&registered| registered).count() != 1
            || destroyed.contains(&true)
        {
            return Err("expected exactly one registration to return before rejection".into());
        }
    } else if cas_returns != [true; 2] || destroyed != [true; 2] {
        return Err("missing settlement or target destruction".into());
    }
    Ok(model)
}

fn check(stdout: &str, [version, first, schedule]: [usize; 3]) {
    let report = replay(stdout).unwrap_or_else(|error| panic!("{error}\n{stdout}"));
    assert_eq!(report.rejected(), schedule >= FOREIGN, "{stdout}");
    assert_eq!(report.published(), schedule != FOREIGN, "{stdout}");
    assert_eq!(
        report.captured(),
        (schedule == FOREIGN_THEN_PUBLISH).then_some(Domain::ALL[first]),
        "{stdout}"
    );
    if version == 3 && first == 1 && schedule == BETWEEN {
        let corrupt = stdout
            .lines()
            .map(|row| {
                if let Some((head, _)) = row.split_once(",rollback_refs,") {
                    format!("{head},rollback_refs,0,2,1")
                } else {
                    row.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(replay(&corrupt).is_err());
    }
}

fn rust_child([version, first, schedule]: [usize; 3]) {
    let trace = Arc::new(DomainTrace::default());
    let probes = [trace.probe(0), trace.probe(1)];
    let reactors = probes
        .each_ref()
        .map(|p| AsyncReactor::with_lifetime_trace(Arc::new(|| {}), Arc::clone(p)));
    let mut mailboxes =
        [(); 2].map(|()| OperationMailbox::new(Default::default(), Arc::new(|| {})));
    let unit = Box::new(NotifiedNativeCompletion::default());
    let mut scalar = Box::new(TransferNativeCompletion::default());
    scalar.result = 7183;
    let carrier = if version == 2 {
        &*unit
    } else {
        &scalar.completion
    };
    let raw = std::ptr::from_ref(carrier).cast_mut();
    // SAFETY: this provider allocation stays pinned through publication and all receipts.
    let pointer = unsafe { CompletionPointer::new(raw as usize) };
    for p in &probes {
        p.observe_carrier(raw as usize, 100);
    }
    let other = 1 - first;
    let [ticket0, ticket1] = mailboxes
        .each_mut()
        .map(|mailbox| mailbox.admit(TaskId(1), "register".into()).unwrap());
    let ids = [ticket0.operation, ticket1.operation];
    let receipts = [ticket0, ticket1]
        .into_iter()
        .enumerate()
        .map(|(d, ticket)| {
            probes[d].block_for("register_retain", ids[d].0);
            if d == other && schedule >= FOREIGN {
                probes[d].block_for("register_drop", ids[d].0);
                if schedule == FOREIGN_THEN_PUBLISH {
                    probes[d].block_for("register_cas", ids[d].0);
                }
            }
            if version == 2 {
                reactors[d].submit_native_completion(ticket, pointer)
            } else {
                reactors[d].submit_native_transfer(ticket, pointer)
            }
            .unwrap()
        })
        .collect::<Vec<_>>();
    for d in 0..2 {
        probes[d].wait_for("register_retain", ids[d].0, 1);
    }
    assert_eq!(carrier.notification.load(Ordering::Acquire), 0);
    let publish = |p: &Probe| {
        // SAFETY: the SDK runs this single final exchange with the carrier still pinned.
        let captured = unsafe { NotifiedNativeCompletion::capture_with_lifetime_trace(raw, p) };
        p.record(
            "capture_target",
            0,
            captured
                .as_ref()
                .map_or(0, |value| i64::try_from(value.address()).unwrap()),
            0,
        );
        assert_eq!(carrier.notification.load(Ordering::Acquire), 1);
        captured
    };
    if schedule == EARLY {
        assert!(publish(&probes[first]).is_none());
    }
    probes[first].release_for("register_retain", ids[first].0);
    probes[first].wait_for("registered", ids[first].0, 1);
    let installed = carrier.notification.load(Ordering::Acquire);
    let capture = |p: &Probe| -> CapturedNativeWake {
        let reference = publish(p).unwrap();
        assert_eq!(reference.address(), installed);
        reference
    };
    let captured = (schedule == BETWEEN).then(|| capture(&probes[first]));
    probes[other].release_for("register_retain", ids[other].0);
    if schedule >= FOREIGN {
        let during_rejection = if schedule == FOREIGN_THEN_PUBLISH {
            probes[other].wait_for("register_cas", ids[other].0, 1);
            let reference = capture(&probes[first]);
            probes[other].release_for("register_cas", ids[other].0);
            Some(reference)
        } else {
            None
        };
        probes[other].wait_for("register_drop", ids[other].0, 1);
        assert_eq!(
            carrier.notification.load(Ordering::Acquire),
            if during_rejection.is_some() {
                1
            } else {
                installed
            }
        );
        print_timeline(&trace);
        probes[other].release_for("register_drop", ids[other].0);
        let _ = reactors[other].wait_completion(Duration::from_secs(10));
        std::process::exit(99);
    }
    probes[other].wait_for("registered", ids[other].0, 1);
    if let Some(reference) = captured {
        reference.notify();
        drop(reference);
    }
    for d in 0..2 {
        reactors[d].trace_progress().unwrap();
        assert_eq!(
            reactors[d]
                .wait_completion(Duration::from_secs(30))
                .unwrap()
                .operation,
            ids[d]
        );
        mailboxes[d].take_resolutions().unwrap();
        assert_eq!(
            mailboxes[d].settle(ids[d]).unwrap(),
            Resolution::Completed(Ok(()))
        );
        assert_eq!(
            receipts[d].native_transfer_result(),
            (version == 3).then_some(7183)
        );
    }
    drop(receipts);
    drop(reactors);
    print_timeline(&trace);
}

fn print_timeline(trace: &DomainTrace) {
    for row in trace.events() {
        println!("{row}");
    }
    std::io::stdout().flush().unwrap();
}

#[test]
fn rust_parallel_registrars_compete_with_sdk_publication() {
    if let Some(arguments) = std::env::var_os(CHILD) {
        let values: Vec<_> = arguments
            .to_str()
            .unwrap()
            .split(',')
            .map(|arg| arg.parse::<usize>().unwrap())
            .collect();
        rust_child(values.try_into().unwrap());
        return;
    }
    for case @ [version, first, schedule] in cases() {
        let output = process::bounded(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "parallel::rust_parallel_registrars_compete_with_sdk_publication",
                    "--nocapture",
                ])
                .env(CHILD, format!("{version},{first},{schedule}")),
        );
        assert_eq!(
            output.status.code(),
            Some(if schedule >= FOREIGN { 86 } else { 0 }),
            "{case:?} {output:?}"
        );
        check(std::str::from_utf8(&output.stdout).unwrap(), case);
        println!("PARALLEL REGISTRATION Rust {case:?}");
    }
}

#[test]
fn c_parallel_registrars_compete_with_sdk_publication() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let temp = Temporary(std::env::temp_dir().join(format!(
        "rinzu-parallel-registration-{}",
        std::process::id()
    )));
    std::fs::create_dir(&temp.0).unwrap();
    for sanitized in [false, true] {
        let binary = temp.0.join("parallel");
        let mut cc = native::clang();
        cc.args([
            "-std=c11", "-O1", "-g", "-pthread", "-Wall", "-Wextra", "-Werror",
        ])
        .arg("-I")
        .arg(root.join("include"))
        .arg(root.join("crates/runtime/tests/fixtures/parallel_registration.c"))
        .arg("-o")
        .arg(&binary);
        if sanitized {
            cc.args(["-fsanitize=address,undefined", "-fno-sanitize-recover=all"]);
        }
        let build = process::bounded_tool(&mut cc);
        assert!(build.status.success(), "{build:?}");
        for case @ [_, _, schedule] in cases() {
            let output = process::bounded(
                Command::new(&binary)
                    .args(case.map(|argument| argument.to_string()))
                    .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
                    .env("UBSAN_OPTIONS", "halt_on_error=1")
                    .env("RINZU_NATIVE_COMPLETION_NOTIFY", "1")
                    .env("RINZU_NATIVE_COMPLETION_DIRECT", "1"),
            );
            assert_eq!(
                output.status.code(),
                Some(if schedule >= FOREIGN { 70 } else { 0 }),
                "{case:?} {output:?}"
            );
            assert!(output.stderr.is_empty(), "{output:?}");
            check(std::str::from_utf8(&output.stdout).unwrap(), case);
            println!("PARALLEL REGISTRATION C sanitized={sanitized} {case:?}");
        }
    }
}
