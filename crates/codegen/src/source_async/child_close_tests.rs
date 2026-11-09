use super::*;
use crate::test_library::compile_library as compile_source;
use rinzu_compiler::{FileId, SourceFile};

#[test]
fn main_task_admission_rejects_before_prefix_and_physically_joins_admitted_children() {
    let file = FileId(381);
    let source = SourceFile::new(
        file,
        "main-limit.rinzu",
        r#"
import rinzu.stdio;
import rinzu.timer;
async fn pause(milliseconds: Int) -> Unit { await rinzu.timer.sleep(milliseconds); }

class Count {value:Int}
async fn child(count:Count) {count.value=count.value+1;await pause(2);}
async fn main() {let count=Count(value:0);try all {let first=child(count);let second=child(count);let third=child(count);let rejected=child(count);await rejected;await first;await second;await third;} finally {rinzu.stdio.writeLine(f"{count.value}");}}
"#,
    );
    let mut options = rinzu_compiler::CompilerOptions::default();
    options.sdk_files.insert(file);
    let artifact = crate::test_library::compile_library_with_options(&source, &options).unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            main_task_limit: Some(3),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.future_admissions.len(), 3);
    assert_eq!(run.stdout, b"0\n");
    assert_eq!(
        run.failure_details.as_ref().unwrap().primary.cause,
        FailureCause::Overloaded
    );
    assert!(run.resources_returned);
}

#[test]
fn saturated_mailbox_keeps_native_drain_and_cancellation_until_physical_completion() {
    use rinzu_runtime::native_completion::{CompletionPointer, NativeCompletion};
    use std::sync::atomic::{AtomicU32, Ordering};
    let module = Arc::new(
        compile_source(&SourceFile::new(
            FileId(1),
            "drain.rinzu",
            "async fn main(){} ",
        ))
        .unwrap()
        .ir,
    );
    let shared = Arc::new(Mutex::new(SharedState::default()));
    let owner = OwnerValues::new(&module).unwrap();
    let mut operations = SourceOperations::new(
        Arc::clone(&module),
        Arc::clone(&shared),
        &SourceAsyncOptions::default(),
        &owner,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        Arc::new(NativeBindings::default()),
    );
    operations.mailbox = OperationMailbox::new(
        rinzu_runtime::resources::ResourceLimits {
            pending_operations: 0,
            ..Default::default()
        },
        Arc::new(|| {}),
    );
    let Value::Future(rejected) = shared
        .lock()
        .unwrap()
        .future(
            0,
            PendingRequest::Timer {
                milliseconds: 1,
                source: None,
            },
        )
        .unwrap()
    else {
        unreachable!()
    };
    let rejection = operations.start(rejected).unwrap_err();
    assert_eq!(
        rejection
            .downcast_ref::<ManagedFailure>()
            .unwrap()
            .0
            .primary
            .cause,
        FailureCause::Overloaded
    );
    {
        let state = shared.lock().unwrap();
        assert!(
            state.future_scopes.is_empty()
                && state.future_admissions.is_empty()
                && state.pending.is_empty()
                && state.completed.is_empty()
        );
    }
    assert!(operations.mailbox.resources().used.is_baseline());
    let native = Box::new(NativeCompletion {
        version: 1,
        size: 16,
        done: AtomicU32::new(0),
        cancelled: AtomicU32::new(0),
    });
    // SAFETY: the Box stays pinned through completion and receipt below.
    let completion =
        unsafe { CompletionPointer::new((&*native as *const NativeCompletion) as usize) };
    let Value::Future(future) = shared
        .lock()
        .unwrap()
        .future(
            0,
            PendingRequest::NativeCompletion {
                completion,
                transfer: false,
                source: None,
            },
        )
        .unwrap()
    else {
        unreachable!()
    };
    operations.start(future).unwrap();
    assert!(operations.deferred_native.contains(&future));
    assert!(!shared.lock().unwrap().completed.contains_key(&future));
    assert!(operations.request_control(future, false));
    assert_eq!(native.cancelled.load(Ordering::Acquire), 1);
    assert!(!operations.receive_one_ready().unwrap());
    native.done.store(1, Ordering::Release);
    assert!(operations.receive_one_ready().unwrap());
    assert!(operations.deferred_native.is_empty());
    assert_eq!(
        shared.lock().unwrap().completed[&future]
            .as_ref()
            .unwrap_err()
            .primary
            .cause,
        FailureCause::Cancelled
    );
    assert!(operations.mailbox.resources().used.is_baseline());
    for cancelled in [false, true] {
        use rinzu_runtime::native_completion::{
            NotifiedNativeCompletion, TransferNativeCompletion,
        };
        let mut carrier = Box::new(TransferNativeCompletion::default());
        carrier.result = i64::MIN + 65536;
        // SAFETY: the Box remains pinned through the deferred physical wait.
        let completion = unsafe {
            CompletionPointer::new((&*carrier as *const TransferNativeCompletion) as usize)
        };
        let Value::Future(future) = shared
            .lock()
            .unwrap()
            .future(
                0,
                PendingRequest::NativeCompletion {
                    completion,
                    transfer: true,
                    source: None,
                },
            )
            .unwrap()
        else {
            unreachable!()
        };
        operations.start(future).unwrap();
        assert!(operations.deferred_native.contains(&future));
        if cancelled {
            assert!(operations.request_control(future, false));
        }
        assert!(!operations.receive_one_ready().unwrap());
        // SAFETY: valid uniquely published v3 prefix, no producer access follows.
        unsafe {
            NotifiedNativeCompletion::publish(&mut carrier.completion);
        }
        assert!(operations.receive_one_ready().unwrap());
        let completed = shared.lock().unwrap().completed[&future].clone();
        drop(carrier);
        if cancelled {
            assert_eq!(
                completed.unwrap_err().primary.cause,
                FailureCause::Cancelled
            );
        } else {
            assert_eq!(completed.unwrap(), Value::int(i64::MIN + 65536));
        }
        assert!(operations.mailbox.resources().used.is_baseline());
    }
    drop(operations);
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn source_child_join_cycle_fails_before_return_or_owner_disposal() {
    let output=std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact","source_async::operations::child_close_tests::source_prefix_starts_children_without_await_and_receives_each_actual_operation","--nocapture"])
            .env("RINZU_SOURCE_WAIT_CYCLE_PROBE","self").output().unwrap();
    assert_eq!(output.status.code(), Some(86), "{output:?}");
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert!(
        diagnostic.contains("observable-wait-cycle")
            && diagnostic.contains("symbol=main ")
            && diagnostic.contains("task=1 dependency=1")
            && !diagnostic.contains("source=0:"),
        "{diagnostic}"
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("test result: ok"));
}

#[test]
fn source_prefix_starts_children_without_await_and_receives_each_actual_operation() {
    let source = SourceFile::new(
        FileId(70),
        "eager-prefix.rinzu",
        r#"
fn echo(value: Int) -> Int { return value; }
async fn main() { let first = spawn echo(7); let second = spawn echo(8); }
"#,
    );
    let artifact = compile_source(&source).unwrap();
    let module = Arc::new(artifact.ir);
    let entry = function_index(&module, "main").unwrap();
    let shared = Arc::new(Mutex::new(SharedState::default()));
    let continuations = Rc::new(RefCell::new(BTreeMap::new()));
    let owner_values = OwnerValues::new(&module).unwrap();
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let source_bindings = Arc::new(NativeBindings::default());
    let operations = Rc::new(RefCell::new(SourceOperations::new(
        Arc::clone(&module),
        Arc::clone(&shared),
        &SourceAsyncOptions::default(),
        &owner_values,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        Arc::clone(&source_bindings),
    )));
    let admission = SpawnAdmissionController::new();
    let mut task = SourceTask {
        module: Arc::clone(&module),
        native_async: BTreeMap::new(),
        frame: Frame::new(&module, entry, &module.functions[entry], Vec::new())
            .with_owner(&owner_values),
        shared: Arc::clone(&shared),
        owner_thread: std::thread::current().id(),
        native_packages: None,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        source_native: Rc::new(NativeEngine::new(source_bindings).unwrap()),
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        supplants: Weak::new(),
        cancellation: CancellationState::default(),
        admission: admission.clone(),
        continuations: Rc::downgrade(&continuations),
        operations: Rc::downgrade(&operations),
        // Private state-corruption probe: the first child's identity is
        // deliberately aliased to the parent, exercising real join registration.
        scope: if std::env::var_os("RINZU_SOURCE_WAIT_CYCLE_PROBE").is_some() {
            1
        } else {
            0
        },
        completion: None,
        closing: None,
        closing_child: None,
        closing_group: None,
        task_groups: Vec::new(),
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        readiness: None,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        wait_dependency: None,
    };
    assert!(matches!(
        task.resume(TaskEvent::Start),
        TaskAction::Pending {
            cleanup_safe: true,
            ..
        }
    ));
    {
        let shared = shared.lock().unwrap();
        assert!(
            shared
                .events
                .iter()
                .all(|event| matches!(event, SourceAsyncEvent::Submitted { .. }))
        );
        assert_eq!(shared.events.len(), 2);
        assert!(shared.completed.is_empty() && shared.result.is_none());
    }
    // Both Worker bodies complete while the Main has registered no source
    // await. Each native packet and its admission stay held for its Owner.
    let mut services = operations.borrow_mut();
    for _ in 0..2 {
        let completion = services
            .workers
            .wait_completion(Duration::from_secs(2))
            .unwrap();
        services
            .worker_completions
            .insert(completion.operation, completion);
    }
    let snapshot = admission.snapshot(OriginDomainId(0));
    assert_eq!(snapshot.worker_domains, 2);
    assert_eq!(snapshot.in_flight_for_origin, 2);
    assert_eq!(snapshot.transport_bytes, 16);
    assert!(!services.mailbox.resources().used.is_baseline());
    // Receive in reverse creation order using the real completion IDs.
    services.receive(1).unwrap();
    assert!(shared.lock().unwrap().result.is_none());
    assert_eq!(
        shared
            .lock()
            .unwrap()
            .completed
            .get(&1)
            .unwrap()
            .as_ref()
            .unwrap()
            .as_i64()
            .unwrap(),
        8
    );
    services.receive(0).unwrap();
    assert_eq!(
        shared
            .lock()
            .unwrap()
            .completed
            .get(&0)
            .unwrap()
            .as_ref()
            .unwrap()
            .as_i64()
            .unwrap(),
        7
    );
    assert!(services.is_baseline());
    drop(services);
    assert!(matches!(
        task.resume(TaskEvent::Operation(Resolution::Completed(Ok(())))),
        TaskAction::Return
    ));
    assert!(shared.lock().unwrap().result.as_ref().unwrap().is_ok());
    assert_eq!(admission.snapshot(OriginDomainId(0)).transport_bytes, 0);
}

#[test]
fn sync_launchers_keep_child_scope_and_catch_chronology_across_pending_prefixes() {
    let text = r#"
fn echo() -> Int { return 7; }
async fn delayed() -> Int {
    try { let prefixFailure = CInt8(200); } catch prefix { }
    let value = await spawn echo();
    try { let resumedFailure = CInt8(300); } catch resumed { }
    return value;
}
fn launch() -> Int {
    try { let beforeFailure = CInt8(400); } catch before { }
    let child = delayed();
    try { let afterFailure = CInt8(500); } catch after { }
    let second = spawn echo();
    return 7;
}
async fn main() { let value = launch(); }
"#;
    let source = SourceFile::new(FileId(68), "sync-launcher.rinzu", text);
    let artifact = compile_source(&source).unwrap();
    let run = run_source_async_report(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
    assert!(run.failure_details.is_none(), "{run:?}");
    assert_eq!(
        run.caught_errors
            .iter()
            .map(|failure| failure.primary.source.unwrap().start_byte as usize)
            .collect::<Vec<_>>(),
        [400, 500, 200, 300].map(|value| text.find(&format!("CInt8({value})")).unwrap())
    );
    assert_eq!(
        run.events
            .iter()
            .filter(|event| matches!(event, SourceAsyncEvent::WorkerCompleted { .. }))
            .count(),
        2
    );
    assert!(run.resources_returned);
}

#[test]
fn pending_file_callees_resume_with_values_and_immutable_io_failures() {
    let source = SourceFile::new(
        FileId(69),
        "pending-file.rinzu",
        r#"
async fn read(path: String) -> Int { return await sdk.io.readSize(path); }
async fn main() {
    try all {
    let future = read("data.txt");
    let missing = read("missing.txt"); let alias = missing;
    let size = await future;
    if size != 5 { panic("wrong_size"); }
    try { let first = await missing; } catch [one: Error, ...oneRest] { let firstMessage = one.message; }
    try { let second = await alias; } catch [two: Error, ...twoRest] { let secondMessage = two.message; }
    } catch errors {}
}
"#,
    );
    let mut compiler = rinzu_compiler::CompilerOptions::default();
    compiler.native_functions.insert(
        "sdk.io.readSize".into(),
        rinzu_compiler::NativeFunction {
            parameters: vec![rinzu_compiler::Type::String],
            result: rinzu_compiler::Type::Future(Box::new(rinzu_compiler::Type::Int)),
            call_class: rinzu_compiler::NativeCallClass::NativeSafe,
            metadata_complete: true,
            cleanup_safe: false,
        },
    );
    let artifact = crate::test_library::compile_library_with_options(&source, &compiler).unwrap();
    let root = std::env::temp_dir().join(format!(
        "rinzu-pending-file-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("data.txt"), b"rinzu").unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            package_root: root.clone(),
            native_async: BTreeMap::from([(
                "sdk.io.readSize".into(),
                NativeAsyncOperation::FileReadSize { path_argument: 0 },
            )]),
            ..SourceAsyncOptions::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:?}");
    assert_eq!(run.caught_errors.len(), 2);
    assert_eq!(run.caught_errors[0], run.caught_errors[1]);
    assert_eq!(
        run.caught_errors[0].primary.cause,
        FailureCause::Error(ErrorKind::Io)
    );
    assert_eq!(run.awaited_scalars.len(), 2);
    assert!(run.awaited_scalars.iter().all(|value| value.bits() == 5));
    assert!(run.resources_returned);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pending_callees_keep_their_prefix_and_resume_each_dependency_before_parent_completion() {
    let source = SourceFile::new(
        FileId(65),
        "pending-callee.rinzu",
        r#"
fn echo(value: Int) -> Int { return value; }
async fn delayed(value: Int) -> Int {
    let first = await spawn echo(value);
    let second = await spawn echo(first + 1);
    return second;
}
async fn nested(value: Int) -> Int { return await delayed(value); }
async fn main() {
    var value = 7;
    let future = nested(value); value = 100;
    let copy = await future;
    if copy != 8 || value != 100 { panic("wrong_value"); }
}
"#,
    );
    let artifact = compile_source(&source).unwrap();
    let run = run_source_async_report(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
    assert!(run.failure_details.is_none(), "{run:?}");
    assert_eq!(
        run.awaited_scalars
            .iter()
            .map(|value| value.bits())
            .collect::<Vec<_>>(),
        [7, 8, 8, 8]
    );
    let flow = run
        .events
        .iter()
        .filter_map(|event| match event {
            SourceAsyncEvent::Suspended {
                future, operation, ..
            } => Some(format!("pending:{future}:{operation}")),
            SourceAsyncEvent::WorkerCompleted { future, .. } => Some(format!("worker:{future}")),
            SourceAsyncEvent::Resumed { future, .. } => Some(format!("ready:{future}")),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        flow,
        [
            "pending:0:async",
            "pending:1:async",
            "pending:2:spawn",
            "worker:2",
            "ready:2",
            "pending:3:spawn",
            "worker:3",
            "ready:3",
            "ready:1",
            "ready:0"
        ]
    );
    assert!(run.resources_returned);
}

#[test]
fn abandoned_failures_are_rejected_and_unstarted_children_cancel_without_cleanup() {
    let functions = r#"
fn echo() -> Int { return 7; }
fn forever() -> Int { while true { } return 0; }
async fn failed() -> Int {
    try { let first = await spawn echo(); let failure = CInt8(200); return 7; }
    finally { let cleanup = CInt8(300); }
}
async fn waiting() -> Int {
    try { let value = await spawn forever(); return value; }
    finally { try {let cleanup = CInt8(400);}catch errors{} }
}
async fn joining() -> Int { let child = waiting(); return 7; }
"#;
    for (body, cause, cleanup) in [
        (
            "let child = failed();",
            FailureCause::Error(ErrorKind::UnobservedChildFailure),
            2,
        ),
        (
            "let child = waiting(); let failure = 1 / 0;",
            FailureCause::ArithmeticTrap,
            0,
        ),
        (
            "let child = joining(); let failure = 1 / 0;",
            FailureCause::ArithmeticTrap,
            0,
        ),
        (
            "try { let child = joining(); let failure = 1 / 0; } finally { let outerCleanup = CInt8(500); }",
            FailureCause::ArithmeticTrap,
            1,
        ),
    ] {
        let source = SourceFile::new(
            FileId(66),
            "pending-close.rinzu",
            format!("{functions}\nasync fn main() {{ {body} }}"),
        );
        if body == "let child = failed();" {
            let errors = compile_source(&source).unwrap_err();
            assert!(
                errors
                    .iter()
                    .any(|error| error.code == rinzu_compiler::DiagnosticCode::FutureDiscarded),
                "{errors:#?}"
            );
            continue;
        }
        let artifact = compile_source(&source).unwrap();
        let run =
            run_source_async_report(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
        let failure = run.failure_details.as_ref().unwrap();
        assert_eq!(failure.primary.cause, cause, "{body}: {run:?}");
        assert_eq!(failure.cleanup.len(), cleanup, "{body}: {run:?}");
        assert!(run.resources_returned, "{body}: {run:?}");
        if cause == FailureCause::ArithmeticTrap {
            assert!(
                run.events
                    .iter()
                    .all(|event| !matches!(event, SourceAsyncEvent::Submitted { .. })),
                "a child cancelled before its first turn must not enter a Worker",
            );
        }
    }
}

#[test]
fn failed_pending_callee_is_observed_only_at_await_and_keeps_its_snapshot() {
    let source = SourceFile::new(
        FileId(67),
        "pending-observation.rinzu",
        r#"
fn echo() -> Int { return 7; }
async fn failed() -> CInt8 {
    try { let first = await spawn echo(); return CInt8(200); }
    finally { let cleanup = CInt8(300); }
}
async fn main() {
    var order = 0;
    try {
        let future = failed(); let alias = future; order = 1;
        try { let first = await future; } catch one { order = order * 10 + 2; }
        try { let second = await alias; } catch two { order = order * 10 + 3; }
    } catch wrong { order = 99; }
    if order != 123 { panic("wrong_order"); }
}
"#,
    );
    let artifact = compile_source(&source).unwrap();
    let run = run_source_async_report(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
    assert!(run.failure_details.is_none(), "{run:?}");
    assert_eq!(run.caught_errors.len(), 2);
    assert_eq!(run.caught_errors[0], run.caught_errors[1]);
    assert_eq!(
        run.caught_errors[0].primary.cause,
        FailureCause::Error(ErrorKind::Conversion)
    );
    assert_eq!(run.caught_errors[0].cleanup.len(), 1);
    assert!(run.resources_returned);
}

#[test]
fn spawn_reserves_at_creation_and_rejection_does_not_return_a_future() {
    let source = SourceFile::new(
        FileId(64),
        "spawn-admission.rinzu",
        r#"
fn echo(value: Int) -> Int { return value; }
async fn main() {
    var prefix = 0;
    try {
        let first = spawn echo(7);
        let denied = spawn echo(8);
        prefix = 1;
        let value = await denied;
    } catch wrong { prefix = 99; }
    finally {
        if prefix != 0 { panic("wrong_prefix"); }
        let cleanup = CInt8(300);
    }
}
"#,
    );
    let artifact = compile_source(&source).unwrap();
    let admission =
        SpawnAdmissionController::from_target_profile(rinzu_runtime::spawn::SpawnLimits {
            worker_domains: 1,
            ..rinzu_runtime::spawn::SpawnLimits::default()
        });
    let run = run_source_async_report_admitted(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            record_transport_evidence: true,
            ..SourceAsyncOptions::default()
        },
        admission,
    )
    .unwrap();
    assert_eq!(run.future_admissions.len(), 1);
    let failure = run.failure_details.unwrap();
    assert_eq!(failure.primary.cause, FailureCause::Overloaded);
    let denied_source = artifact
        .ir
        .functions
        .iter()
        .flat_map(|function| &function.blocks)
        .flat_map(|block| &block.operations)
        .filter(|operation| matches!(operation.kind, OperationKind::Spawn { .. }))
        .nth(1)
        .unwrap()
        .range;
    assert_eq!(
        failure.primary.source.unwrap().start_byte,
        denied_source.start_byte
    );
    assert_eq!(failure.cleanup.len(), 1);
    assert_eq!(
        failure.cleanup[0].cause,
        FailureCause::Error(ErrorKind::Conversion)
    );
    assert!(run.caught_errors.is_empty());
    assert!(
        run.events
            .iter()
            .any(|event| matches!(event, SourceAsyncEvent::Submitted { future: 0, .. }))
    );
    assert!(
        run.events
            .iter()
            .all(|event| !matches!(event, SourceAsyncEvent::Suspended { .. }))
    );
    assert_eq!(run.transport.peak_worker_domains, 1);
    assert_eq!(run.transport.peak_in_flight_per_origin, 1);
    // A started child may return Int or observe cancellation and return a
    // diagnostic packet. Verify the actual credit intervals for either path.
    use rinzu_runtime::transport::TransportDirection;
    let input = run
        .transport_evidence
        .iter()
        .filter(|credit| credit.direction == TransportDirection::Input)
        .collect::<Vec<_>>();
    assert_eq!(input.len(), 1);
    assert_eq!((input[0].operation, input[0].bytes), (Some(0), 8));
    let result = run
        .transport_evidence
        .iter()
        .filter(|credit| credit.direction == TransportDirection::Result)
        .collect::<Vec<_>>();
    assert!(result.len() <= 1);
    let cancelled_bytes = rinzu_runtime::transport::preflight(
        &failure_descriptor(),
        &failure_value(&Failure::new(FailureCause::Cancelled)),
    )
    .unwrap();
    for credit in result {
        assert_eq!(credit.operation, Some(0));
        assert!(
            credit.bytes == 8
                || (run.cancelled_children.contains(&0) && credit.bytes == cancelled_bytes)
        );
        assert!(input[0].released_at.unwrap() < credit.reserved_at);
    }
    let mut changes = run
        .transport_evidence
        .iter()
        .flat_map(|credit| {
            [
                (credit.reserved_at, credit.bytes as i64),
                (credit.released_at.unwrap(), -(credit.bytes as i64)),
            ]
        })
        .collect::<Vec<_>>();
    changes.sort_by_key(|(sequence, _)| *sequence);
    let mut used = 0i64;
    let mut peak = 0i64;
    for (_, change) in changes {
        used += change;
        assert!(used >= 0);
        peak = peak.max(used);
    }
    assert_eq!(used, 0);
    assert_eq!(run.transport.peak_transport_bytes, peak as usize);
    assert!(run.resources_returned);
}

#[test]
fn a_returned_future_keeps_its_failure_independent_of_the_completed_parent() {
    let text = r#"
class Pending {value:Future<CInt8>}
async fn bad()->CInt8 {try{return CInt8(200);}finally{let cleanup=CInt8(300);}}
fn launch()->Pending {return Pending(value:bad());}
async fn parent()->Pending {return launch();}
async fn main() {
    let holder=await parent();let alias=holder.value;
    try{let first=await holder.value;}catch errors{}
    try{let second=await alias;}catch errors{}
}
"#;
    let artifact =
        compile_source(&SourceFile::new(FileId(63), "returned-future.rinzu", text)).unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        run.failure_details.is_none() && run.resources_returned,
        "{run:#?}"
    );
    assert_eq!(run.caught_errors.len(), 2);
    assert_eq!(run.caught_errors[0], run.caught_errors[1]);
    assert_eq!(run.caught_errors[0].cleanup.len(), 1);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}

#[test]
fn unhandled_worker_normal_errors_are_rejected_before_runtime_admission() {
    let source = SourceFile::new(
        FileId(61),
        "unawaited-worker.rinzu",
        r#"
fn bad() -> CInt8 { try { return CInt8(200); } finally { let cleanup = CInt8(300); } }
async fn main() {
    let pending = spawn bad();
    try { let ignored = await pending; } catch errors {}
}
"#,
    );
    let errors = compile_source(&source).unwrap_err();
    assert!(
        errors.iter().any(|diagnostic| {
            diagnostic.code == rinzu_compiler::DiagnosticCode::ErrorEscapesBoundary
                && diagnostic.message.contains("Worker")
        }),
        "{errors:#?}"
    );
}

#[test]
fn parent_failure_cancels_running_child_without_replacing_its_primary() {
    let source = SourceFile::new(
        FileId(62),
        "close-failure.rinzu",
        r#"
fn forever() -> Int { while true { } return 0; }
async fn main() { let child = spawn forever(); try { let failure = 1 / 0; } finally { let cleanup = CInt8(300); } }
"#,
    );
    let artifact = compile_source(&source).unwrap();
    let run = run_source_async_report(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
    let failure = run.failure_details.unwrap();
    assert_eq!(failure.primary.cause, FailureCause::ArithmeticTrap);
    assert_eq!(failure.cleanup.len(), 1);
    assert_eq!(run.transport.peak_worker_domains, 1);
    assert!(run.resources_returned);
}
