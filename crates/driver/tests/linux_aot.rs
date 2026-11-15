#![cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]

#[path = "support/scratch.rs"]
#[allow(dead_code)]
mod scratch;

use object::{Architecture, BinaryFormat, Object, ObjectKind};
use scratch::Scratch;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn build(root: &Path, input: &Path, executable: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rinzu"))
        .current_dir(root)
        .env("RINZU_CACHE_DIR", root.join("cache"))
        .arg("build")
        .arg(input)
        .arg("--target")
        .arg("x86_64-unknown-linux-gnu")
        .arg("--output")
        .arg(executable)
        .output()
        .unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn synchronous_file_and_package_emit_and_execute_x86_64_elf() {
    let scratch = Scratch::new("rinzu-linux-aot");
    let root = scratch.path();
    let source = r#"
import rinzu.option;
import rinzu.list;
import rinzu.process;
import rinzu.stdio;

fn main() {
    try {
        let args = rinzu.process.arguments();
        let value = rinzu.option.getOrElse(Option<Int>.None(), fn () -> Int { return 42; });
        if rinzu.list.length(args) != 2 {
            panic("unexpected argument count");
        }
        let first = rinzu.option.getOrElse(rinzu.list.get(args, 0), fn () -> String { return "missing"; });
        let second = rinzu.option.getOrElse(rinzu.list.get(args, 1), fn () -> String { return "missing"; });
        rinzu.stdio.writeLine(f"{first}:{second}:{value}");
        let accepted = rinzu.process.setExitStatus(7);
        if !accepted {
            panic("invalid exit status");
        }
    }
    catch errors {
        panic("unexpected failure");
    }
}
"#;
    let file = root.join("program.rinzu");
    fs::write(&file, source).unwrap();
    let package = root.join("package");
    fs::create_dir_all(package.join("src")).unwrap();
    fs::write(
        package.join("rinzu.toml"),
        "[package]\nname = \"sample\"\nlanguage = \"0.5\"\n",
    )
    .unwrap();
    fs::write(package.join("src/main.rinzu"), source).unwrap();

    for (input, name) in [(&file, "from-file"), (&package, "from-package")] {
        let executable = root.join(name);
        assert_success(&build(root, input, &executable));
        let object = fs::read(root.join(format!("{name}.o"))).unwrap();
        let object = object::File::parse(object.as_slice()).unwrap();
        assert_eq!(object.format(), BinaryFormat::Elf);
        assert_eq!(object.architecture(), Architecture::X86_64);
        assert_eq!(object.kind(), ObjectKind::Relocatable);
        let binary = fs::read(&executable).unwrap();
        let binary = object::File::parse(binary.as_slice()).unwrap();
        assert_eq!(binary.format(), BinaryFormat::Elf);
        assert_eq!(binary.architecture(), Architecture::X86_64);
        let metadata: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join(format!("{name}.metadata.json"))).unwrap())
                .unwrap();
        assert_eq!(metadata["target"], "x86_64-unknown-linux-gnu");
        assert_eq!(metadata["compilation"]["target"], metadata["target"]);
        assert!(metadata["compile"]["stackMaps"].as_u64().unwrap() > 0);

        let run = Command::new(&executable)
            .args(["hello", ""])
            .output()
            .unwrap();
        assert_eq!(
            run.status.code(),
            Some(7),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        assert_eq!(run.stdout, b"hello::42\n");
        let failed = Command::new(&executable).output().unwrap();
        assert_eq!(failed.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&failed.stderr).contains("unexpected argument count"));
    }
}

#[test]
fn async_aot_resumes_timer_and_worker_continuations() {
    let scratch = Scratch::new("rinzu-linux-async");
    let root = scratch.path();
    let file = root.join("async_main.rinzu");
    let executable = root.join("async-main");
    fs::write(
        &file,
        r#"
import rinzu.stdio;
import rinzu.timer;

fn score(input: Int) -> Int {
    return input * 2;
}

async fn main() {
    try {
        let timer = rinzu.timer.sleep(2);
        let value = await spawn score(21);
        await timer;
        await timer;
        rinzu.stdio.writeLine(f"worker:{value}");
    }
    catch errors {
        panic("unexpected async failure");
    }
}
"#,
    )
    .unwrap();
    assert_success(&build(root, &file, &executable));
    let run = Command::new(executable).output().unwrap();
    assert_success(&run);
    assert_eq!(run.stdout, b"worker:42\n");
}

#[test]
fn elf_sdk_callback_handles_spilled_arguments_after_relocation() {
    let scratch = Scratch::new("rinzu-linux-elf-callback");
    let root = scratch.path();
    let sdk = root.join("sdk");
    let app = root.join("app");
    let dist = root.join("dist");
    fs::create_dir_all(sdk.join("src")).unwrap();
    fs::create_dir_all(app.join("src")).unwrap();
    fs::create_dir(&dist).unwrap();
    fs::write(
        sdk.join("rinzu.toml"),
        r#"
[package]
name = "callbacks"
language = "0.5"
kind = "sdk"
entry = "src/api.rinzu"
[native]
headers = ["api.h"]
objects = ["libcallback.so"]
"#,
    )
    .unwrap();
    fs::write(
        sdk.join("src/api.rinzu"),
        r#"
native {
    fn apply(mode: CInt, callback: fn (CInt, CInt, CInt, CInt, CInt, CInt, CInt,
                          CDouble, CDouble, CDouble, CDouble, CDouble,
                          CDouble, CDouble, CDouble, CDouble) -> CDouble) -> CDouble
        symbol "apply_callback";
}

fn observe(a: CInt, b: CInt, c: CInt, d: CInt, e: CInt, f: CInt, g: CInt,
           h: CDouble, i: CDouble, j: CDouble, k: CDouble, l: CDouble,
           m: CDouble, n: CDouble, o: CDouble, p: CDouble) -> CDouble fallback -1.0 {
    if p == -16.0 {
        let invalid = CInt8(300);
    }
    if a != 1 || b != 2 || c != 3 || d != 4 || e != 5 || f != 6 || g != 7
        || h != 8.0 || i != 9.0 || j != 10.0 || k != 11.0 || l != 12.0
        || m != 13.0 || n != 14.0 || o != 15.0 || p != 16.0 {
        return -2.0;
    }
    return 42.5;
}

fn value() -> CDouble {
    if apply(2, observe) != -1.0 {
        panic("callback did not return its fallback");
    }
    return apply(0, observe);
}
fn foreign() -> CDouble {
    return apply(1, observe);
}
export value;
export foreign;
"#,
    )
    .unwrap();
    let header = "typedef double (*callback_t)(int, int, int, int, int, int, int, double, double, double, double, double, double, double, double, double);\ndouble apply_callback(int mode, callback_t);\n";
    fs::write(sdk.join("api.h"), header).unwrap();
    fs::write(
        sdk.join("api.c"),
        r#"
#include "api.h"
#include <pthread.h>

struct Invocation {
    callback_t callback;
    int mode;
    double result;
};

static void *invoke(void *context) {
    struct Invocation *call = context;
    call->result = call->callback(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
                                  call->mode == 2 ? -16 : 16);
    return 0;
}

double apply_callback(int mode, callback_t callback) {
    struct Invocation call = {callback, mode, 0};
    if (mode == 1) {
        pthread_t thread;
        if (pthread_create(&thread, 0, invoke, &call) || pthread_join(thread, 0))
            return -3;
    } else {
        invoke(&call);
    }
    return call.result;
}
"#,
    )
    .unwrap();
    let clang = std::env::var_os("RINZU_CLANG").unwrap_or_else(|| "clang".into());
    let mut compile = Command::new(&clang);
    if let Some(sysroot) = std::env::var_os("RINZU_SYSROOT") {
        compile.arg("--sysroot").arg(sysroot);
    }
    assert_success(
        &compile
            .current_dir(&sdk)
            .args([
                "-shared",
                "-fPIC",
                "-pthread",
                "-Wl,--build-id,-soname,libcallback.so.1",
                "api.c",
                "-o",
                "libcallback.so",
            ])
            .output()
            .unwrap(),
    );
    fs::write(
        app.join("rinzu.toml"),
        r#"
[package]
name = "app"
language = "0.5"
[dependencies]
callbacks = {path = "../sdk"}
"#,
    )
    .unwrap();
    fs::write(
        app.join("src/main.rinzu"),
        r#"
import callbacks;
import rinzu.process;
import rinzu.list;

async fn main() {
    try {
        if rinzu.list.length(rinzu.process.arguments()) != 0 {
            let unexpected = callbacks.foreign();
            return;
        }
    }
    catch errors {
        panic("invalid callback test arguments");
    }
    let computed = await spawn callbacks.value();
    if callbacks.value() != 42.5 || computed != 42.5 {
        panic("callback ABI mismatch");
    }
}
"#,
    )
    .unwrap();
    assert_success(&build(root, &app, &dist.join("app")));
    let relocated = root.join("relocated");
    fs::rename(dist, &relocated).unwrap();
    fs::write(sdk.join("api.h"), header.replace("int mode", "double mode")).unwrap();
    let bad_header = build(root, &app, &root.join("bad-header"));
    assert!(
        !bad_header.status.success(),
        "incompatible C header was accepted"
    );
    assert!(!root.join("bad-header").exists());
    fs::write(sdk.join("api.h"), header).unwrap();
    assert_success(
        &Command::new(clang)
            .current_dir(&sdk)
            .args([
                "--target=aarch64-linux-gnu",
                "-nostdinc",
                "-x",
                "c",
                "-c",
                "-o",
                "libcallback.so",
                "-",
            ])
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap(),
    );
    let wrong_arch = build(root, &app, &root.join("wrong-arch"));
    assert!(!wrong_arch.status.success(), "AArch64 input was accepted");
    assert!(!root.join("wrong-arch").exists());
    fs::remove_dir_all(sdk).unwrap();
    fs::remove_dir_all(root.join("cache")).unwrap();
    let executable = relocated.join("app");
    assert_success(
        &Command::new(&executable)
            .current_dir(root)
            .output()
            .unwrap(),
    );
    let foreign = Command::new(&executable).arg("foreign").output().unwrap();
    assert_eq!(foreign.status.code(), Some(86), "{foreign:?}");
    assert!(String::from_utf8_lossy(&foreign.stderr).contains("no-compatible-origin"));
    let sidecars = relocated.join("app.source-packages");
    let bundled_image = fs::read_dir(sidecars)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path().join("libcallback.so.1"))
        .find(|path| path.is_file())
        .expect("bundled ELF callback provider");
    let mut bytes = fs::read(&bundled_image).unwrap();
    bytes.push(0);
    fs::write(bundled_image, bytes).unwrap();
    let failed = Command::new(executable).output().unwrap();
    assert_eq!(
        failed.status.code(),
        Some(78),
        "modified ELF provider was accepted"
    );
}

#[test]
fn linux_process_exit_status_needs_no_additional_sdk() {
    let scratch = Scratch::new("rinzu-linux-exit");
    let root = scratch.path();
    let file = root.join("exit.rinzu");
    let executable = root.join("exit");
    fs::write(
        &file,
        "import rinzu.process;\nfn main() { let accepted = rinzu.process.setExitStatus(42); }\n",
    )
    .unwrap();
    assert_success(&build(root, &file, &executable));
    let run = Command::new(executable).output().unwrap();
    assert_eq!(run.status.code(), Some(42));
}
