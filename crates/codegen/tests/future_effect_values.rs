use rinzu_codegen::{SourceAsyncOptions, run_source_async_report};
use rinzu_compiler::{FileId, SourceFile, compile_source};

#[test]
fn map_key_snapshots_observe_the_right_values_and_keep_overwritten_future_aliases() {
    let artifact = compile_source(&SourceFile::new(
        FileId(262),
        "future-map-observation-execution.rinzu",
        include_str!("fixtures/future-map-observations.rinzu"),
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:#?}");
    assert_eq!(run.stdout, b"3\n0\n113\n1\n127\n");
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}

#[test]
fn parameter_completion_conditions_preserve_finite_results_and_resource_lifetimes() {
    let artifact = compile_source(&SourceFile::new(
        FileId(258),
        "future-pending-argument-execution.rinzu",
        include_str!("fixtures/future-pending-arguments.rinzu"),
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:#?}");
    assert_eq!(run.stdout, b"101\n103\n101\n101\n");
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}

#[test]
fn dynamic_list_observation_preserves_aliases_and_error_group_deduplication() {
    let artifact = compile_source(&SourceFile::new(
        FileId(254),
        "future-list-observation-execution.rinzu",
        include_str!("fixtures/future-list-observations.rinzu"),
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:#?}");
    assert_eq!(run.stdout, b"3\n0\n2\n");
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}

#[test]
fn future_observers_and_forwarded_aliases_preserve_failures_and_return_all_gc_roots() {
    let artifact = compile_source(&SourceFile::new(
        FileId(252),
        "future-observation-execution.rinzu",
        include_str!("fixtures/future-observations.rinzu"),
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:#?}");
    assert_eq!(run.stdout, b"71\n73\n79\n83\n89\n97\n7\n");
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}

#[test]
fn selected_callables_preserve_captured_values_and_error_identity() {
    let artifact = compile_source(&SourceFile::new(
        FileId(247),
        "callable-effect-execution.rinzu",
        include_str!("fixtures/callable-error-effects.rinzu"),
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:#?}");
    assert_eq!(run.stdout, b"41\n37\n47\n37\n43\n");
    assert_eq!(run.caught_errors.len(), 5);
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}

#[test]
fn future_parameters_forward_failures_and_success_values_without_losing_error_identity() {
    let text = include_str!("fixtures/future-parameter-errors.rinzu");
    let artifact = compile_source(&SourceFile::new(
        FileId(244),
        "future-parameter-execution.rinzu",
        text,
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:#?}");
    assert_eq!(run.stdout, b"17\n29\n31\n17\n");
    assert_eq!(run.caught_errors.len(), 4);
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}

#[test]
fn future_factories_and_immutable_aggregates_retain_the_original_failure_and_gc_roots() {
    let text = include_str!("fixtures/future-error-values.rinzu");
    let artifact = compile_source(&SourceFile::new(
        FileId(242),
        "future-value-execution.rinzu",
        text,
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:#?}");
    assert_eq!(run.stdout, b"11\n13\n17\n23\n29\n");
    assert_eq!(run.caught_errors.len(), 8);
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}
#[test]
fn growing_future_lists_keep_every_result_alive_until_observation() {
    let artifact = compile_source(&SourceFile::new(
        FileId(261),
        "future-growing-list.rinzu",
        include_str!("fixtures/future-growing-list.rinzu"),
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:#?}");
    assert_eq!(run.stdout, b"0\n1\n2\n0\n1\n1\n");
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}
#[test]
fn growing_future_maps_preserve_every_distinct_key_and_its_failure() {
    let artifact = compile_source(&SourceFile::new(
        FileId(263),
        "future-growing-map.rinzu",
        include_str!("fixtures/future-growing-map.rinzu"),
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:#?}");
    assert_eq!(run.stdout, b"0\n1\n2\n0\n1\n1\n");
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}
#[test]
fn mutable_objects_preserve_shared_identity_and_saved_future_results() {
    let artifact = compile_source(&SourceFile::new(
        FileId(265),
        "future-mutable-objects.rinzu",
        include_str!("fixtures/future-mutable-objects.rinzu"),
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:#?}");
    assert_eq!(run.stdout, b"131\n137\n149\n157\n137\n");
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}
#[test]
fn synchronous_field_calls_keep_alias_order_and_stores_before_errors() {
    let artifact = compile_source(&SourceFile::new(
        FileId(267),
        "future-field-calls.rinzu",
        include_str!("fixtures/future-field-calls.rinzu"),
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:#?}");
    assert_eq!(run.stdout, b"163\n167\n173\n181\n181\n");
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}
#[test]
fn field_observer_leases_preserve_failures_until_completion_and_return_gc_roots() {
    let artifact = compile_source(&SourceFile::new(
        FileId(269),
        "future-field-observers.rinzu",
        include_str!("fixtures/future-field-observers.rinzu"),
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:#?}");
    assert_eq!(run.stdout, b"193\n197\n199\n203\n204\n");
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}

#[test]
fn async_field_completion_retains_values_and_roots_after_success_and_error() {
    let artifact = compile_source(&SourceFile::new(
        FileId(270),
        "future-async-fields.rinzu",
        include_str!("fixtures/future-async-fields.rinzu"),
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:#?}");
    assert_eq!(run.stdout, b"211\n223\n227\n229\n229\n233\n239\n");
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}

#[test]
fn discarded_infallible_tasks_run_and_return_their_gc_roots() {
    let artifact = compile_source(&SourceFile::new(
        FileId(271),
        "future-discard.rinzu",
        include_str!("fixtures/future-discard.rinzu"),
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:#?}");
    assert_eq!(run.stdout, b"241\n251\n257\n263\n269\n");
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}

#[test]
fn future_captures_preserve_identity_results_and_gc_lifetimes() {
    let artifact = compile_source(&SourceFile::new(
        FileId(272),
        "future-capture.rinzu",
        include_str!("fixtures/future-capture.rinzu"),
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:#?}");
    assert_eq!(run.stdout, b"277\n281\n283\n293\n277\n");
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}

#[test]
fn observer_controls_preserve_failures_and_release_pending_timer_roots() {
    let artifact = compile_source(&SourceFile::new(
        FileId(273),
        "future-observer-controls.rinzu",
        include_str!("fixtures/future-observer-controls.rinzu"),
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:#?}");
    assert_eq!(run.stdout, b"307\n311\n307\n");
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}

#[test]
fn worker_local_errors_are_caught_before_scalar_transport() {
    let artifact = compile_source(&SourceFile::new(
        FileId(277),
        "error-worker-boundaries.rinzu",
        include_str!("fixtures/error-worker-boundaries.rinzu"),
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:#?}");
    assert_eq!(run.stdout, b"347\n702\n726\n");
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
    assert_eq!(run.worker_returns, 3);
    assert!(
        run.worker_heaps
            .iter()
            .all(|worker| worker.heap.objects == 0 && worker.heap.roots == 0)
    );
}

#[test]
fn complete_observer_collections_survive_synchronous_wrappers() {
    let artifact = compile_source(&SourceFile::new(
        FileId(276),
        "future-observer-collections.rinzu",
        include_str!("fixtures/future-observer-collections.rinzu"),
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:#?}");
    assert_eq!(run.stdout, b"331\n337\n343\n");
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}

#[test]
fn complete_value_receiver_families_survive_changes_to_field_receivers() {
    let artifact = compile_source(&SourceFile::new(
        FileId(275),
        "future-receiver-coverage.rinzu",
        include_str!("fixtures/future-receiver-coverage.rinzu"),
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:#?}");
    assert_eq!(run.stdout, b"317\n319\n");
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}
