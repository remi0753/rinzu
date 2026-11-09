use super::*;

#[test]
fn async_trace_preserves_task_future_and_native_order_across_interleavings() {
    use rinzu_trace::{EventKind, NativeTransition, TraceEvent};
    let expected = TraceDocument {
        schema: rinzu_trace::TRACE_SCHEMA.into(),
        version: rinzu_trace::TRACE_VERSION,
        producer: "test".into(),
        context: BTreeMap::new(),
        events: [
            (
                "main",
                EventKind::NativeEnter {
                    transition: NativeTransition::NativeSafe,
                },
            ),
            (
                "main:1",
                EventKind::AwaitPending {
                    operation: "file-read-size".into(),
                },
            ),
            (
                "main:1",
                EventKind::AwaitReady {
                    operation: "future:3".into(),
                },
            ),
            (
                "main",
                EventKind::NativeLeave {
                    transition: NativeTransition::NativeSafe,
                },
            ),
        ]
        .into_iter()
        .enumerate()
        .map(|(sequence, (task, event))| TraceEvent {
            sequence: sequence as u64,
            domain: "main".into(),
            task: Some(task.into()),
            source: None,
            event,
            observation: None,
        })
        .collect(),
    };
    let mut actual = expected.clone();
    actual.events.swap(2, 3);
    for (sequence, event) in actual.events.iter_mut().enumerate() {
        event.sequence = sequence as u64;
    }
    actual.validate().unwrap();
    compare_async_task_traces(&expected, &actual).unwrap();
    let mut already_ready = actual.clone();
    already_ready
        .events
        .retain(|event| !matches!(event.event, EventKind::AwaitPending { .. }));
    compare_async_task_traces(&expected, &already_ready).unwrap();
    let mut changed = actual.clone();
    changed.events[3].event = EventKind::AwaitReady {
        operation: "future:4".into(),
    };
    assert!(compare_async_task_traces(&expected, &changed).is_err());
    let mut changed = actual.clone();
    changed.events.swap(0, 2);
    assert!(compare_async_task_traces(&expected, &changed).is_err());
    let mut changed = actual.clone();
    changed.events[3].task = Some("main:2".into());
    assert!(compare_async_task_traces(&expected, &changed).is_err());
    let mut changed = actual.clone();
    changed.events[3].domain = "worker".into();
    assert!(compare_async_task_traces(&expected, &changed).is_err());
    actual.events.pop();
    assert!(compare_async_task_traces(&expected, &actual).is_err());
}

#[test]
fn await_evidence_allows_task_interleaving_but_keeps_each_task_future_and_order_exact() {
    let source = rinzu_compiler::SourceFile::new(
        rinzu_compiler::FileId(1),
        "tasks.rinzu",
        "async fn ready(value:CInt)->CInt{return value;} async fn child(value:CInt)->CInt{let result=await ready(value);return result;} async fn main(){let first=child(7);let second=child(9);let a=await first;let b=await second;}",
    );
    let artifact = rinzu_compiler::compile_source(&source).unwrap();
    let run = rinzu_codegen::run_source_async_report(
        &artifact.ir,
        "main",
        rinzu_codegen::SourceAsyncOptions::default(),
    )
    .unwrap();
    let report = |tasks: &BTreeMap<u64, Vec<(u64, rinzu_runtime::scalar::ScalarValue)>>| {
        let values = tasks
            .iter()
            .flat_map(|(scope, values)| {
                values.iter().map(move |(future, value)| {
                    (
                        *scope,
                        *future,
                        format!("{}:{:016x}", value.kind() as u32, value.bits()),
                    )
                })
            })
            .collect::<Vec<_>>();
        format!(
            "RINZU_AWAITED_VALUES={}\nRINZU_AWAITED_TASK_VALUES={}\n",
            values
                .iter()
                .map(|(_, _, value)| value.clone())
                .collect::<Vec<_>>()
                .join(","),
            values
                .iter()
                .map(|(scope, future, value)| format!("{scope}:{future}:{value}"))
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    let regrouped = report(&run.awaited_tasks);
    compare_awaited_values(&run, &regrouped).unwrap();
    let mut reordered = run.awaited_tasks.clone();
    reordered.get_mut(&0).unwrap().reverse();
    assert!(compare_awaited_values(&run, &report(&reordered)).is_err());
    let mut wrong_future = run.awaited_tasks.clone();
    wrong_future.get_mut(&0).unwrap()[0].0 += 1;
    assert!(compare_awaited_values(&run, &report(&wrong_future)).is_err());
    let mut omitted = run.awaited_tasks.clone();
    omitted.remove(&0);
    assert!(compare_awaited_values(&run, &report(&omitted)).is_err());
}

#[test]
fn file_close_race_keeps_explicit_observation_and_other_cancellation_exact() {
    use rinzu_codegen::SourceAsyncEvent;
    use rinzu_runtime::failure::FailureCause;
    let source = rinzu_compiler::SourceFile::new(
        rinzu_compiler::FileId(1),
        "file-close-evidence.rinzu",
        "async fn main() { panic(\"failure\"); }",
    );
    let artifact = rinzu_compiler::compile_source(&source).unwrap();
    let mut run = rinzu_codegen::run_source_async_report(
        &artifact.ir,
        "main",
        rinzu_codegen::SourceAsyncOptions::default(),
    )
    .unwrap();
    run.events.push(SourceAsyncEvent::Submitted {
        future: 0,
        operation: "file-read-size".into(),
    });
    run.scope_cancellation_requests.push(0);
    let completed = std::collections::BTreeSet::new();
    let cancelled = std::collections::BTreeSet::from([0]);
    compare_cancelled_children(&run, &cancelled).unwrap();
    run.cancelled_children.push(0);
    compare_cancelled_children(&run, &completed).unwrap();
    assert!(compare_cancelled_children(&run, &std::collections::BTreeSet::from([1])).is_err());

    for operation in ["spawn", "timer"] {
        let mut changed = run.clone();
        changed.events = vec![SourceAsyncEvent::Submitted {
            future: 0,
            operation: operation.into(),
        }];
        assert!(compare_cancelled_children(&changed, &completed).is_err());
    }
    let mut changed = run.clone();
    changed.scope_cancellation_requests.clear();
    assert!(compare_cancelled_children(&changed, &completed).is_err());
    let mut changed = run.clone();
    changed.control_attempts.push(0);
    assert!(compare_cancelled_children(&changed, &completed).is_err());
    let mut changed = run.clone();
    changed.events.push(SourceAsyncEvent::Awaited {
        scope: 0,
        future: 0,
    });
    assert!(compare_cancelled_children(&changed, &completed).is_err());
    let mut changed = run.clone();
    changed.events.push(SourceAsyncEvent::Suspended {
        scope: 0,
        future: 0,
        operation: "file-read-size".into(),
        owner_thread: std::thread::current().id(),
    });
    assert!(compare_cancelled_children(&changed, &completed).is_err());
    let mut changed = run.clone();
    changed.failure_details = None;
    assert!(compare_cancelled_children(&changed, &completed).is_err());
    let mut changed = run.clone();
    changed.failure_details.as_mut().unwrap().primary.cause = FailureCause::Cancelled;
    assert!(compare_cancelled_children(&changed, &completed).is_err());
}
#[test]
fn native_unit_race_requires_cancelled_ancestor_and_no_outside_observer() {
    use rinzu_codegen::{FutureAdmission, SourceAsyncEvent};
    let artifact = rinzu_compiler::compile_source(&rinzu_compiler::SourceFile::new(
        rinzu_compiler::FileId(1),
        "native-race.rinzu",
        "async fn main(){} ",
    ))
    .unwrap();
    let mut run =
        rinzu_codegen::run_source_async_report(&artifact.ir, "main", Default::default()).unwrap();
    run.future_admissions = vec![
        FutureAdmission {
            future: 10,
            parent_scope: 0,
            ordinal: 0,
        },
        FutureAdmission {
            future: 11,
            parent_scope: 11,
            ordinal: 0,
        },
    ];
    run.cancelled_children = vec![10];
    run.control_attempts = vec![10];
    run.scope_cancellation_requests = vec![11];
    run.events = vec![
        SourceAsyncEvent::Submitted {
            future: 11,
            operation: "native-completion".into(),
        },
        SourceAsyncEvent::Suspended {
            scope: 11,
            future: 11,
            operation: "native-completion".into(),
            owner_thread: std::thread::current().id(),
        },
    ];
    let actual = std::collections::BTreeSet::from([10, 11]);
    assert_eq!(
        compare_cancelled_children(&run, &actual).unwrap(),
        std::collections::BTreeSet::from([11])
    );
    let mut changed = run.clone();
    changed.events.push(SourceAsyncEvent::Awaited {
        scope: 0,
        future: 11,
    });
    assert!(compare_cancelled_children(&changed, &actual).is_err());
    changed = run.clone();
    changed.control_attempts.push(11);
    assert!(compare_cancelled_children(&changed, &actual).is_err());
    changed = run.clone();
    changed.scope_cancellation_requests.clear();
    assert!(compare_cancelled_children(&changed, &actual).is_err());
    changed = run.clone();
    changed.resources_returned = false;
    assert!(compare_cancelled_children(&changed, &actual).is_err());
    changed = run.clone();
    changed.future_admissions[1].parent_scope = 0;
    assert!(compare_cancelled_children(&changed, &actual).is_err());
    assert!(compare_cancelled_children(&run, &std::collections::BTreeSet::from([11])).is_err());
}

#[test]
fn admission_close_race_requires_unobserved_workers_and_real_scope_cancellation() {
    use rinzu_codegen::SourceAsyncEvent;
    use rinzu_runtime::failure::FailureCause;
    let source = rinzu_compiler::SourceFile::new(
        rinzu_compiler::FileId(1),
        "admission-close-evidence.rinzu",
        "async fn main() { panic(\"failure\"); }",
    );
    let artifact = rinzu_compiler::compile_source(&source).unwrap();
    let mut run = rinzu_codegen::run_source_async_report(
        &artifact.ir,
        "main",
        rinzu_codegen::SourceAsyncOptions::default(),
    )
    .unwrap();
    run.failure_details.as_mut().unwrap().primary.cause = FailureCause::Overloaded;
    run.events = vec![SourceAsyncEvent::Submitted {
        future: 0,
        operation: "spawn".into(),
    }];
    run.scope_cancellation_requests.push(0);
    let completed = std::collections::BTreeSet::new();
    let cancelled = std::collections::BTreeSet::from([0]);
    compare_cancelled_children(&run, &cancelled).unwrap();
    run.cancelled_children.push(0);
    compare_cancelled_children(&run, &completed).unwrap();
    assert!(compare_cancelled_children(&run, &std::collections::BTreeSet::from([1])).is_err());
    let mut received = run.clone();
    received.scope_cancellation_requests = vec![1];
    assert!(compare_cancelled_children(&received, &completed).is_err());
    received.events.push(SourceAsyncEvent::WorkerCompleted {
        future: 0,
        domain: 1,
        worker_thread: std::thread::current().id(),
    });
    compare_cancelled_children(&received, &completed).unwrap();
    for cause in [FailureCause::Cancelled, FailureCause::ArithmeticTrap] {
        let mut changed = run.clone();
        changed.failure_details.as_mut().unwrap().primary.cause = cause;
        assert!(compare_cancelled_children(&changed, &completed).is_err());
    }
    let mut changed = run.clone();
    changed.scope_cancellation_requests.clear();
    assert!(compare_cancelled_children(&changed, &completed).is_err());
    let mut changed = run.clone();
    changed.control_attempts.push(0);
    assert!(compare_cancelled_children(&changed, &completed).is_err());
    for event in [
        SourceAsyncEvent::Awaited {
            scope: 0,
            future: 0,
        },
        SourceAsyncEvent::Suspended {
            scope: 0,
            future: 0,
            operation: "spawn".into(),
            owner_thread: std::thread::current().id(),
        },
    ] {
        let mut changed = run.clone();
        changed.events.push(event);
        assert!(compare_cancelled_children(&changed, &completed).is_err());
    }
}

#[test]
fn implicit_close_races_validate_transport_and_heaps_in_both_directions() {
    use rinzu_codegen::SourceAsyncEvent;
    use rinzu_runtime::failure::{Failure, FailureCause};
    use rinzu_runtime::transport::TransportDirection;
    let source = rinzu_compiler::SourceFile::new(
        rinzu_compiler::FileId(1),
        "close-packet-evidence.rinzu",
        "fn echo(value:CUInt64)->CUInt64{return value;} async fn main(){let child=spawn echo(7);}",
    );
    let artifact = rinzu_compiler::compile_source(&source).unwrap();
    let mut run = rinzu_codegen::run_source_async_report(
        &artifact.ir,
        "main",
        rinzu_codegen::SourceAsyncOptions {
            gc_stress: true,
            record_transport_evidence: true,
            ..Default::default()
        },
    )
    .unwrap();
    run.failure_details = Some(Failure::new(FailureCause::Overloaded));
    run.scope_cancellation_requests.push(0);
    let completed = "RINZU_AWAITED_VALUES=\nRINZU_AWAITED_TASK_VALUES=\nRINZU_TRANSPORT=0,1,0,1,0,8\nRINZU_CREDITS=1:0:0:8:1:2;2:0:1:8:3:4\nRINZU_CANCELLED_CHILDREN=\nRINZU_STARTED_CHILDREN=1,0\nRINZU_STARTED_TIMERS=0\nRINZU_WORKER_HEAPS=0:0:0:2:0:2:1\n";
    let cancelled = completed
        .replace(";2:0:1:8:3:4", "")
        .replace(
            "RINZU_CANCELLED_CHILDREN=\n",
            "RINZU_CANCELLED_CHILDREN=0\n",
        )
        .replace("0:0:0:2:0:2:1", "0:0:0:1:0:1:0");
    compare_source_async_values_and_transport(&run, completed).unwrap();
    compare_source_async_values_and_transport(&run, &cancelled).unwrap();
    run.cancelled_children.push(0);
    run.events
        .retain(|event| !matches!(event, SourceAsyncEvent::WorkerCompleted { .. }));
    run.worker_heaps.clear();
    run.transport_evidence
        .retain(|credit| credit.direction == TransportDirection::Input);
    compare_source_async_values_and_transport(&run, completed).unwrap();
    compare_source_async_values_and_transport(&run, &cancelled).unwrap();
    run.failure_details = Some(Failure::new(FailureCause::Cancelled));
    assert!(compare_source_async_values_and_transport(&run, completed).is_err());
    run.root_cancellation_requested = true;
    compare_source_async_values_and_transport(&run, completed).unwrap();
    for corrupt in [
        completed.replace("1:0:0:8:1:2", "1:0:0:4:1:2"),
        completed.replace("1:0:0:8:1:2", "1:0:0:8:1:0"),
        completed.replace("0,1,0,1,0,8", "0,1,0,1,8,8"),
        completed.replace("0:0:0:2:0:2:1", "0:1:0:2:0:2:1"),
        completed.replace("RINZU_STARTED_CHILDREN=1,0", "RINZU_STARTED_CHILDREN=0,0"),
        completed.replace(
            "RINZU_CANCELLED_CHILDREN=\n",
            "RINZU_CANCELLED_CHILDREN=9\n",
        ),
    ] {
        assert!(
            compare_source_async_values_and_transport(&run, &corrupt).is_err(),
            "accepted corrupt evidence: {corrupt}"
        );
    }
    run.control_attempts.push(0);
    assert!(compare_source_async_values_and_transport(&run, completed).is_err());
    run.control_attempts.clear();
    run.events.push(SourceAsyncEvent::Suspended {
        scope: 0,
        future: 0,
        operation: "spawn".into(),
        owner_thread: std::thread::current().id(),
    });
    compare_source_async_values_and_transport(&run, completed).unwrap();
    run.events.push(SourceAsyncEvent::Awaited {
        scope: 0,
        future: 0,
    });
    assert!(compare_source_async_values_and_transport(&run, completed).is_err());
}

#[test]
fn async_equivalence_rejects_changed_type_bits_order_and_resource_evidence() {
    let source = rinzu_compiler::SourceFile::new(
        rinzu_compiler::FileId(1),
        "evidence.rinzu",
        "fn echo(value: CFloat) -> CFloat { return value; } async fn ready() -> CUInt64 { return 18446744073709551615; } async fn main() { let high = await ready(); let zero = await spawn echo(-0.0); }",
    );
    let artifact = rinzu_compiler::compile_source(&source).unwrap();
    let jit = rinzu_codegen::run_source_async(
        &artifact.ir,
        "main",
        rinzu_codegen::SourceAsyncOptions {
            gc_stress: true,
            record_transport_evidence: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(jit.result, 0);
    let correct = "RINZU_AWAITED_VALUES=7:ffffffffffffffff,8:0000000080000000\nRINZU_AWAITED_TASK_VALUES=0:0:7:ffffffffffffffff,0:1:8:0000000080000000\nRINZU_TRANSPORT=0,1,0,1,0,4\nRINZU_CREDITS=1:1:0:4:1:2;2:1:1:4:3:4\nRINZU_CANCELLED_CHILDREN=\nRINZU_STARTED_CHILDREN=1,0\nRINZU_STARTED_TIMERS=0\nRINZU_WORKER_HEAPS=1:0:0:2:0:2:1\n";
    compare_source_async_values_and_transport(&jit, correct).unwrap();
    for corrupt in [
        correct.replace(
            "RINZU_AWAITED_TASK_VALUES=0:0:7:ffffffffffffffff,0:1:8:0000000080000000\n",
            "",
        ),
        correct.replace("0:1:8:0000000080000000", "1:1:8:0000000080000000"),
        correct.replace("0:1:8:0000000080000000", "0:2:8:0000000080000000"),
        correct.replace("8:0000000080000000", "9:0000000080000000"),
        correct.replace("8:0000000080000000", "8:0000000000000000"),
        correct.replace(
            "7:ffffffffffffffff,8:0000000080000000",
            "8:0000000080000000,7:ffffffffffffffff",
        ),
        correct.replace("0,1,0,1,0,4", "0,1,0,1,4,4"),
        correct.replace("0,1,0,1,0,4", "0,1,0,1,0,8"),
        correct.replace("1:1:0:4:1:2", "1:1:0:4:1:0"),
        correct.replace("2:1:1:4:3:4", "1:1:1:4:3:4"),
        correct.replace("RINZU_STARTED_CHILDREN=1,0", "RINZU_STARTED_CHILDREN=0,0"),
        correct.replace("RINZU_STARTED_TIMERS=0\n", ""),
        correct.replace("RINZU_STARTED_TIMERS=0", "RINZU_STARTED_TIMERS=1"),
        correct.replace(
            "RINZU_WORKER_HEAPS=1:0:0:2:0:2:1",
            "RINZU_WORKER_HEAPS=1:1:0:2:0:2:1",
        ),
        correct.replace(
            "RINZU_WORKER_HEAPS=1:0:0:2:0:2:1",
            "RINZU_WORKER_HEAPS=1:0:1:2:0:2:1",
        ),
        correct.replace("RINZU_WORKER_HEAPS=1:0:0:2:0:2:1", "RINZU_WORKER_HEAPS="),
        correct.replace(
            "RINZU_WORKER_HEAPS=1:0:0:2:0:2:1",
            "RINZU_WORKER_HEAPS=1:0:0:2:0:1:1",
        ),
    ] {
        assert!(
            compare_source_async_values_and_transport(&jit, &corrupt).is_err(),
            "accepted corrupt evidence: {corrupt}"
        );
    }
    let mut jit = jit;
    jit.heap.userdata = rinzu_runtime::data::NativeUserDataEvidence {
        created: 2,
        released: 2,
        ..Default::default()
    };
    jit.worker_heaps[0].heap.userdata = rinzu_runtime::data::NativeUserDataEvidence {
        created: 1,
        released: 1,
        ..Default::default()
    };
    let userdata = format!(
        "{correct}RINZU_NATIVE_USERDATA=0,0,0,0,2,2\nRINZU_WORKER_USERDATA=1:0:0:0:0:1:1\nRINZU_USERDATA_PROCESS=0,0,0\n"
    );
    compare_source_async_values_and_transport(&jit, &userdata).unwrap();
    for corrupt in [
        userdata.replace("0,0,0,0,2,2", "1,0,0,0,2,2"),
        userdata.replace("0,0,0,0,2,2", "0,1,0,0,2,2"),
        userdata.replace("0,0,0,0,2,2", "0,0,1,0,2,2"),
        userdata.replace("0,0,0,0,2,2", "0,0,0,1,2,2"),
        userdata.replace("0,0,0,0,2,2", "0,0,0,0,2,1"),
        userdata.replace("0,0,0,0,2,2", "0,0,0,0,1,1"),
        userdata.replace("RINZU_NATIVE_USERDATA=0,0,0,0,2,2\n", ""),
        userdata.replace("1:0:0:0:0:1:1", "1:0:1:0:0:1:1"),
        userdata.replace("1:0:0:0:0:1:1", "1:0:0:0:0:1:0"),
        userdata.replace("1:0:0:0:0:1:1", ""),
        userdata.replace("1:0:0:0:0:1:1", "2:0:0:0:0:1:1"),
        userdata.replace("1:0:0:0:0:1:1", "1:0:0:0:0:1:1;1:0:0:0:0:1:1"),
        userdata.replace("RINZU_USERDATA_PROCESS=0,0,0\n", ""),
        userdata.replace(
            "RINZU_USERDATA_PROCESS=0,0,0",
            "RINZU_USERDATA_PROCESS=4,4,160",
        ),
        userdata.replace(
            "RINZU_USERDATA_PROCESS=0,0,0",
            "RINZU_USERDATA_PROCESS=3,2,120",
        ),
        userdata.replace(
            "RINZU_USERDATA_PROCESS=0,0,0",
            "RINZU_USERDATA_PROCESS=3,3,96",
        ),
    ] {
        assert!(
            compare_source_async_values_and_transport(&jit, &corrupt).is_err(),
            "accepted corrupt userdata: {corrupt}"
        );
    }
}

#[test]
fn callback_resource_equivalence_keeps_retention_and_temporary_baseline_separate() {
    let correct = "RINZU_NATIVE_ENTRIES=780,2,65536,157568,0,0\nRINZU_CALLBACK_PROCESS=780,124800,48\nRINZU_CALLBACK_TEMPORARY=0,0\n";
    validate_callback_resource_evidence(correct).unwrap();
    for broken in [
        correct.replace("780,124800,48", "779,124800,48"),
        correct.replace("780,124800,48", "780,124799,48"),
        correct.replace("780,124800,48", "780,124800,0"),
        correct.replace("0,0\n", "1,0\n"),
        correct.replace(
            "RINZU_CALLBACK_TEMPORARY=0,0",
            "RINZU_CALLBACK_TEMPORARY=0,64",
        ),
        correct.replace("RINZU_CALLBACK_TEMPORARY=0,0\n", ""),
        correct.replace("780,2,65536", "780,1,65536"),
    ] {
        assert!(
            validate_callback_resource_evidence(&broken).is_err(),
            "{broken}"
        );
    }
}

#[test]
fn callback_resource_equivalence_accepts_retired_and_mixed_controls() {
    for (control, tombstone, retired, slots, released) in [
        (32768, 0, 780, 24960, 124800),
        (93568, 60800, 400, 12800, 64000),
    ] {
        let correct = format!(
            "RINZU_NATIVE_ENTRIES=780,2,65536,{control},0,0\nRINZU_CALLBACK_PROCESS=780,{tombstone},48\nRINZU_CALLBACK_TEMPORARY=0,0\nRINZU_CALLBACK_RETIREMENT={retired},{slots},{released}\n"
        );
        validate_callback_resource_evidence(&correct).unwrap();
        for broken in [
            correct.replace(&format!("{retired},{slots},{released}"), "781,24992,124960"),
            correct.replace(
                &format!("{retired},{slots},{released}"),
                &format!("{retired},1,{released}"),
            ),
            correct.replace(
                &format!("{retired},{slots},{released}"),
                &format!("{retired},{slots},0"),
            ),
            correct.replace(
                &format!("RINZU_CALLBACK_PROCESS=780,{tombstone},48"),
                "RINZU_CALLBACK_PROCESS=780,1,48",
            ),
        ] {
            assert!(
                validate_callback_resource_evidence(&broken).is_err(),
                "{broken}"
            );
        }
    }
}

#[test]
fn native_equivalence_rejects_missing_return_copy_and_live_resource_evidence() {
    let evidence = rinzu_codegen::SourceNativeCallEvidence {
        prepared: 5,
        returned: 5,
        closed: 5,
        settled: 5,
        read: 5,
        released: 5,
        callback_entries: 96,
        copied_bytes: 416,
        ..Default::default()
    };
    let correct = "RINZU_NATIVE_CALLS=5,5,5,5,5,5,96,416,0,0,0\nRINZU_NATIVE_ABANDONED=0\nRINZU_CALLBACK_RETURNS=0,0,0,0\nRINZU_NATIVE_ENTRIES=0,0,0,0,0,0\nRINZU_CALLBACK_PROCESS=0,0,0\nRINZU_CALLBACK_TEMPORARY=0,0\n";
    compare_native_call_evidence(evidence, correct).unwrap();
    for broken in [
        "RINZU_NATIVE_CALLS=5,4,5,5,5,5,96,416,0,0,0\n",
        "RINZU_NATIVE_CALLS=5,5,5,5,5,5,95,416,0,0,0\n",
        "RINZU_NATIVE_CALLS=5,5,5,5,5,5,96,415,0,0,0\n",
        "RINZU_NATIVE_CALLS=5,5,5,5,5,5,96,416,0,1,0\n",
        "",
    ] {
        assert!(compare_native_call_evidence(evidence, broken).is_err());
    }
    for broken in ["", "1,0,0,24", "0,1,1,24", "0,0,1,0", "0,0,0,24"] {
        assert!(
            compare_native_call_evidence(
                evidence,
                &correct.replace("0,0,0,0\n", &format!("{broken}\n"))
            )
            .is_err()
        );
    }
}

#[test]
fn native_equivalence_compares_abandonment_without_synthesizing_return() {
    let evidence = rinzu_codegen::SourceNativeCallEvidence {
        prepared: 3,
        returned: 2,
        closed: 2,
        settled: 2,
        read: 2,
        released: 2,
        abandoned: 1,
        ..Default::default()
    };
    let correct = "RINZU_NATIVE_CALLS=3,2,2,2,2,2,0,0,0,0,0\nRINZU_NATIVE_ABANDONED=1\nRINZU_CALLBACK_RETURNS=0,0,0,0\nRINZU_NATIVE_ENTRIES=0,0,0,0,0,0\nRINZU_CALLBACK_PROCESS=0,0,0\nRINZU_CALLBACK_TEMPORARY=0,0\n";
    compare_native_call_evidence(evidence, correct).unwrap();
    for broken in [
        correct.replace("RINZU_NATIVE_ABANDONED=1", "RINZU_NATIVE_ABANDONED=0"),
        correct.replace("RINZU_NATIVE_ABANDONED=1\n", ""),
        correct.replace("3,2,2,2,2,2", "3,3,3,3,3,3"),
        correct.replace("RINZU_NATIVE_ABANDONED=1", "RINZU_NATIVE_ABANDONED=1,1"),
    ] {
        assert!(compare_native_call_evidence(evidence, &broken).is_err());
    }
}

#[test]
fn native_equivalence_rejects_unreleased_scopes_and_false_promotion() {
    let evidence = rinzu_codegen::SourceNativeCallEvidence {
        prepared: 1,
        returned: 1,
        closed: 1,
        settled: 1,
        read: 1,
        released: 1,
        callback_returns: rinzu_codegen::CallbackReturnEvidence {
            normal: 1,
            fallback: 1,
            bytes: 8,
            metadata_bytes: 48,
        },
        ..Default::default()
    };
    let correct = "RINZU_NATIVE_CALLS=1,1,1,1,1,1,0,0,0,0,0\nRINZU_NATIVE_ABANDONED=0\nRINZU_CALLBACK_ALLOCATIONS=1,1,8,48\nRINZU_CALLBACK_RETURNS=0,0,0,0\nRINZU_CALLBACK_SCOPES=2,2,0,8,8,0,0,8,6,0,0\nRINZU_NATIVE_ENTRIES=0,0,0,0,0,0\nRINZU_CALLBACK_PROCESS=0,0,0\nRINZU_CALLBACK_TEMPORARY=0,0\n";
    compare_native_call_evidence(evidence, correct).unwrap();
    for replacement in [
        "2,1,0,8,8,0,0,8,6,0,0",
        "2,2,0,8,7,0,0,8,6,0,0",
        "2,2,0,8,8,0,1,8,6,0,0",
        "2,2,0,8,8,0,0,8,6,1,0",
        "2,2,0,8,8,0,0,8,6,0,24",
        "2,1,1,8,4,4,0,8,6,0,0",
        "2",
    ] {
        assert!(
            compare_native_call_evidence(
                evidence,
                &correct.replace("2,2,0,8,8,0,0,8,6,0,0", replacement)
            )
            .is_err()
        );
    }
}

#[test]
fn native_equivalence_requires_all_foreign_receipts_retirement_and_wake_baseline() {
    let evidence = rinzu_codegen::SourceNativeCallEvidence {
        prepared: 1,
        returned: 1,
        closed: 1,
        settled: 1,
        read: 1,
        released: 1,
        foreign: rinzu_codegen::ForeignInvocationEvidence {
            created: 8,
            published: 8,
            received: 8,
            retired: 8,
            ..Default::default()
        },
        ..Default::default()
    };
    let correct = "RINZU_NATIVE_CALLS=1,1,1,1,1,1,0,0,0,0,0\nRINZU_NATIVE_ABANDONED=0\nRINZU_CALLBACK_RETURNS=0,0,0,0\nRINZU_FOREIGN=8,8,8,8,0,0,0,0\nRINZU_NATIVE_ENTRIES=0,0,0,0,0,0\nRINZU_CALLBACK_PROCESS=0,0,0\nRINZU_CALLBACK_TEMPORARY=0,0\n";
    compare_native_call_evidence(evidence, correct).unwrap();
    for corrupt in [
        "",
        "8,8,7,8,0,0,0,0",
        "8,8,8,7,0,0,0,0",
        "8,8,8,8,1,0,0,0",
        "8,8,8,8,0,1,0,0",
        "8,8,8,8,0,0,1,0",
        "8,8,8,8,0,0,0,1",
        "8,8,8,8,0,0,0,0,0",
    ] {
        assert!(
            compare_native_call_evidence(evidence, &correct.replace("8,8,8,8,0,0,0,0", corrupt))
                .is_err(),
            "accepted {corrupt}"
        );
    }
}

#[test]
fn concurrent_transport_evidence_proves_actual_peaks_and_packet_widths() {
    let early_inputs =
        parse_transport_credits("1:0:0:14:1:3;2:1:0:14:2:4;3:0:1:4:5:7;4:1:1:4:6:8").unwrap();
    let early_result =
        parse_transport_credits("1:0:0:14:1:2;2:0:1:4:3:7;3:1:0:14:4:5;4:1:1:4:6:8").unwrap();
    assert_eq!(
        validate_transport_credits(&early_inputs, 28).unwrap(),
        validate_transport_credits(&early_result, 18).unwrap()
    );
    for (credits, peak) in [(&early_inputs, 18), (&early_result, 28)] {
        assert!(validate_transport_credits(credits, peak).is_err());
    }
    for invalid in [
        "1:0:0:14:1:4;2:0:1:4:2:3", // result before input release
        "1:0:0:14:1:2;2:0:1:4:3:3", // duplicate sequence
        "1:0:0:14:1:0",             // missing release
        "1:0:1:4:1:2",              // no admitted input
    ] {
        assert!(
            validate_transport_credits(&parse_transport_credits(invalid).unwrap(), 18).is_err()
        );
    }
}
