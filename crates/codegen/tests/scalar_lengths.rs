#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use rinzu_codegen::{BaselineOptions, run_baseline_jit_with_options};
use rinzu_compiler::{FileId, SourceFile, compile_source};

#[test]
fn scalar_length_leaf_callbacks_keep_unicode_gc_and_worker_fixture_semantics() {
    let source = include_str!("../../driver/tests/fixtures/scalar_lengths.rinzu")
        .replace("async fn main()", "fn main()")
        .replace("await spawn worker()", "worker()");
    assert!(!source.contains("spawn ") && !source.contains("await "));
    let artifact = compile_source(&SourceFile::new(FileId(1), "lengths.rinzu", source)).unwrap();
    for stress in [false, true] {
        let report = run_baseline_jit_with_options(
            &artifact.ir,
            "main",
            BaselineOptions {
                collect_every_allocation: stress,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(report.result, Ok(0));
        assert_eq!(report.heap_objects, 0);
        if stress {
            assert!(report.collection_count > 100);
        }
    }
}
