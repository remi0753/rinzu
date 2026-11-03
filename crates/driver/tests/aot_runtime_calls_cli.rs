#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
use object::{Object, ObjectSection, ObjectSymbol, RelocationFlags, RelocationTarget};
use std::path::PathBuf;
use std::process::Command;

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn accompanying_c_runtime_calls_link_directly_and_keep_worker_gc_errors_and_exports() {
    let directory =
        Scratch(std::env::temp_dir().join(format!("rinzu-aot-calls-{}", std::process::id())));
    std::fs::create_dir_all(&directory.0).unwrap();
    let source = directory.0.join("main.rinzu");
    std::fs::write(
        &source,
        r#"
import rinzu.stdio;
error Check {}
class Box {value:Int}
fn work(value:Int)->Int {
    let words=["scope","界🙂"]; let boxValue=Box(value:value);
    if value<0 {throw Check(message:"expected");}
    return boxValue.value+listLength(words);
}
fn workerRoot(value:Int)->Int{try{return work(value);}catch errors{return -1;}}
async fn main(){try {
    let pending=spawn workerRoot(40); let result=await pending;
    rinzu.stdio.writeLine(result);
    try {let invalid=work(-1);}catch[Check]{rinzu.stdio.writeLine(7);}
}catch unexpected{panic("wrong");}}
"#,
    )
    .unwrap();
    for verb in ["run", "verify-aot", "build"] {
        let mut command = Command::new(frozen_cli::path());
        command
            .arg(verb)
            .arg(&source)
            .env("RINZU_GC_STRESS", "1")
            .env("RINZU_ERROR_TRACE", "1")
            .env("RINZU_CACHE_DIR", directory.0.join("cache"));
        if verb == "build" {
            command.arg("--output").arg(directory.0.join("app"));
        }
        let output = command.output().unwrap();
        assert!(output.status.success(), "{verb}: {output:?}");
        if verb == "run" {
            assert_eq!(output.stdout, b"42\n7\n");
        }
    }
    let bytes = std::fs::read(directory.0.join("app.o")).unwrap();
    let object = object::File::parse(bytes.as_slice()).unwrap();
    let mut calls = 0;
    for section in object.sections() {
        for (_, relocation) in section.relocations() {
            let RelocationTarget::Symbol(symbol) = relocation.target() else {
                continue;
            };
            let symbol = object.symbol_by_index(symbol).unwrap();
            if symbol.name().unwrap().starts_with("_rinzu_baseline_") {
                assert!(
                    matches!(
                        relocation.flags(),
                        RelocationFlags::MachO {
                            r_type: object::macho::ARM64_RELOC_BRANCH26,
                            ..
                        }
                    ),
                    "{}: {relocation:?}",
                    symbol.name().unwrap()
                );
                calls += 1;
            }
        }
    }
    assert!(calls > 20, "expected substantive runtime call coverage");
    for sanitized in [false, true] {
        if sanitized {
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            let mut build = Command::new("clang");
            build.args([
                "-target",
                "arm64-apple-macos11",
                "-std=c11",
                "-O2",
                "-pthread",
                "-fsanitize=address,undefined",
                "-fno-sanitize-recover=all",
                "-I",
            ]);
            build
                .arg(root.join("include"))
                .arg(directory.0.join("app.o"))
                .arg(directory.0.join("app.runtime.c"))
                .arg(root.join("crates/runtime/src/callback_entry.c"))
                .arg(root.join("crates/runtime/src/callback_template.S"));
            let metadata: serde_json::Value = serde_json::from_slice(
                &std::fs::read(directory.0.join("app.metadata.json")).unwrap(),
            )
            .unwrap();
            for path in metadata["nativeInputs"]["objects"].as_array().unwrap() {
                build.arg(path.as_str().unwrap());
            }
            for path in metadata["nativeInputs"]["searchPaths"].as_array().unwrap() {
                build.arg("-L").arg(path.as_str().unwrap());
            }
            for library in metadata["nativeInputs"]["libraries"].as_array().unwrap() {
                build.arg(format!("-l{}", library.as_str().unwrap()));
            }
            let result = build
                .arg("-o")
                .arg(directory.0.join("app"))
                .output()
                .unwrap();
            assert!(result.status.success(), "{result:?}");
        }
        let output = Command::new(directory.0.join("app"))
            .env("RINZU_GC_STRESS", "1")
            .env("RINZU_ERROR_TRACE", "1")
            .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
            .env("RINZU_CACHE_DIR", directory.0.join("cache"))
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(output.stdout, b"42\n7\n");
        assert!(String::from_utf8_lossy(&output.stderr).contains("RINZU_FAILURE=none"));
    }
}
