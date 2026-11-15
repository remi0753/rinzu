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
fn linux_build_reports_unsupported_async_before_publishing_output() {
    let scratch = Scratch::new("rinzu-linux-async");
    let root = scratch.path();
    let file = root.join("async_main.rinzu");
    let executable = root.join("async-main");
    fs::write(&file, "async fn main() {}\n").unwrap();
    let output = build(root, &file, &executable);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("async AOT is unavailable"));
    assert!(!executable.exists());
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
