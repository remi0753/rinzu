#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "../test-support/library.rs"]
mod test_library;
use rinzu_codegen::{
    BaselineOptions, SourceAsyncOptions, run_baseline_jit_with_options, run_source_async_report,
};
use rinzu_compiler::{FileId, SourceFile};
use test_library::compile_library;

#[test]
fn specialized_scalar_and_reference_graphs_survive_every_allocation_collection() {
    let artifact = compile_library(&SourceFile::new(
        FileId(1),
        "generic-gc.rinzu",
        r#"
import rinzu.stdio;
class Box<T> { const value: T }
enum Choice<T> { Empty, Value(value: T) }
fn identity<T>(value: T) -> T { let garbage = ["allocate", "collect"]; return value; }
fn main() {
    let a = Box<Int>(value: identity<Int>(42));
    let b = Box<String>(value: identity<String>("日本語\u{0}tail"));
    let c = Box<Box<String>>(value: b);
    let e = Choice<Box<String>>.Value(value: c.value);
    var index = 0;
    while index < 64 { let next = Box<String>(value: "temporary"); index = index + 1; }
    rinzu.stdio.writeLine(a.value);
    switch e {
        case Choice.Value(payload): { rinzu.stdio.writeLine(payload.value); }
        case Choice.Empty: {}
    }
}
"#,
    ))
    .unwrap();
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
    assert_eq!(run.stdout, "42\n日本語\0tail\n".as_bytes());
    assert!(run.collection_count > 64);
    assert_eq!(run.heap_objects, 0);
    assert_eq!(run.published_code_versions, run.retired_code_versions);
}

#[test]
fn specialized_worker_arguments_and_results_settle_both_domains() {
    let artifact = compile_library(&SourceFile::new(
        FileId(2),
        "generic-worker.rinzu",
        include_str!("../../../tests/fixtures/programs/generics/src/main.rinzu"),
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        run.stdout,
        "42\n日本語\0tail\n42\nchoice\nconversion caught\ntrue\ntrue\n".as_bytes()
    );
    assert_eq!(run.heap.objects, 0);
    assert_eq!(run.worker_heaps.len(), 3);
    assert!(
        run.worker_heaps
            .iter()
            .all(|w| w.heap.objects == 0 && w.heap.roots == 0)
    );
}

#[test]
fn user_defined_result_payloads_survive_collection_and_transfer() {
    let artifact = compile_library(&SourceFile::new(
        FileId(3),
        "result-gc.rinzu",
        r#"
import rinzu.stdio;
enum Result<T,E>{Ok(value:T),Err(error:E)}
fn main() {
    let nested = Option<Result<String, Int>>.Some(value: Result<String, Int>.Ok(value: "日本語\u{0}tail"));
    var index = 0;
    while index < 64 { let temporary = Option<String>.Some(value: "collect"); index = index + 1; }
    switch nested { case Option.Some(value): {
        switch value { case Result.Ok(payload): { rinzu.stdio.writeLine(payload); } case Result.Err(payload): {} }
    } case Option.None: {} }
}

"#,
    )).unwrap();
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
    assert_eq!(run.stdout, "日本語\0tail\n".as_bytes());
    assert!(run.collection_count > 64);
    assert_eq!(run.heap_objects, 0);

    let artifact = compile_library(&SourceFile::new(
        FileId(4),
        "result-worker.rinzu",
        include_str!("../../../tests/fixtures/programs/results/src/main.rinzu"),
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.worker_heaps.len(), 1);
    assert!(
        run.worker_heaps
            .iter()
            .all(|w| w.heap.objects == 0 && w.heap.roots == 0)
    );
    assert_eq!(run.heap.objects, 0);
}

#[test]
fn immutable_collection_operations_preserve_roots_and_snapshots() {
    let source = r#"import rinzu.List;
import rinzu.Map;

import rinzu.stdio;
fn main() {
    let original=["日本語\u{0}tail"];
    let changed=rinzu.List.append(original,"second");
    var index=0;
    while index<64 { let temporary=rinzu.List.append(changed,"collect"); index=index+1; }
    switch rinzu.List.get(original,0) { case Option.Some(value): {rinzu.stdio.writeLine(value);} case Option.None:{} }
    switch rinzu.List.set(changed,1,"updated") {case Option.Some(value):{rinzu.stdio.writeLine(value);} case Option.None:{} }
    switch rinzu.List.remove(changed,0) {case Option.Some(value):{rinzu.stdio.writeLine(value);} case Option.None:{} }
    let table: Map<String,List<String>> = {"a":original,"b":changed};
    let updated=rinzu.Map.remove(rinzu.Map.insert(table,"a",changed),"b");
    rinzu.stdio.writeLine(rinzu.Map.keys(updated));
    rinzu.stdio.writeLine(rinzu.Map.length(updated));
    switch rinzu.Map.get(table,"a") {case Option.Some(value):{rinzu.stdio.writeLine(value==original);} case Option.None:{} }
}
"#;
    let artifact =
        compile_library(&SourceFile::new(FileId(5), "collection-gc.rinzu", source)).unwrap();
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
        "日本語\0tail\n[\"日本語\\u{00}tail\", \"updated\"]\n[\"second\"]\n[\"a\"]\n1\ntrue\n"
            .as_bytes()
    );
    assert!(run.collection_count > 64);
    assert_eq!(run.heap_objects, 0);
    let artifact = compile_library(&SourceFile::new(
        FileId(6),
        "collection-worker.rinzu",
        include_str!("../../../tests/fixtures/programs/collections/src/main.rinzu"),
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.heap.objects, 0);
    assert_eq!(run.worker_heaps.len(), 1);
    assert!(
        run.worker_heaps
            .iter()
            .all(|w| w.heap.objects == 0 && w.heap.roots == 0)
    );
}
