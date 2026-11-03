#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "../test-support/scalar_comparisons.rs"]
mod vectors;
use rinzu_codegen::{BaselineOptions, run_baseline_jit_with_options};
use rinzu_compiler::{FileId, SourceFile, compile_source};

#[test]
fn integer_boundaries_and_bool_comparisons_match_the_scalar_algebra() {
    let artifact = compile_source(&SourceFile::new(
        FileId(319),
        "comparisons.rinzu",
        vectors::source(),
    ))
    .unwrap();
    let run = run_baseline_jit_with_options(
        &artifact.ir,
        "main",
        BaselineOptions {
            capture_error_trace: true,
            collect_every_allocation: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.result, Ok(0), "{run:#?}");
    assert!(run.failure_details.is_none());
    assert_eq!(run.heap_objects, 0);
}
