#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use rinzu_codegen::{BaselineOptions, run_baseline_jit_with_options};
use rinzu_compiler::check_package;

#[test]
fn generic_algorithms_and_function_references_keep_nested_roots() {
    let (_, artifact) = check_package(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/programs/aggregation"),
    )
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
    assert_eq!(run.stdout,"7\n[6, -2, 6, 4]\n[3, 3, 2]\nOption<Int>.Some(value: 0)\nOption<Int>.None\n[3, -1, 3, 2]\n{\"\": 1, \"a\": 2, \"b\": 1, \"日本語\": 1}\n日本語\0tail\ncallback caught\ncleanup\n".as_bytes());
    assert_eq!(run.heap_objects, 0);
    assert!(run.collection_count > 30);
}
