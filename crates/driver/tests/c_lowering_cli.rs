#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
#[path = "../../codegen/test-support/scalar_comparisons.rs"]
mod vectors;
use std::process::Command;

#[test]
fn c_functions_preserve_canonical_signed_unsigned_and_bool_boundaries() {
    let directory = std::env::temp_dir().join(format!("rinzu-c-functions-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let source = directory.join("main.rinzu");
    let vectors = vectors::source();
    assert_eq!(vectors.matches("fn main(){").count(), 1);
    let content = format!(
        "import rinzu.process;\n{}\nasync fn main() {{ try {{ let environment = rinzu.process.environment(\"PATH\"); comparisonBody(); }} catch unexpected {{ panic(\"badEnvironment\"); }} }}\n",
        vectors.replace("fn main(){", "fn comparisonBody(){")
    );
    std::fs::write(&source, content).unwrap();
    let run = |arguments: &[&std::ffi::OsStr]| {
        let output = Command::new(frozen_cli::path())
            .args(arguments)
            .env("RINZU_CACHE_DIR", directory.join("cache"))
            .env("RINZU_GC_STRESS", "1")
            .env("RINZU_ERROR_TRACE", "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        output
    };
    let program = directory.join("program");
    run(&[
        "build".as_ref(),
        source.as_os_str(),
        "--output".as_ref(),
        program.as_os_str(),
    ]);
    let metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.join("program.metadata.json")).unwrap())
            .unwrap();
    let selected = metadata["cLowering"]["functions"].as_array().unwrap();
    for name in ["compare0_0", "compare3_0", "compare7_5", "compare8_0"] {
        assert!(
            selected.iter().any(|v| v == name),
            "missing {name}: {selected:?}"
        );
    }
    for trace in ["0", "1"] {
        let output = Command::new(&program)
            .env("RINZU_GC_STRESS", "1")
            .env("RINZU_ERROR_TRACE", trace)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("RINZU_FAILURE=none"));
    }
    let output = run(&["verify-aot".as_ref(), source.as_os_str()]);
    assert!(String::from_utf8_lossy(&output.stdout).contains("JIT/AOT equivalent"));
    let output = run(&["run".as_ref(), source.as_os_str()]);
    assert!(String::from_utf8_lossy(&output.stderr).contains("compiled JIT continuation runtime"));
    std::fs::remove_dir_all(directory).unwrap();
}
