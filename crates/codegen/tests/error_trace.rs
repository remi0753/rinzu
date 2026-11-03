use rinzu_codegen::{SourceAsyncOptions, run_source_async_report};
use rinzu_compiler::{FileId, SourceFile, compile_source};
use std::sync::Arc;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn scalar_inputs_and_constants_preserve_exact_failure_and_caller_spans() {
    let text = include_str!("fixtures/scalar-source-locations.rinzu");
    let artifact =
        compile_source(&SourceFile::new(FileId(320), "scalar-spans.rinzu", text)).unwrap();
    for enabled in [false, true] {
        let run = rinzu_codegen::run_baseline_jit_with_options(
            &artifact.ir,
            "main",
            rinzu_codegen::BaselineOptions {
                collect_every_allocation: true,
                capture_error_trace: enabled,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(run.result, Ok(0), "{run:#?}");
        assert_eq!(run.caught_errors.len(), 1);
        let failure = &run.caught_errors[0];
        let at = failure.primary.source.unwrap();
        let start = text.find("CInt8(value)").unwrap();
        assert_eq!(at.start_byte as usize, start);
        assert_eq!(at.end_byte as usize, start + "CInt8(value)".len());
        if enabled {
            let frames = &failure.traces[&0].frames;
            assert_eq!(frames.len(), 3);
            for (frame, (function, expression)) in frames.iter().zip([
                ("leaf", "CInt8(value)"),
                ("relay", "leaf(value, true, 1.5, ())"),
                ("main", "relay(300)"),
            ]) {
                assert_eq!(frame.function.as_ref(), function);
                let at = frame.source.unwrap();
                let start = text.find(expression).unwrap();
                assert_eq!(at.start_byte as usize, start);
                assert_eq!(at.end_byte as usize, start + expression.len());
            }
        } else {
            assert!(failure.traces.is_empty());
        }
        assert_eq!(run.heap_objects, 0);
        assert!(run.collection_count > 0);
    }
}

#[test]
fn caught_worker_operation_traces_reach_private_diagnostics_without_retaining_the_heap() {
    let text = r#"
fn leaf(){let invalid=CInt8(300);}
fn work()->Int{try{leaf();}catch errors{}try{leaf();}catch errors{}return 7;}
async fn main(){let pending=spawn work();try{let ignored=await pending;}catch errors{}}
"#;
    let artifact =
        compile_source(&SourceFile::new(FileId(233), "trace-worker.rinzu", text)).unwrap();
    for enabled in [false, true] {
        let run = run_source_async_report(
            &artifact.ir,
            "main",
            SourceAsyncOptions {
                capture_error_trace: enabled,
                gc_stress: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(run.failure_details.is_none(), "{run:#?}");
        assert_eq!(run.caught_errors.len(), 2);
        for failure in &run.caught_errors {
            if enabled {
                let trace = &failure.traces[&0];
                assert_eq!(
                    trace
                        .frames
                        .iter()
                        .map(|frame| frame.function.as_ref())
                        .collect::<Vec<_>>(),
                    ["leaf", "work"]
                );
                assert_eq!(trace.frames[0].source, failure.primary.source);
                assert!(
                    rinzu_codegen::format_error_traces(&artifact.ir, failure)
                        .contains("at leaf (trace-worker.rinzu:2:")
                );
            } else {
                assert!(failure.traces.is_empty());
            }
        }
        assert!(run.resources_returned);
        assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
        assert!(
            run.worker_heaps
                .iter()
                .all(|worker| worker.heap.objects == 0 && worker.heap.roots == 0)
        );
    }
}

#[test]
fn source_async_trace_keeps_the_failing_task_and_original_snapshot_after_repeated_await() {
    let text = r#"
error A{}
fn leaf(){throw A(message:"original");}
async fn work(){leaf();}
async fn main(){
    let pending=work();
    try{let firstValue=await pending;}catch[first:A]{}
    try{let secondValue=await pending;}catch[second:A]{}
}
"#;
    let artifact = compile_source(&SourceFile::new(FileId(231), "trace.rinzu", text)).unwrap();
    for enabled in [false, true] {
        let run = run_source_async_report(
            &artifact.ir,
            "main",
            SourceAsyncOptions {
                gc_stress: true,
                capture_error_trace: enabled,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(run.failure_details.is_none(), "{run:#?}");
        assert_eq!(run.caught_errors.len(), 2);
        if enabled {
            let first = &run.caught_errors[0].traces[&0];
            let second = &run.caught_errors[1].traces[&0];
            assert!(Arc::ptr_eq(first, second));
            assert_eq!(
                first
                    .frames
                    .iter()
                    .map(|frame| frame.function.as_ref())
                    .collect::<Vec<_>>(),
                ["leaf", "work"]
            );
            assert_eq!(first.frames[0].source, run.caught_errors[0].primary.source);
            assert_eq!(
                first.frames[1].source.unwrap().start_byte as usize,
                text.find("leaf();").unwrap()
            );
            let display = rinzu_codegen::format_error_traces(&artifact.ir, &run.caught_errors[0]);
            assert!(display.contains("at leaf (trace.rinzu:3:"), "{display}");
            assert!(display.contains("at work (trace.rinzu:4:"), "{display}");
            assert!(!display.contains("at main"), "{display}");
        } else {
            assert!(
                run.caught_errors
                    .iter()
                    .all(|failure| failure.traces.is_empty())
            );
            assert!(
                rinzu_codegen::format_error_traces(&artifact.ir, &run.caught_errors[0]).is_empty()
            );
        }
        assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn sync_jit_keeps_independent_cleanup_traces_after_error_payloads_are_reclaimed() {
    let text = r#"
error A{} error B{}
fn body(){throw A(message:"body");}
fn cleanup(){throw B(message:"cleanup");}
fn main(){try{try{body();}finally{cleanup();}}catch[A,B]{}}
"#;
    let artifact = compile_source(&SourceFile::new(FileId(232), "trace-jit.rinzu", text)).unwrap();
    for enabled in [false, true] {
        let run = rinzu_codegen::run_baseline_jit_with_options(
            &artifact.ir,
            "main",
            rinzu_codegen::BaselineOptions {
                collect_every_allocation: true,
                capture_error_trace: enabled,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(run.result, Ok(0), "{run:#?}");
        let failure = &run.caught_errors[0];
        if enabled {
            assert_eq!(
                failure.traces[&0]
                    .frames
                    .iter()
                    .map(|frame| frame.function.as_ref())
                    .collect::<Vec<_>>(),
                ["body", "main"]
            );
            assert_eq!(
                failure.traces[&1]
                    .frames
                    .iter()
                    .map(|frame| frame.function.as_ref())
                    .collect::<Vec<_>>(),
                ["cleanup", "main"]
            );
            assert!(!Arc::ptr_eq(&failure.traces[&0], &failure.traces[&1]));
            assert_eq!(
                failure.traces[&0].frames[1].source.unwrap().start_byte as usize,
                text.find("body();").unwrap()
            );
            assert_eq!(
                failure.traces[&1].frames[1].source.unwrap().start_byte as usize,
                text.find("cleanup();").unwrap()
            );
        } else {
            assert!(failure.traces.is_empty());
        }
        assert_eq!(run.heap_objects, 0);
    }
}
