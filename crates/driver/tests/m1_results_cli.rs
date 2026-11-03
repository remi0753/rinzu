#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::process::Command;

#[path = "fixtures/native_opaque_sdk/setup.rs"]
mod fixture;

#[path = "fixtures/checked_entry.rs"]
mod checked_entry;
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
#[test]
fn explicit_native_null_to_option_preserves_jit_aot_result_semantics() {
    let root = std::env::temp_dir().join(format!("rinzu-m1-native-results-{}", std::process::id()));
    fixture::create(&root);
    let app = root.join("app");
    let manifest = std::fs::read_to_string(app.join("rinzu.toml"))
        .unwrap()
        .replace("0.2", "0.5");
    std::fs::write(app.join("rinzu.toml"), manifest).unwrap();
    for entry in ["fn main()", "async fn main()"] {
        // The fixture's objects.failure() deterministically produces only NULL.
        // General SDK wrappers should classify Error codes (M1.06), rather than
        // convert every recoverable Error into absence.
        let source = format!(
            r#"
import objects;
import rinzu.stdio;
enum Result<T,E> {{Ok(value:T),Err(error:E)}}
fn missing() -> Option<Int> {{
    try {{ let raw = objects.failure(); return Option<Int>.Some(value: 99); }}
    catch nullError {{ return Option<Int>.None(); }}
}}
{entry} {{
    let none = missing();
    let some = Option<Int>.Some(value: objects.measure(42));
    rinzu.stdio.writeLine(none == Option<Int>.None());
    switch some {{ case Option.Some(value): {{ rinzu.stdio.writeLine(value); }} case Option.None: {{}} }}
    switch Result<Int, Int>.Err(error: 7) {{ case Result.Ok(value): {{}} case Result.Err(value): {{ rinzu.stdio.writeLine(value); }} }}
}}
"#
        );
        std::fs::write(app.join("src/main.rinzu"), checked_entry::source(&source)).unwrap();
        for command in ["check", "run", "build", "verify-aot"] {
            let output = Command::new(frozen_cli::path())
                .args([command])
                .arg(&app)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{command}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            if command == "run" {
                assert_eq!(output.stdout, b"true\n42\n7\n");
            }
        }
        let output = Command::new(app.join("build/opaque_app")).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"true\n42\n7\n");
    }
    std::fs::remove_dir_all(root).unwrap();
}
