#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "../../compiler/test-support/source_edit.rs"]
mod source_edit;
use rinzu_codegen::{
    BaselineOptions, SourceAsyncOptions, run_baseline_jit_with_options, run_source_async_report,
};
use rinzu_compiler::{FileId, SourceFile, compile_source};
use source_edit::ReplaceChecked;

#[test]
fn local_callable_shadowing_a_generic_template_dispatches_to_the_lambda() {
    let artifact = compile_source(&SourceFile::new(
        FileId(1),
        "shadowing.rinzu",
        r#"
        fn identity<T>(value: T) -> T { return value; }
        fn main() {
            let identity = fn(value: Int) -> Int { return value + 1; };
            let alias = identity;
            let result = alias(41);
            if result != 42 { panic("wrongTarget"); }
        }
    "#,
    ))
    .unwrap();
    let run = run_baseline_jit_with_options(
        &artifact.ir,
        "main",
        BaselineOptions {
            collect_every_allocation: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.result, Ok(0));
    assert_eq!(run.heap_objects, 0);
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:#?}");
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}

#[test]
fn lambda_cycles_pending_async_and_value_snapshots_survive_gc() {
    let text = include_str!("../../../tests/fixtures/programs/lambdas/src/main.rinzu");
    let artifact = compile_source(&SourceFile::new(FileId(1), "lambdas.rinzu", text)).unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            gc_stress: true,
            capture_stdout: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:?}");
    assert_eq!(
        run.stdout,
        b"42\nescaped:7\ncreated\ncalled\nstarted\n43\n42\n"
    );
    assert!(run.heap.collections > 600);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
    assert!(
        run.worker_heaps
            .iter()
            .all(|w| w.heap.objects == 0 && w.heap.roots == 0)
    );
    let text = text
        .replace("async fn main()", "fn main()")
        .replace("async fn (value: Int) -> Int", "fn (value: Int) -> Int")
        .replace("await spawn identity(value)", "identity(value)")
        .replace_checked("later: fn (Int) -> Future<Int>", "later: fn (Int) -> Int")
        .replace("await pending", "pending");
    let artifact = compile_source(&SourceFile::new(FileId(2), "lambdas-gc.rinzu", text)).unwrap();
    let run = run_baseline_jit_with_options(
        &artifact.ir,
        "main",
        BaselineOptions {
            capture_stdout: true,
            collect_every_allocation: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.result, Ok(0));
    assert_eq!(
        run.stdout,
        b"42\nescaped:7\ncreated\nstarted\ncalled\n43\n42\n"
    );
    assert!(run.collection_count > 600);
    assert_eq!(run.heap_objects, 0);
}
