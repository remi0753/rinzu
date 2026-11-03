#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "../test-support/library.rs"]
mod test_library;
use rinzu_bindgen::source_package::{SourceInterface, generate_source_package, sha256};
use rinzu_codegen::source_package::{LoadedSourcePackages, SourcePackageArtifact};
use rinzu_codegen::{
    BaselineOptions, SourceAsyncOptions, run_baseline_jit_with_package_bindings,
    run_source_async_report,
};
use rinzu_compiler::{FileId, SourceFile};
use test_library::compile_library;
#[test]
fn rust_backends_keep_typed_capture_roots_and_target_fallback() {
    let mut interface: SourceInterface = serde_json::from_str(include_str!(
        "../../../tests/fixtures/source-package/interface.json"
    ))
    .unwrap();
    interface.package.exports = vec!["invoke".into()];
    interface.constants.clear();
    interface.constant_types.clear();
    let function = &mut interface.functions[0];
    function.identity = "invoke".into();
    function.source_type = "native(fn(Int)->Int)->Int".into();
    function.symbol = Some("capture_invoke".into());
    function.parameter_requirements = vec!["fn(Int)->Int".into()];
    function.c_prototype = "int64_t capture_invoke(int64_t (*handler)(int64_t))".into();
    let generated = generate_source_package(interface, b"capture fixture").unwrap();
    let directory = std::env::temp_dir().join(format!("rinzu-capture-rust-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("source-package.h"), generated.header).unwrap();
    std::fs::write(directory.join("table.c"), generated.c_table).unwrap();
    std::fs::write(directory.join("api.c"),"#include <stdint.h>\nint64_t capture_invoke(int64_t (*handler)(int64_t)){return handler(3);}\n").unwrap();
    let library = directory.join("fixture.dylib");
    let run = std::process::Command::new("clang")
        .args(["-std=c11", "-Werror", "-dynamiclib", "-I"])
        .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
        .arg(directory.join("table.c"))
        .arg(directory.join("api.c"))
        .arg("-o")
        .arg(&library)
        .output()
        .unwrap();
    assert!(run.status.success(), "{run:?}");
    let package = SourcePackageArtifact {
        facade_digest: Some(sha256(&std::fs::read(&library).unwrap())),
        library: Some(library),
        expected_wire: generated.wire,
    };
    let text = r#"import rinzu.stdio;
native {fn invoke(handler:fn(Int)->Int)->Int symbol "capture_invoke";}
class Editor {value:Int}
fn make()->fn(Int)->Int {
    let editor=Editor(value:39);
    return fn(value:Int)->Int fallback -7 {
        var i=0;while i<300 {let moved=f"gc-{i}";i=i+1;}
        return invoke(fn(n:Int)->Int {editor.value=editor.value+n;return editor.value;});
    };
}
fn main() {
    rinzu.stdio.writeLine(invoke(make()));
    let editor=Editor(value:0);
    rinzu.stdio.writeLine(invoke(fn(value:Int)->Int fallback -9 {let invalid=CInt8(300);return editor.value+value;}));
}"#;
    let artifact = compile_library(&SourceFile::new(FileId(1), "capture.rinzu", text)).unwrap();
    let run = run_baseline_jit_with_package_bindings(
        &artifact.ir,
        "main",
        BaselineOptions {
            capture_stdout: true,
            collect_every_allocation: true,
            ..Default::default()
        },
        None,
        Some(LoadedSourcePackages::load(std::slice::from_ref(&package)).unwrap()),
    )
    .unwrap();
    assert_eq!(run.result, Ok(0));
    assert_eq!(run.stdout, b"42\n-9\n");
    assert_eq!(run.heap_objects, 0);
    assert!(run.collection_count > 300);
    assert_eq!(run.native_calls.live_calls, 0);
    let artifact = compile_library(&SourceFile::new(
        FileId(1),
        "capture-async.rinzu",
        text.replace("fn main()", "async fn main()"),
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            gc_stress: true,
            capture_stdout: true,
            source_packages: Some(LoadedSourcePackages::load(&[package]).unwrap()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:?}");
    assert_eq!(run.stdout, b"42\n-9\n");
    assert!(run.heap.collections > 300);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
    assert_eq!(run.native_calls.live_calls, 0);
    std::fs::remove_dir_all(directory).unwrap();
}
