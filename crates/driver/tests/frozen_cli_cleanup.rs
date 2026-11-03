#![cfg(unix)]
use std::path::PathBuf;
use std::process::Command;

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn frozen_cli_is_stable_during_a_process_and_removed_on_its_exit() {
    let scratch = Scratch(
        std::env::temp_dir().join(format!("rinzu-frozen-cleanup-test-{}", std::process::id())),
    );
    std::fs::create_dir_all(&scratch.0).unwrap();
    let input = scratch.0.join("input");
    std::fs::write(&input, b"frozen fixture").unwrap();
    let helper =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/frozen_cli.rs");
    let program = format!(
        r#"
        #[path = {helper:?}] mod frozen_cli;
        fn main() {{
            let first = frozen_cli::path();
            assert_eq!(first, frozen_cli::path());
            assert_eq!(std::fs::read(first).unwrap(), b"frozen fixture");
            println!("{{}}", first.display());
            match std::env::args().nth(1).as_deref() {{
                Some("exit") => std::process::exit(7),
                Some("panic") => panic!("intentional fixture exit"),
                _ => {{}}
            }}
        }}
    "#
    );
    let source = scratch.0.join("probe.rs");
    let binary = scratch.0.join("probe");
    std::fs::write(&source, program).unwrap();
    let build = Command::new("rustc")
        .args(["--edition=2024", "-O"])
        .env("CARGO_BIN_EXE_rinzu", &input)
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(build.status.success(), "{build:?}");
    for (mode, status) in [("normal", 0), ("exit", 7), ("panic", 101)] {
        let result = Command::new(&binary)
            .arg(mode)
            .env("RUST_BACKTRACE", "0")
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(status), "{result:?}");
        let frozen = PathBuf::from(String::from_utf8(result.stdout).unwrap().trim());
        assert_eq!(frozen.file_name().unwrap(), "rinzu");
        let directory = frozen.parent().unwrap();
        assert!(
            directory
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("rinzu-integration-cli-")
        );
        let leaked = directory.exists();
        if leaked {
            std::fs::remove_dir_all(directory).unwrap();
        }
        assert!(
            !leaked,
            "frozen CLI survived {mode}: {}",
            directory.display()
        );
    }
    assert_eq!(std::fs::read(input).unwrap(), b"frozen fixture");
}
