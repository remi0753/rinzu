use rinzu_codegen::{SourceAsyncOptions, run_source_async_report};
use rinzu_compiler::{FileId, SourceFile, compile_source};
const SOURCE: &str = include_str!("fixtures/fallible-data.rinzu");
pub const EXPECTED: &[u8] =
    b"42\nhello\n42\n42\n42\n42\n42\n0\n1\n201\n202\n200\n301\n300\n300\n300\n100\n-7\n101\n2\n";

#[test]
fn ordinary_async_finally_preserves_incoming_errors_and_return_roots() {
    let artifact = compile_source(&SourceFile::new(
        FileId(400),
        "async-finally.rinzu",
        include_str!("fixtures/async-finally.rinzu"),
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
    assert!(
        run.failure_details.is_none() && run.resources_returned,
        "{run:#?}"
    );
    assert_eq!(run.stdout, b"body\ncleanup\npreserved-42\n");
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}

#[test]
fn fallible_intrinsics_return_success_values_and_throw_errors_with_gc() {
    let artifact =
        compile_source(&SourceFile::new(FileId(396), "fallible-data.rinzu", SOURCE)).unwrap();
    for gc_stress in [false, true] {
        let run = run_source_async_report(
            &artifact.ir,
            "main",
            SourceAsyncOptions {
                capture_stdout: true,
                gc_stress,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            run.failure_details.is_none() && run.resources_returned,
            "{run:#?}"
        );
        assert_eq!(run.stdout, EXPECTED);
        assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        let sync = SOURCE.replace("async fn main()", "fn main()");
        let artifact = compile_source(&SourceFile::new(
            FileId(397),
            "fallible-data-sync.rinzu",
            sync,
        ))
        .unwrap();
        for collect_every_allocation in [false, true] {
            let run = rinzu_codegen::run_baseline_jit_with_options(
                &artifact.ir,
                "main",
                rinzu_codegen::BaselineOptions {
                    capture_stdout: true,
                    collect_every_allocation,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(run.result, Ok(0), "{run:#?}");
            assert_eq!(run.stdout, EXPECTED);
            assert_eq!(run.heap_objects, 0);
        }
    }
}
