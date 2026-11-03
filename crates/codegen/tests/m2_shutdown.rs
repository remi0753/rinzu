#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "../test-support/library.rs"]
mod test_library;
use rinzu_codegen::{SourceAsyncOptions, run_source_async_report};
use rinzu_compiler::{FileId, SourceFile};
use rinzu_runtime::failure::{ErrorKind, FailureCause};
use test_library::compile_library;
fn run(source: &str) -> rinzu_codegen::SourceAsyncRun {
    let artifact = compile_library(&SourceFile::new(FileId(0), "control.rinzu", source)).unwrap();
    let result = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(result.resources_returned, "{result:?}");
    assert_eq!((result.heap.objects, result.heap.roots), (0, 0));
    result
}
const WORK: &str = "fn work()->Int {var i=0;while i<1000000 {i=i+1;}return 42;} async fn turn(){}";
#[test]
fn ready_and_explicit_cancel_preserve_completed_values_and_settle_pending_children() {
    let source = format!(
        r#"import rinzu.stdio;{WORK} async fn ready()->Int{{return 42;}} async fn pending(){{let f=spawn work();try{{let value=await f;}}finally{{try{{rinzu.stdio.writeLine("CHILD:CLOSE");}}catch errors{{}}}}}} async fn main(){{let r=ready();let first=await r;rinzu.stdio.writeLine(futureReady(r));rinzu.stdio.writeLine(cancelFuture(r));rinzu.stdio.writeLine(await r);try all{{let f=pending();await turn();rinzu.stdio.writeLine(cancelFuture(f));rinzu.stdio.writeLine(cancelFuture(f));}}finally{{rinzu.stdio.writeLine("OWNER:CLOSE");}}}}"#
    );
    let result = run(&source);
    assert!(
        result.failure_details.is_none(),
        "{:?}",
        result.failure_details
    );
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(text.starts_with("true\nfalse\n42\n"), "{text}");
    assert!(text.ends_with("OWNER:CLOSE\n"), "{text}");
    assert!(text.contains("CHILD:CLOSE"), "{text}");
}
#[test]
fn explicit_cancel_keeps_cleanup_failure() {
    let source = format!(
        "{WORK} async fn pending(){{let f=spawn work();try{{let value=await f;}}finally{{let bad=CInt8(300);}}}} async fn main(){{let f=pending();await turn();let c=timeoutFuture(f);await f;}}"
    );
    let result = run(&source);
    let failure = result.failure_details.unwrap();
    assert_eq!(failure.primary.cause, FailureCause::Cancelled);
    assert!(
        failure
            .cleanup
            .iter()
            .any(|d| d.cause == FailureCause::Error(ErrorKind::Conversion)),
        "{failure:?}"
    );
}
#[test]
fn root_shutdown_drains_children_after_lexical_cleanup() {
    let source = format!(
        r#"import rinzu.stdio;{WORK} async fn child(){{let f=spawn work();try{{let value=await f;}}finally{{rinzu.stdio.writeLine("CHILD:CLOSE");}}}} async fn main(){{try all{{let f=child();await turn();let q=requestShutdown();await f;}}finally{{rinzu.stdio.writeLine("OWNER:CLOSE");}}}}"#
    );
    let result = run(&source);
    let failure = result.failure_details.unwrap();
    assert_eq!(failure.primary.cause, FailureCause::Shutdown);
    assert!(failure.cleanup.is_empty(), "{failure:?}");
    assert_eq!(
        String::from_utf8(result.stdout).unwrap(),
        "OWNER:CLOSE\nCHILD:CLOSE\n"
    );
}

#[test]
fn repeated_shutdown_runs_join_cleanup_children_and_return_all_heap_roots() {
    let source = r#"import rinzu.stdio; class Editor {value:Int}
        fn work()->Int {return 42;}
        async fn finish(editor:Editor) {let f=spawn work();editor.value=await f;rinzu.stdio.writeLine(editor.value);}
        async fn main(){let editor=Editor(value:0);try all{let q=requestShutdown();}finally{let pending=finish(editor);await pending;}}"#;
    let artifact = compile_library(&SourceFile::new(FileId(0), "repeat.rinzu", source)).unwrap();
    for _ in 0..50 {
        let result = run_source_async_report(
            &artifact.ir,
            "main",
            SourceAsyncOptions {
                capture_stdout: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            result.shutdown_complete && result.resources_returned,
            "{result:?}"
        );
        assert_eq!((result.heap.objects, result.heap.roots), (0, 0));
        assert_eq!(result.stdout, b"42\n");
    }
}

#[test]
fn nested_shutdown_joins_do_not_turn_root_control_into_cleanup_failure() {
    for cleanup in ["", "let bad=CInt8(300);"] {
        let source = format!(
            r#"import rinzu.stdio;import rinzu.timer;{WORK}
async fn timer(milliseconds: Int) -> Unit {{ await rinzu.timer.sleep(milliseconds); }}
native {{}}
            async fn background() {{let worker=spawn work();try all{{await timer(100);let value=await worker;}}finally{{rinzu.stdio.writeLine("CHILD");{cleanup}}}}}
            async fn window() {{try all{{let pending=background();await turn();let q=requestShutdown();await pending;}}finally{{rinzu.stdio.writeLine("WINDOW");}}}}
            async fn database() {{try all{{await window();}}finally{{rinzu.stdio.writeLine("DATABASE");}}}}
            async fn main() {{try all{{await database();}}finally{{rinzu.stdio.writeLine("APP");}}}}"#
        );
        let result = run(&source);
        let failure = result.failure_details.unwrap();
        assert_eq!(failure.primary.cause, FailureCause::Shutdown);
        assert!(
            failure
                .cleanup
                .iter()
                .all(|d| d.cause != FailureCause::Shutdown),
            "{failure:?}"
        );
        assert_eq!(
            failure.cleanup.len(),
            usize::from(!cleanup.is_empty()),
            "{failure:?}"
        );
        assert_eq!(result.shutdown_complete, cleanup.is_empty());
        assert_eq!(result.stdout, b"WINDOW\nCHILD\nDATABASE\nAPP\n");
    }
}

#[test]
fn cleanup_mask_is_inherited_through_async_grandchildren() {
    let source = r#"import rinzu.stdio;
        import rinzu.timer;
async fn timer(milliseconds: Int) -> Unit { await rinzu.timer.sleep(milliseconds); }

        async fn grandchild(){await timer(1);rinzu.stdio.writeLine("GRANDCHILD");}
        async fn child(){await grandchild();rinzu.stdio.writeLine("CHILD");}
        async fn main(){try all{let q=requestShutdown();}finally{let pending=child();await pending;}}"#;
    let result = run(source);
    assert!(result.shutdown_complete, "{:?}", result.failure_details);
    assert_eq!(result.stdout, b"GRANDCHILD\nCHILD\n");
}

#[test]
fn cleanup_mask_is_inherited_by_sync_save_and_close_helpers() {
    let result = run(r#"import rinzu.stdio;
        fn save(){rinzu.stdio.writeLine("SAVE");}
        fn close(){save();rinzu.stdio.writeLine("CLOSE");}
        async fn main(){try all{let q=requestShutdown();}finally{close();}}"#);
    assert!(result.shutdown_complete, "{:?}", result.failure_details);
    assert_eq!(result.stdout, b"SAVE\nCLOSE\n");
    let local = run(r#"import rinzu.stdio;
        fn save(){rinzu.stdio.writeLine("SAVE");}
        fn close(){save();rinzu.stdio.writeLine("CLOSE");}
        fn shutdown(){try{let q=requestShutdown();}finally{close();}}
        async fn main(){try all{shutdown();}finally{}}"#);
    assert!(local.shutdown_complete, "{:?}", local.failure_details);
    assert_eq!(local.stdout, b"SAVE\nCLOSE\n");
}
