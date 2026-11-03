#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "../test-support/library.rs"]
mod test_library;
use test_library::compile_library;
#[path = "../../compiler/test-support/source_edit.rs"]
mod source_edit;
use source_edit::ReplaceChecked;

use rinzu_codegen::{
    BaselineOptions, SourceAsyncOptions, run_baseline_jit_with_options, run_source_async_report,
};
use rinzu_compiler::{FileId, SourceFile};
#[test]
fn converted_capture_fields_cycles_and_pending_calls_survive_gc() {
    let text = include_str!("fixtures/m2_closures.rinzu");
    let artifact = compile_library(&SourceFile::new(FileId(1), "closures.rinzu", text)).unwrap();
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
    assert_eq!(run.stdout, b"42\nescaped:7\ngeneric\n43\n");
    assert!(run.heap.collections > 600 && run.heap.moved_objects > 600);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
    assert!(
        run.worker_heaps
            .iter()
            .all(|w| w.heap.objects == 0 && w.heap.roots == 0)
    );
    let text = text
        .replace_checked("async fn later", "fn later")
        .replace_checked("await spawn identity(value)", "identity(value)")
        .replace_checked("async fn main()", "fn main()")
        .replace_checked("await pending", "pending")
        .replace_checked("-> Future<Int>", "-> Int");
    let artifact =
        compile_library(&SourceFile::new(FileId(2), "closures-sync.rinzu", text)).unwrap();
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
    assert_eq!(run.stdout, b"42\nescaped:7\ngeneric\n43\n");
    assert_eq!(run.heap_objects, 0);
    assert!(run.collection_count > 600);
}

#[test]
fn repeated_closure_state_cycles_are_reclaimed_between_scopes() {
    let text = r#"import rinzu.stdio;
    class Editor {value:Int,command:Option<fn(Int)->Int>}
    fn edit(editor:Editor,value:Int)->Int {editor.value=value;return editor.value;}
    fn make()->fn(Int)->Int {let editor=Editor(value:0,command:Option<fn(Int)->Int>.None());let function=fn(argument0:Int)->Int {return edit(editor, argument0);};editor.command=Option<fn(Int)->Int>.Some(value:function);return function;}
    async fn main() {var i=0;var total=0;while i<100 {let function=make();total=total+function(i);var j=0;while j<3 {let temporary=f"gc-{j}";j=j+1;}i=i+1;}rinzu.stdio.writeLine(total);}"#;
    let artifact =
        compile_library(&SourceFile::new(FileId(1), "cycle-scopes.rinzu", text)).unwrap();
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
    assert_eq!(run.stdout, b"4950\n");
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
    assert!(run.heap.collections > 300);
    assert!(run.heap.peak_objects < 100, "{run:?}");
}
