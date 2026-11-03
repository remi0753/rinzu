#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::fs;

#[test]
fn process_status_and_cleanup_match_source_runtime_and_aot() {
    let directory = crate::verify_capture::TemporaryDirectory::new("m5-status").unwrap();
    let root = &directory.0;
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("rinzu.toml"),
        "[package]\nname=\"m5_status\"\nlanguage=\"0.5\"\n",
    )
    .unwrap();
    fs::write(root.join("src/main.rinzu"),r#"import rinzu.stdio;
async fn child(){try {try all {var count=0;while count<1000{count=count+1;}}finally{rinzu.stdio.writeLine("CHILD:SETTLED");}}catch unexpectedChildErrors{panic("childFailure");}}
async fn main(){try {try all{let accepted=setProcessExitStatus(7);let rejected=setProcessExitStatus(-1);if !accepted || rejected{panic("bad");}let pending=child();}finally{rinzu.stdio.writeLine("MAIN:SETTLED");}}catch unexpected{panic("failed");}}"#).unwrap();
    crate::aot::verify_equivalence_with_options(root, rinzu_codegen::BaselineOptions::default())
        .unwrap();
    fs::write(
        root.join("src/main.rinzu"),
        "async fn main(){let accepted=setProcessExitStatus(0);try {let bad=CInt8(300);} catch errors {let failed=setProcessExitStatus(1);}}",
    )
    .unwrap();
    crate::aot::verify_equivalence_with_options(root, rinzu_codegen::BaselineOptions::default())
        .unwrap();
}

#[test]
fn explicit_shutdown_returns_the_requested_status_after_finally() {
    let directory = crate::verify_capture::TemporaryDirectory::new("explicit-shutdown").unwrap();
    let source = directory.0.join("main.rinzu");
    fs::write(&source, r#"import rinzu.stdio;
fn cleanup(){try{rinzu.stdio.writeLine("CLEANUP");}catch errors{panic("cleanup output failed");}}
async fn main(){try{if !setProcessExitStatus(7){panic("invalid exit status");}let requested=requestShutdown();return;}finally{cleanup();}}"#).unwrap();
    crate::aot::verify_equivalence_with_options(&source, Default::default()).unwrap();
    let executable = directory.0.join("shutdown");
    crate::aot::build_package(&source, Some(executable.clone())).unwrap();
    let output = std::process::Command::new(executable).output().unwrap();
    assert_eq!(output.status.code(), Some(7), "{output:?}");
    assert_eq!(output.stdout, b"CLEANUP\n");
}
