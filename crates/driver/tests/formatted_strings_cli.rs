#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
#[path = "fixtures/native_opaque_sdk/setup.rs"]
mod opaque_fixture;
#[path = "fixtures/native_record_writeback_sdk/setup.rs"]
#[allow(dead_code)]
mod writeback_fixture;

#[path = "fixtures/checked_entry.rs"]
mod checked_entry;
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

fn package(name: &str, source: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("rinzu-formatted-{name}-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("rinzu.toml"),
        "[package]\nname=\"formatted_app\"\nlanguage=\"0.2\"\n",
    )
    .unwrap();
    std::fs::write(root.join("src/main.rinzu"), checked_entry::source(source)).unwrap();
    root
}

fn cli(command: &str, root: &Path) -> Output {
    let output = Command::new(frozen_cli::path())
        .arg(command)
        .arg(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{command}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn expect_both(root: &Path, expected: &[u8]) {
    assert_eq!(cli("run", root).stdout, expected);
    cli("build", root);
    let aot = Command::new(root.join("build/formatted_app"))
        .output()
        .unwrap();
    assert!(
        aot.status.success(),
        "{}",
        String::from_utf8_lossy(&aot.stderr)
    );
    assert_eq!(aot.stdout, expected);
    cli("verify-aot", root);
    sanitized_aot(root, "formatted_app", expected);
}

fn sanitized_aot(app: &Path, name: &str, expected: &[u8]) {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let binary = app.join("build/formatted-sanitized");
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
        .arg(app.join(format!("build/{name}.o")))
        .arg(app.join(format!("build/{name}.runtime.c")))
        .arg(manifest.join("../runtime/src/callback_entry.c"))
        .arg(manifest.join("../runtime/src/callback_template.S"))
        .arg("-I")
        .arg(manifest.join("../../include"))
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let run = Command::new(binary)
        .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
        .env("UBSAN_OPTIONS", "halt_on_error=1:print_stacktrace=1")
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(run.stdout, expected);
    let report = String::from_utf8_lossy(&run.stderr);
    assert!(
        !report.contains("ERROR: AddressSanitizer") && !report.contains("runtime error:"),
        "{report}"
    );
}

fn expect_sdk(app: &Path, name: &str, expected: &[u8]) {
    let entry = app.join("src/main.rinzu");
    let source = std::fs::read_to_string(&entry).unwrap();
    std::fs::write(entry, checked_entry::source(&source)).unwrap();
    assert_eq!(cli("run", app).stdout, expected);
    cli("build", app);
    let aot = Command::new(app.join("build").join(name)).output().unwrap();
    assert!(
        aot.status.success(),
        "{}",
        String::from_utf8_lossy(&aot.stderr)
    );
    assert_eq!(aot.stdout, expected);
    cli("verify-aot", app);
    sanitized_aot(app, name, expected);
}

#[test]
fn formatted_text_is_a_string_with_escapes_unicode_nul_and_nested_expressions() {
    for (name, main) in [("sync", "fn"), ("async", "async fn")] {
        let source = r#"
import rinzu.stdio;
fn identity(text: String) -> String { return text; }
MAIN main() {
    let text = f"前\{\}:{identity("日本語\u{0}🙂")}:{40 + 2}:{f"内{true}"}";
    rinzu.stdio.writeLine(text);
    rinzu.stdio.writeLine(f"");
    rinzu.stdio.writeLine(f"\u{7B}\u{7D}");
    rinzu.stdio.writeLine(f"quote: {identity("a\"b")}");
    rinzu.stdio.writeLine("{ordinary}");
}

"#
        .replace("MAIN", main);
        let root = package(name, &source);
        expect_both(
            &root,
            "前{}:日本語\0🙂:42:内true\n\n{}\nquote: a\"b\n{ordinary}\n".as_bytes(),
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn all_scalar_and_aggregate_values_use_the_shared_display_rules() {
    let source = r#"
import rinzu.stdio;
enum Choice { empty, item(value: Int) }
class Packet { const text: String, const data: Bytes(4), const choice: Choice, const values: List<Int>, const entries: Map<String, Int> }
fn unit() {}
MAIN main() {
    let largest: CUInt64 = 18446744073709551615;
    rinzu.stdio.writeLine(f"{unit()}|{true}|{false}|{CInt8(-128)}|{CInt16(-32768)}|{CInt32(-2147483648)}|{-9223372036854775808}|{9223372036854775807}");
    rinzu.stdio.writeLine(f"{CUInt8(255)}|{CUInt16(65535)}|{CUInt32(4294967295)}|{largest}|{CFloat(0.1)}|{0.1}|{-0.0}|{1.0}|{0.0 / 0.0}|{1.0 / 0.0}|{-1.0 / 0.0}");
    rinzu.stdio.writeLine(f"{b""}|{b"\x00\xff\"\\"}");
    let packet = Packet(text: "日本語\u{0}\n🙂", data: b"\x00\xff\"\\", choice: Choice.item(value: 7), values: [1, -2], entries: {"z": 2, "a": 1});
    rinzu.stdio.writeLine(f"packet: {packet}");
    let integers: Map<Int, Int> = {2: 20, 10: 100, 1: 10};
    let empty: List<String> = [];
    rinzu.stdio.writeLine(f"{integers}|{empty}|{Choice.empty()}");
}
"#;
    let expected = concat!(
        "()|true|false|-128|-32768|-2147483648|-9223372036854775808|9223372036854775807\n",
        "255|65535|4294967295|18446744073709551615|0.100000001|0.10000000000000001|-0.0|1.0|NaN|inf|-inf\n",
        "b\"\"|b\"\\x00\\xff\\\"\\\\\"\n",
        "packet: Packet(text: \"日本語\\u{00}\\n🙂\", data: b\"\\x00\\xff\\\"\\\\\", choice: Choice.item(value: 7), values: [1, -2], entries: {\"a\": 1, \"z\": 2})\n",
        "{1: 10, 10: 100, 2: 20}|[]|Choice.empty\n"
    );
    for (name, main) in [("values_sync", "fn"), ("values_async", "async fn")] {
        let root = package(name, &source.replace("MAIN", main));
        expect_both(&root, expected.as_bytes());
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn expressions_run_once_in_order_and_errors_skip_remaining_parts() {
    let source = r#"
import rinzu.stdio;
fn mark(value: Int) -> Int { rinzu.stdio.writeLine(f"mark:{value}"); return value; }
fn fail() -> Int { rinzu.stdio.writeLine("fail"); let invalid = CInt8(300); return 9; }
MAIN main() {
    rinzu.stdio.writeLine(f"joined:{mark(1)}:{mark(2)}:{mark(3)}");
    try { let partial = f"{mark(4)}:{fail()}:{mark(5)}"; rinzu.stdio.writeLine(partial); }
    catch [error:Error,...rest] { rinzu.stdio.writeLine(f"caught:{error}"); }
    finally { rinzu.stdio.writeLine(f"finally:{6}"); }
    rinzu.stdio.writeLine("done");
}
"#;
    let expected = b"mark:1\nmark:2\nmark:3\njoined:1:2:3\nmark:4\nfail\ncaught:Error(message: \"numeric conversion is out of range or loses required integer precision\")\nfinally:6\ndone\n";
    for (name, main) in [("effects_sync", "fn"), ("effects_async", "async fn")] {
        let root = package(name, &source.replace("MAIN", main));
        expect_both(&root, expected);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn await_and_workers_keep_earlier_parts_and_format_future_and_function_markers() {
    let root = package(
        "workers",
        r#"
import rinzu.stdio;
class Pending { const value: Future<String> }
fn worker(value: Int) -> String {
    var index = 0;
    while index < 40 { let temporary = [f"temporary:{index}"]; index = index + 1; }
    return f"worker:{value}:日本語\u{0}🙂";
}
async fn pending() -> String {
    let child = spawn worker(7); return f"child:{await child}";
}
fn target(value: Int) -> Int { return value; }
async fn main() {
    let function = target;
    let future = pending();
    rinzu.stdio.writeLine(f"{function}|{future}|{Pending(value: future)}");
    let result = f"prefix:{await future}:suffix";
    let forwarded = spawn worker(Int(utf8ByteLength(result)));
    rinzu.stdio.writeLine(result);
    rinzu.stdio.writeLine(f"{await forwarded}");
}
"#,
    );
    let result = "prefix:child:worker:7:日本語\0🙂:suffix";
    let expected = format!(
        "<function>|<Future<String>>|Pending(value: <Future<String>>)\n{result}\nworker:{}:日本語\0🙂\n",
        result.len()
    );
    expect_both(&root, expected.as_bytes());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn formatting_a_failed_future_does_not_observe_its_failure() {
    let root = package(
        "failed_future",
        r#"
import rinzu.stdio;
async fn failed() -> Int { let invalid = CInt8(999); return 7; }
async fn main() { let future = failed(); let text = f"future:{future}"; rinzu.stdio.writeLine(text); }
"#,
    );
    for command in ["check", "run", "build", "verify-aot"] {
        let rejected = Command::new(frozen_cli::path())
            .arg(command)
            .arg(&root)
            .output()
            .unwrap();
        assert!(!rejected.status.success(), "{command}: {rejected:?}");
        assert!(
            String::from_utf8_lossy(&rejected.stderr).contains("RZ0406"),
            "{command}: {rejected:?}"
        );
        assert!(
            rejected.stdout.is_empty(),
            "invalid source cannot run: {rejected:?}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn many_parts_and_deep_recursive_values_survive_collection() {
    let fields = (0..129)
        .map(|i| format!("const f{i}: Int"))
        .collect::<Vec<_>>()
        .join(", ");
    let values = (0..129)
        .map(|i| format!("f{i}: {i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let parts = (0..129)
        .map(|i| format!("{{{i}}}"))
        .collect::<Vec<_>>()
        .join("|");
    let source = r#"
import rinzu.stdio;
class Wide { FIELDS }
class Node { const children: List<Node> }
MAIN main() {
    rinzu.stdio.writeLine(f"PARTS");
    rinzu.stdio.writeLine(f"wide:{Wide(VALUES)}");
    var node = Node(children: []); var depth = 0;
    while depth < 256 { node = Node(children: [node]); depth = depth + 1; }
    rinzu.stdio.writeLine(f"deep:{node}");
}
"#
    .replace("FIELDS", &fields)
    .replace("VALUES", &values)
    .replace("PARTS", &parts);
    let numbers = (0..129)
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join("|");
    let expected = format!(
        "{numbers}\nwide:Wide({values})\ndeep:{}Node(children: []){}\n",
        "Node(children: [".repeat(256),
        "])".repeat(256)
    );
    for (name, main) in [("deep_sync", "fn"), ("deep_async", "async fn")] {
        let root = package(name, &source.replace("MAIN", main));
        expect_both(&root, expected.as_bytes());
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn sdk_opaque_and_shared_record_values_format_on_sync_and_compiled_async_backends() {
    let root = std::env::temp_dir().join(format!("rinzu-formatted-sdk-{}", std::process::id()));
    opaque_fixture::create(&root);
    let api = root.join("objects/src/api.rinzu");
    let source = std::fs::read_to_string(&api).unwrap();
    std::fs::write(
        &api,
        format!(
            "{source}\n{}",
            r#"
fn rendered() -> String {
    let object = create(42); let holder = Holder(value: object);
    release(object);
    return f"object:{object}|{holder}";
}
export rendered;
"#
        ),
    )
    .unwrap();
    for main in ["fn", "async fn"] {
        std::fs::write(root.join("app/src/main.rinzu"), format!(
            "import objects; import rinzu.stdio; {main} main() {{ rinzu.stdio.writeLine(objects.rendered()); }}"
        )).unwrap();
        expect_sdk(
            &root.join("app"),
            "opaque_app",
            b"object:<objects::Object>|objects::Holder(value: <objects::Object>)\n",
        );
    }
    std::fs::remove_dir_all(&root).unwrap();
    writeback_fixture::create(&root);
    let api = root.join("records/src/api.rinzu");
    let source = std::fs::read_to_string(&api).unwrap();
    std::fs::write(
        &api,
        format!(
            "{source}\n{}",
            r#"
fn displayCallback() -> Int { return 0; }
fn rendered() -> String {
    let output = Packet(x:1,valid:false); let alias = output;
    let before = f"before:{output}";
    let status = change(output,40);
    return f"{before}|after:{alias}|status:{status}|{displayCallback}";
}
export rendered;
"#
        ),
    )
    .unwrap();
    let expected = b"before:records::Packet(x: 1, valid: false)|after:records::Packet(x: 41, valid: true)|status:-7|<function>\n";
    for main in ["fn", "async fn"] {
        std::fs::write(root.join("app/src/main.rinzu"), format!(
            "import records; import rinzu.stdio; {main} main() {{ rinzu.stdio.writeLine(records.rendered()); }}"
        )).unwrap();
        expect_sdk(&root.join("app"), "writeback_app", expected);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn multi_file_formatted_string_sample_runs_and_matches_aot() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/programs/formatted-strings");
    let expected = concat!(
        "Rinzu notebook × 2 = 2400円\n",
        "商品: products::Item(name: \"Rinzu notebook\", unitPrice: 1200, quantity: 2)\n",
        "一覧: [1, 2, 3]\n波括弧: {name}\n入れ子: Rinzu notebook: 2個\n"
    );
    assert_eq!(cli("run", &root).stdout, expected.as_bytes());
    cli("verify-aot", &root);
}
