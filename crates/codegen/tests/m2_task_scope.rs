#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "../test-support/library.rs"]
mod test_library;
use rinzu_codegen::{SourceAsyncOptions, run_source_async_report};
use rinzu_compiler::{FileId, SourceFile};
use test_library::compile_library;

#[test]
fn explicit_awaits_finish_children_before_cleanup_and_nested_return() {
    let source = r#"
import rinzu.stdio;
fn work(value: Int) -> Int { var count=0; while count < 10000 { count=count+1; } return value; }
async fn child(value: Int) { let result = await spawn work(value); rinzu.stdio.writeLine(f"child={result}"); }
async fn body() -> Int {
    try all {
        let one = child(1);
        try all { let two = child(2); await two; await one; return 9; }
        finally { rinzu.stdio.writeLine("inner-close"); }
    } finally { rinzu.stdio.writeLine("outer-close"); }
    return 0;
}
async fn main() { let result=await body(); rinzu.stdio.writeLine(f"return={result}"); }
"#;
    let artifact = compile_library(&SourceFile::new(FileId(1), "scope.rinzu", source)).unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            ..Default::default()
        },
    )
    .unwrap();
    let text = String::from_utf8(run.stdout).unwrap();
    assert!(
        text.find("child=2").unwrap() < text.find("inner-close").unwrap(),
        "{text}"
    );
    assert!(
        text.find("child=1").unwrap() < text.find("outer-close").unwrap(),
        "{text}"
    );
    assert!(
        text.find("inner-close").unwrap() < text.find("outer-close").unwrap(),
        "{text}"
    );
    assert!(text.ends_with("return=9\n"), "{text}");
    assert!(run.failure_details.is_none(), "{:?}", run.failure_details);
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}

#[test]
fn explicit_body_and_cleanup_awaits_preserve_failure_routes() {
    let source = r#"
import rinzu.stdio;
fn work() -> Int { var count=0; while count < 10000 { count=count+1; } return 1; }
async fn cleanup() { let result = await spawn work(); rinzu.stdio.writeLine("cleanup-child"); }
async fn main() {
    try {
        try all { let pending=cleanup(); await pending; let bad=CInt8(200); }
        finally { let finishing=cleanup(); try {rinzu.stdio.writeLine("cleanup-body");} catch errors {} await finishing; }
    } catch error { rinzu.stdio.writeLine("caught"); }
}


"#;
    let artifact =
        compile_library(&SourceFile::new(FileId(2), "scope-fail.rinzu", source)).unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            ..Default::default()
        },
    )
    .unwrap();
    let text = String::from_utf8(run.stdout).unwrap();
    assert!(
        text.contains("cleanup-body\ncleanup-child\ncaught\n"),
        "{text}"
    );
    assert!(run.failure_details.is_none(), "{:?}", run.failure_details);
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}

#[test]
fn late_child_failures_route_after_cleanup_without_losing_cleanup_frames() {
    for cleanup_child in [false, true] {
        let start = if cleanup_child {
            ""
        } else {
            "let pending=badChild();await pending;"
        };
        let finish = if cleanup_child {
            "let pending=badChild();await pending;"
        } else {
            ""
        };
        let text = format!(
            r#"
import rinzu.stdio;
fn work() -> Int {{ return 1; }}
async fn badChild() {{ let result=await spawn work(); let bad=CInt8(200); }}
async fn main() {{
    try {{ try all {{ {start} }} finally {{ rinzu.stdio.writeLine("closed"); {finish} }} }}
    catch error {{ rinzu.stdio.writeLine("caught"); }}
}}
"#
        );
        let artifact =
            compile_library(&SourceFile::new(FileId(3), "late-failure.rinzu", text)).unwrap();
        let run = run_source_async_report(
            &artifact.ir,
            "main",
            SourceAsyncOptions {
                capture_stdout: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(run.stdout, b"closed\ncaught\n");
        assert!(run.failure_details.is_none(), "{:?}", run.failure_details);
        assert!(run.resources_returned);
        assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
    }
}

#[test]
fn explicitly_observed_failure_overrides_return_and_future_alias_survives_collection() {
    let text = r#"
import rinzu.stdio;
fn work() -> String { return "alive"; }
async fn ready() -> String { return "ready"; }
async fn child() -> String { return await spawn work(); }
async fn badChild() { let result=await spawn work(); let bad=CInt8(200); }
async fn body() -> String {
    try { try all { let pending=badChild(); await pending; return "discarded"; } finally {} }
    catch error { return "caught"; }
    return "other";
}

async fn main() {
    let caught=await body(); rinzu.stdio.writeLine(caught);
    var saved=ready();
    try all { saved=child(); } finally {}
    let result=await saved; rinzu.stdio.writeLine(result);
}
"#;
    let artifact =
        compile_library(&SourceFile::new(FileId(4), "return-alias.rinzu", text)).unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.stdout, b"caught\nalive\n");
    assert!(run.failure_details.is_none(), "{:?}", run.failure_details);
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}

#[test]
fn cancellation_waits_for_child_cleanup_and_masks_pending_cleanup_children() {
    let text = r#"
import rinzu.stdio;
fn work() -> Int { var count=0; while count<100000 { count=count+1; } return count; }
async fn child() {
    try { let result=await spawn work(); }
    finally { rinzu.stdio.writeLine("child-cleanup"); }
}
async fn turn() {}
async fn finish() { let result=await spawn work(); rinzu.stdio.writeLine("async-close"); }
async fn main() {
    let pending=child();
    await turn();
    try all {
        var count=0; while count<100000 { count=count+1; }
    } finally {
        try all { await pending; }
        finally { rinzu.stdio.writeLine("close-start"); let finishing=finish(); await finishing; }
    }
}
"#;
    let artifact =
        compile_library(&SourceFile::new(FileId(5), "cancel-scope.rinzu", text)).unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            cancel_after_polls: Some(200),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.stdout, b"child-cleanup\nclose-start\nasync-close\n");
    assert_eq!(
        run.failure_details.as_ref().unwrap().primary.cause,
        rinzu_runtime::failure::FailureCause::Cancelled
    );
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}

#[test]
fn cleanup_collection_observes_its_child_even_after_an_earlier_error() {
    let text = r#"
import rinzu.stdio;
fn work() -> Int {var count=0;while count<10000{count=count+1;}return count;}
async fn finish() {try {let result=await spawn work();} finally {rinzu.stdio.writeLine("cleanup-child-settled");}}
async fn main() {
    try {try all {} finally {try all{let finishing=finish();let bad=CInt8(200);await finishing;}finally{}}}
    catch problem {rinzu.stdio.writeLine("caught");}
}
"#;
    let artifact =
        compile_library(&SourceFile::new(FileId(6), "cleanup-failure.rinzu", text)).unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.stdout, b"cleanup-child-settled\ncaught\n");
    assert!(run.failure_details.is_none());
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}

#[test]
fn resumed_nested_callee_can_explicitly_observe_a_new_child() {
    let text = r#"
import rinzu.stdio;
fn work()->Int{return 7;}
async fn child(){let value=await spawn work();rinzu.stdio.writeLine("child");}
async fn body(){try all {let ready=await spawn work();let pending=child();await pending;}finally{rinzu.stdio.writeLine("closed");}}
async fn main(){await body();}
"#;
    let artifact =
        compile_library(&SourceFile::new(FileId(7), "resume-callee.rinzu", text)).unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.stdout, b"child\nclosed\n");
    assert!(run.failure_details.is_none() && run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}
