#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "../test-support/library.rs"]
mod test_library;
use rinzu_codegen::{SourceAsyncOptions, run_source_async_report};
use rinzu_compiler::{FileId, SourceFile};
use test_library::compile_library;
#[test]
fn continuation_unbound_assignment_and_explicit_observation_match_the_contract() {
    let artifact = compile_library(&SourceFile::new(
        FileId(1),
        "try-all.rinzu",
        include_str!("../../../tests/fixtures/programs/try-all/src/main.rinzu"),
    ))
    .unwrap();
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
    assert_eq!(
        run.stdout,
        b"nested\nordinary\n7\nbody\nsettled\nerrors=9\nfinally\n42\n"
    );
    assert_eq!(run.caught_errors.len(), 10);
    assert_eq!(
        run.caught_errors
            .iter()
            .filter(|f| f.primary.cause
                == rinzu_runtime::failure::FailureCause::Error(
                    rinzu_runtime::failure::ErrorKind::UnboundValue
                ))
            .count(),
        2
    );
    assert!(
        run.failure_details.is_none() && run.resources_returned,
        "{run:?}"
    );
    assert!(run.heap.collections > 300);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}

#[test]
fn control_exits_observe_explicit_awaits_and_skip_unreached_statements() {
    let artifact = compile_library(&SourceFile::new(
        FileId(1),
        "control.rinzu",
        include_str!("../../../tests/fixtures/programs/try-all-control/src/main.rinzu"),
    ))
    .unwrap();
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
    assert_eq!(run.stdout,b"a\nb\nchoose-errors=1\nchoose-finally\n9\nreturn-expression-continued\nexpression-errors=1\nexpression-finally\n7\na\nunhandled-finally\nunhandled=1\na\nheld-errors=1\nheld-finally\nb\nheld\nreplacement-finally\n6\nb\nloop-finally\nb\nloop-finally\n2\ninner-body\ninner-errors=2\ninner-finally\n");
    assert!(
        run.failure_details.is_none() && run.resources_returned,
        "{run:?}"
    );
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}

#[test]
fn cancel_and_fatal_do_not_enter_group_catch_and_wait_for_child_cleanup() {
    let source = r#"
import rinzu.stdio;
fn work()->Int {var n=0;while n<100000{n=n+1;}return n;}
async fn child() {
    try {let done=await spawn work();}
    finally {try {rinzu.stdio.writeLine("child-cleanup");} catch errors {}}
}
async fn turn(){}
async fn main() {
    try all {let pending=child();await turn();var n=0;while n<100000{n=n+1;}}
    catch errors {rinzu.stdio.writeLine("catch-unreachable");}
    finally {rinzu.stdio.writeLine("finally");}
}
"#;
    for (text, cancel, cause) in [
        (
            source.to_owned(),
            Some(200),
            rinzu_runtime::failure::FailureCause::Cancelled,
        ),
        (
            source.replace(
                "let pending=child();await turn();var n=0;while n<100000{n=n+1;}",
                "let pending=child();await turn();let fatal=1/0;",
            ),
            None,
            rinzu_runtime::failure::FailureCause::ArithmeticTrap,
        ),
    ] {
        let artifact = compile_library(&SourceFile::new(FileId(1), "cancel.rinzu", text)).unwrap();
        let run = run_source_async_report(
            &artifact.ir,
            "main",
            SourceAsyncOptions {
                gc_stress: true,
                capture_stdout: true,
                cancel_after_polls: cancel,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(run.stdout, b"finally\nchild-cleanup\n");
        assert_eq!(run.failure_details.as_ref().unwrap().primary.cause, cause);
        assert!(run.resources_returned, "{run:?}");
        assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
    }
}
