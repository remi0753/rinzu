#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "../test-support/library.rs"]
mod test_library;
use rinzu_codegen::{
    BaselineOptions, SourceAsyncOptions, run_baseline_jit_with_options, run_source_async_report,
};
use rinzu_compiler::{FileId, SourceFile};
use test_library::compile_library;

#[test]
fn xor_is_immutable_across_gc_and_worker_transport() {
    let body = r#"
fn transform(data:ByteArray)->ByteArray {return bytesXor(data,CUInt8(255));}
fn verify(data:ByteArray,copy:ByteArray) {
    if data!=byteArray(b"\x00\x01\x7f\x80\xff") || copy!=byteArray(b"\xff\xfe\x80\x7f\x00") {panic("fail");}
    if bytesXor(copy,CUInt8(255))!=data || bytesXor(data,CUInt8(0))!=data || bytesLength(bytesXor(byteArray(b""),CUInt8(255)))!=0 {panic("fail2");}
}
"#;
    let sync = format!(
        "{body} fn main(){{let data=byteArray(b\"\\x00\\x01\\x7f\\x80\\xff\");let copy=transform(data);verify(data,copy);}}"
    );
    let artifact = compile_library(&SourceFile::new(FileId(1), "xor-gc.rinzu", &sync)).unwrap();
    let run = run_baseline_jit_with_options(
        &artifact.ir,
        "main",
        BaselineOptions {
            collect_every_allocation: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.result, Ok(0));
    assert_eq!(run.heap_objects, 0);
    assert!(run.collection_count > 0);
    let async_source = format!(
        "{body} async fn main(){{let data=byteArray(b\"\\x00\\x01\\x7f\\x80\\xff\");let copy=await spawn transform(data);verify(data,copy);}}"
    );
    let artifact = compile_library(&SourceFile::new(
        FileId(1),
        "xor-worker.rinzu",
        &async_source,
    ))
    .unwrap();
    let run = run_source_async_report(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
    assert!(run.failure_details.is_none() && run.resources_returned);
    assert_eq!(run.worker_returns, 1);
}

#[test]
fn status_waits_for_children_and_does_not_mask_runtime_failure() {
    let source = r#"import rinzu.stdio;
async fn child(){try all {var n=0;while n<1000{n=n+1;}}finally{try{rinzu.stdio.writeLine("settled");}catch errors{}}}
async fn main(){let a=setProcessExitStatus(7);let invalid=setProcessExitStatus(256);if !a || invalid{panic("fail");}let childTask=child();}"#;
    let artifact = compile_library(&SourceFile::new(FileId(1), "status.rinzu", source)).unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.exit_status, 7);
    assert_eq!(run.stdout, b"settled\n");
    assert!(run.resources_returned);
    let artifact = compile_library(&SourceFile::new(
        FileId(1),
        "status-fail.rinzu",
        "async fn main(){let a=setProcessExitStatus(0);let bad=CInt8(300);}",
    ))
    .unwrap();
    let run = run_source_async_report(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
    assert_eq!(run.exit_status, 1);
    assert!(run.failure_details.is_some() && run.resources_returned);
}
