#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "../test-support/library.rs"]
mod test_library;
use rinzu_codegen::{SourceAsyncOptions, run_source_async_report};
use rinzu_compiler::{FileId, SourceFile};
use test_library::compile_library;

#[test]
fn cancellation_during_collection_updates_bypasses_catch_and_releases_data() {
    let source = r#"
import rinzu.stdio;
async fn main() {
    try {
        var values: List<Int> = [];
        var index = 0;
        while index < 100 { values = listAppend(values,index); index=index+1; }
    } catch error { rinzu.stdio.writeLine("caught"); }
    finally { rinzu.stdio.writeLine("cleanup"); }
}
"#;
    let artifact = compile_library(&SourceFile::new(
        FileId(1),
        "collection-cancel.rinzu",
        source,
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            cancel_after_polls: Some(12),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.stdout, b"cleanup\n");
    assert_eq!(run.heap.objects, 0);
    assert_eq!(run.heap.roots, 0);
    assert!(run.caught_errors.is_empty());
}
