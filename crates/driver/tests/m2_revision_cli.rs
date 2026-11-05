#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::path::Path;
use std::process::Command;

#[path = "fixtures/checked_entry.rs"]
mod checked_entry;
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

#[test]
fn native_compiled_jit_collects_unbound_and_child_errors_and_matches_aot() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join(format!("rinzu-native-all-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("rinzu.toml"),
        "[package]\nname=\"native_all\"\nlanguage=\"0.5\"\n[dependencies]\n\"rinzu.sqlite\"={path=\"../../sdk/sqlite\"}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/main.rinzu"),
        checked_entry::source(
            r#"import rinzu.list;

import rinzu.sqlite;
import rinzu.stdio;
async fn failed() {let invalid=CInt8(300);}
async fn main() {
    let connection=rinzu.sqlite.openMemory();
    try all {
        let pending=failed();
        await pending;
        await pending;
        let value:Int=Int(CInt8(300));
        rinzu.stdio.writeLine(value);
        rinzu.stdio.writeLine("continued");
    } catch errors {
        rinzu.stdio.writeLine(rinzu.list.length(errors));
        switch rinzu.list.get(errors,2) {
            case Option.None: {panic("bad");}
            case Option.Some(problem): {rinzu.stdio.writeLine(errorCode(problem));}
        }
    } finally {
        let closed=rinzu.sqlite.close(connection);
        rinzu.stdio.writeLine("closed");
    }
}
"#,
        ),
    )
    .unwrap();
    let expected = b"continued\n3\n8\nclosed\n";
    for command in ["run", "verify-aot", "build"] {
        let output = Command::new(frozen_cli::path())
            .arg(command)
            .arg(&root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{command}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        if command == "run" {
            assert_eq!(output.stdout, expected);
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .contains("compiled JIT continuation runtime")
            );
        }
    }
    let output = Command::new(root.join("build/native_all"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, expected);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn anonymous_record_values_and_future_types_have_source_level_display_on_jit_and_aot() {
    let root = std::env::temp_dir().join(format!("rinzu-m2r-display-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("rinzu.toml"),
        "[package]\nname=\"public_display\"\nlanguage=\"0.5\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/main.rinzu"),
        checked_entry::source(
            r#"
import rinzu.stdio;
type Point = { x:Int, label:String };
class Counter { value:Int }
class Box<T> { const value:T }
async fn ready()->List<Point> { return [{label="one", x=1}]; }
fn callable()->fn(Point)->Point {
    return fn(point:Point)->Point { return point; };
}
async fn main() {
    let point:Point = {x=1, label="one"};
    rinzu.stdio.writeLine(point);
    rinzu.stdio.writeLine({point=point, number:Int=2});
    rinzu.stdio.writeLine(Box<Point>(value:point));
    rinzu.stdio.writeLine(Counter(value:2));
    let pending=ready();
    let apply=callable();
    rinzu.stdio.writeLine(pending);
    let list=await pending;
    rinzu.stdio.writeLine(list);
    rinzu.stdio.writeLine(apply(point));
    rinzu.stdio.writeLine({apply=apply});
    rinzu.stdio.writeLine(f"value={point}");
}
"#,
        ),
    )
    .unwrap();
    let expected = concat!(
        "{label=\"one\", x=1}\n",
        "{number=2, point={label=\"one\", x=1}}\n",
        "Box<{label:String,x:Int}>(value: {label=\"one\", x=1})\n",
        "<class Counter>\n",
        "<Future<List<{label: String, x: Int}>>>\n",
        "[{label=\"one\", x=1}]\n",
        "{label=\"one\", x=1}\n",
        "{apply=<function>}\n",
        "value={label=\"one\", x=1}\n",
    )
    .as_bytes();
    for command in ["check", "run", "verify-aot", "build"] {
        let output = Command::new(frozen_cli::path())
            .arg(command)
            .arg(&root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{command}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        if command == "run" {
            assert_eq!(output.stdout, expected);
        }
    }
    let binary = root.join("build/public_display");
    let output = Command::new(&binary).output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, expected);
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let sanitized = root.join("build/public_display_sanitized");
    let compiled = Command::new("clang")
        .args([
            "-std=c11",
            "-O1",
            "-g",
            "-Werror",
            "-pthread",
            "-fsanitize=address,undefined",
            "-fno-omit-frame-pointer",
        ])
        .arg(binary.with_extension("o"))
        .arg(binary.with_extension("runtime.c"))
        .arg(manifest.join("../runtime/src/callback_entry.c"))
        .arg(manifest.join("../runtime/src/callback_template.S"))
        .arg(manifest.join("../runtime/src/owner_dispatcher.c"))
        .args(["-framework", "CoreFoundation", "-I"])
        .arg(manifest.join("../../include"))
        .arg("-o")
        .arg(&sanitized)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let run = Command::new(&sanitized)
        .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
        .env("UBSAN_OPTIONS", "halt_on_error=1:print_stacktrace=1")
        .output()
        .unwrap();
    assert!(run.status.success(), "{run:?}");
    assert_eq!(run.stdout, expected);
    std::fs::remove_dir_all(root).unwrap();
}
