#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "../test-support/library.rs"]
mod test_library;
use rinzu_codegen::{
    BaselineOptions, SourceAsyncOptions, run_baseline_jit_with_options, run_source_async_report,
};
use rinzu_compiler::{CompilerOptions, FileId, SourceFile};
use test_library::{compile_library, compile_library_with_options};
#[test]
fn typed_conversion_environment_is_rooted_across_scope_cycle_and_pending() {
    for (name, text, expected) in [
        (
            "static",
            include_str!("../../../tests/fixtures/functions/static.rinzu"),
            b"42\n".as_slice(),
        ),
        (
            "immutable",
            include_str!("../../../tests/fixtures/functions/immutable.rinzu"),
            b"escaped:42\n".as_slice(),
        ),
        (
            "state",
            include_str!("../../../tests/fixtures/functions/state.rinzu"),
            b"after\nafter\n".as_slice(),
        ),
        (
            "async",
            include_str!("../../../tests/fixtures/functions/async.rinzu"),
            b"42\n".as_slice(),
        ),
    ] {
        let async_text = if name == "async" {
            text.to_owned()
        } else {
            text.replace("fn main()", "async fn main()")
        };
        let options = CompilerOptions {
            sdk_files: [FileId(1)].into_iter().collect(),
            ..Default::default()
        };
        let artifact = compile_library_with_options(
            &SourceFile::new(FileId(1), format!("{name}.rinzu"), async_text),
            &options,
        )
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
        assert!(run.failure_details.is_none(), "{name}: {run:?}");
        assert_eq!(run.stdout, expected);
        assert!(run.heap.collections > 300 && run.heap.moved_objects > 300);
        assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
        if name != "async" {
            let artifact = compile_library(&SourceFile::new(
                FileId(2),
                format!("{name}-sync.rinzu"),
                text,
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
            assert_eq!(run.stdout, expected);
            assert_eq!(run.heap_objects, 0);
            assert!(run.collection_count > 300);
        }
    }
}
