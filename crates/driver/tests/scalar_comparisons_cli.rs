#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
#[path = "../../codegen/test-support/scalar_comparisons.rs"]
mod vectors;
use std::path::PathBuf;
use std::process::Command;

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn direct_integer_comparisons_match_jit_aot_gc_and_trace_paths() {
    let directory =
        Scratch(std::env::temp_dir().join(format!("rinzu-comparisons-{}", std::process::id())));
    std::fs::create_dir_all(&directory.0).unwrap();
    let source = directory.0.join("main.rinzu");
    std::fs::write(&source, vectors::source()).unwrap();
    for command in ["run", "verify-aot", "build"] {
        let mut run = Command::new(frozen_cli::path());
        run.arg(command)
            .arg(&source)
            .env("RINZU_GC_STRESS", "1")
            .env("RINZU_ERROR_TRACE", "1")
            .env("RINZU_CACHE_DIR", directory.0.join("cache"));
        if command == "build" {
            run.arg("--output").arg(directory.0.join("app"));
        }
        let output = run.output().unwrap();
        assert!(output.status.success(), "{command}: {output:?}");
    }
    for trace in ["0", "1"] {
        let output = Command::new(directory.0.join("app"))
            .env("RINZU_GC_STRESS", "1")
            .env("RINZU_ERROR_TRACE", trace)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("RINZU_FAILURE=none"));
    }
}
