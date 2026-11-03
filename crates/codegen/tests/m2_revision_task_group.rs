#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "../test-support/library.rs"]
mod test_library;
use rinzu_codegen::{SourceAsyncOptions, run_source_async_report};
use rinzu_compiler::{FileId, OperationKind, SourceFile};
use test_library::compile_library;

#[test]
fn collection_continues_to_an_explicit_sibling_await_after_error() {
    let source = r#"
import rinzu.stdio;
fn work()->Int {var n=0;while n<10000{n=n+1;}return 42;}
async fn child() {let result=await spawn work();rinzu.stdio.writeLine(result);}
async fn main() {
    try {try all {let pending=child();let bad=CInt8(300);await pending;} finally {rinzu.stdio.writeLine("settled");}}
    catch error {rinzu.stdio.writeLine("caught");}
}
"#;
    let artifact =
        compile_library(&SourceFile::new(FileId(1), "group-mode.rinzu", source)).unwrap();
    assert!(
        artifact
            .ir
            .functions
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.operations)
            .any(|op| matches!(op.kind, OperationKind::EnterErrorGroup { .. }))
    );
    rinzu_compiler::verify_module(&artifact.ir).unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.stdout, b"42\nsettled\ncaught\n");
    assert!(
        run.failure_details.is_none() && run.resources_returned,
        "{run:?}"
    );
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}
