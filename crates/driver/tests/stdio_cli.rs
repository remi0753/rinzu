#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
#[path = "fixtures/native_opaque_sdk/setup.rs"]
mod opaque_fixture;
#[path = "fixtures/native_record_sdk/setup.rs"]
mod record_fixture;
#[path = "fixtures/native_record_writeback_sdk/setup.rs"]
#[allow(dead_code)]
mod writeback_fixture;

#[path = "fixtures/checked_entry.rs"]
mod checked_entry;
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

fn package(name: &str, source: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("rinzu-stdio-{name}-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("rinzu.toml"),
        "[package]\nname=\"stdio_app\"\nlanguage=\"0.2\"\n",
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

#[test]
fn string_stdout_runs_sync_and_async_jit_and_aot() {
    for (name, main) in [("sync", "fn"), ("async", "async fn")] {
        let root = package(
            name,
            &format!(
                r#"
import rinzu.stdio;
fn identity(text: String) -> String {{ return text; }}
{main} main() {{
    rinzu.stdio.write(identity("日本語\u{{0}}🙂"));
    rinzu.stdio.writeLine("");
    rinzu.stdio.writeLine("tail");
    try {{ let bad = CInt8(300); }} catch error {{ rinzu.stdio.write("caught"); }}
    finally {{ rinzu.stdio.writeLine(" cleanup"); }}
}}
"#
            ),
        );
        let expected = "日本語\0🙂\ntail\ncaught cleanup\n".as_bytes();
        let jit = cli("run", &root);
        assert_eq!(jit.stdout, expected);
        cli("build", &root);
        let aot = Command::new(root.join("build/stdio_app")).output().unwrap();
        assert!(
            aot.status.success(),
            "{}",
            String::from_utf8_lossy(&aot.stderr)
        );
        assert_eq!(aot.stdout, expected);
        let verified = cli("verify-aot", &root);
        assert!(!verified.stdout.starts_with(expected));
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn stdout_errors_enter_catch_and_run_finally() {
    for (name, main) in [("error_sync", "fn"), ("error_async", "async fn")] {
        let root = package(
            name,
            &format!(
                r#"
import rinzu.stdio;
{main} main() {{
    var result = 0;
    try {{ rinzu.stdio.writeLine("unwritable"); }}
    catch [error: Error, ...errorRest] {{ if error.message != "I/O operation failed" {{ panic("bad"); }} result = 1; }}
    finally {{ result = result + 10; }}
    if result != 11 {{ panic("missed"); }}
}}
"#
            ),
        );
        cli("build", &root);
        let sink = root.join("readonly");
        std::fs::write(&sink, b"").unwrap();
        let jit = Command::new(frozen_cli::path())
            .arg("run")
            .arg(&root)
            .stdout(std::fs::File::open(&sink).unwrap())
            .output()
            .unwrap();
        assert!(
            jit.status.success(),
            "{:?}: {}",
            jit.status,
            String::from_utf8_lossy(&jit.stderr)
        );
        let output = Command::new(root.join("build/stdio_app"))
            .stdout(std::fs::File::open(&sink).unwrap())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{:?}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(std::fs::read(sink).unwrap().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn stdout_without_newline_cannot_hide_runtime_reports() {
    let root = package(
        "no_newline",
        "import rinzu.stdio; fn main() { rinzu.stdio.write(\"RINZU_RESULT=spoof\"); }",
    );
    let jit = cli("run", &root);
    assert_eq!(jit.stdout, b"RINZU_RESULT=spoof");
    assert!(String::from_utf8_lossy(&jit.stderr).contains("result: 0"));
    let verified = cli("verify-aot", &root);
    assert!(String::from_utf8_lossy(&verified.stdout).contains("outcome=success"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn test_and_async_verification_keep_execution_reports_off_stdout() {
    let root = package(
        "test_commands",
        "import rinzu.stdio; async fn main() { rinzu.stdio.writeLine(42); }",
    );
    for command in ["test", "verify-async"] {
        let output = cli(command, &root);
        same_stdout(&output.stdout, b"42\n", command);
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(if command == "test" {
                "exit status: 0"
            } else {
                "resources=baseline"
            })
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn multi_file_sample_prints_records_and_computed_totals() {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/programs/multi-file");
    let output = cli("run", &root);
    same_stdout(
        &output.stdout,
        concat!(
            "catalog::Item(name: \"Rinzu notebook\", unitPrice: 1200, quantity: 2)\nTotal: 2640\n",
            "catalog::Item(name: \"Pen\", unitPrice: 200, quantity: 3)\nTotal: 660\n"
        )
        .as_bytes(),
        "multi-file sample",
    );
    cli("verify-aot", &root);
}

fn expect_both(name: &str, source: &str, expected: &[u8]) {
    let root = package(name, source);
    let jit = cli("run", &root);
    same_stdout(&jit.stdout, expected, name);
    cli("build", &root);
    let aot = Command::new(root.join("build/stdio_app")).output().unwrap();
    assert!(
        aot.status.success(),
        "{}",
        String::from_utf8_lossy(&aot.stderr)
    );
    same_stdout(&aot.stdout, expected, name);
    let verified = cli("verify-aot", &root);
    assert!(String::from_utf8_lossy(&verified.stdout).contains("resources=baseline"));
    std::fs::remove_dir_all(root).unwrap();
}

fn same_stdout(actual: &[u8], expected: &[u8], name: &str) {
    let offset = actual
        .iter()
        .zip(expected)
        .position(|(a, b)| a != b)
        .unwrap_or(actual.len().min(expected.len()));
    let start = offset.saturating_sub(16);
    assert!(
        actual == expected,
        "{name}: stdout differs at byte {offset} (length {} / {}): {:?} / {:?}",
        actual.len(),
        expected.len(),
        String::from_utf8_lossy(&actual[start..actual.len().min(offset + 96)]),
        String::from_utf8_lossy(&expected[start..expected.len().min(offset + 96)])
    );
}

#[test]
fn all_scalar_and_aggregate_values_have_identical_stdout_and_single_evaluation() {
    let source = r#"
import rinzu.stdio;
enum Choice { empty, item(value: Int) }
class Packet { const text: String, const data: Bytes(4), const choice: Choice, const values: List<Int>, const entries: Map<String, Int> }
fn unit() {}
fn once() -> Int { rinzu.stdio.writeLine("once"); return 9; }
MAIN main() {
    rinzu.stdio.writeLine(unit());
    rinzu.stdio.writeLine(true); rinzu.stdio.writeLine(false);
    rinzu.stdio.writeLine(CInt8(-128)); rinzu.stdio.writeLine(CInt16(-32768));
    rinzu.stdio.writeLine(CInt32(-2147483648)); rinzu.stdio.writeLine(-9223372036854775808);
    rinzu.stdio.writeLine(9223372036854775807);
    rinzu.stdio.writeLine(CUInt8(255)); rinzu.stdio.writeLine(CUInt16(65535));
    rinzu.stdio.writeLine(CUInt32(4294967295));
    let largest: CUInt64 = 18446744073709551615; rinzu.stdio.writeLine(largest);
    rinzu.stdio.writeLine(CFloat(0.1)); rinzu.stdio.writeLine(0.1);
    rinzu.stdio.writeLine(-0.0); rinzu.stdio.writeLine(1.0);
    rinzu.stdio.writeLine(0.0 / 0.0); rinzu.stdio.writeLine(1.0 / 0.0); rinzu.stdio.writeLine(-1.0 / 0.0);
    rinzu.stdio.writeLine(b""); rinzu.stdio.writeLine(b"\x00\xff\"\\");
    let packet = Packet(text: "日本語\u{0}\n🙂", data: b"\x00\xff\"\\", choice: Choice.item(value: 7), values: [1, -2], entries: {"z": 2, "a": 1});
    rinzu.stdio.writeLine(packet);
    let integers: Map<Int, Int> = {2: 20, 10: 100, 1: 10}; rinzu.stdio.writeLine(integers);
    let nested: Map<String, Map<String, Int>> = {"b": {"x": 2}, "a": {"z": 3, "a": 1}};
    rinzu.stdio.writeLine(nested);
    let empty: List<String> = []; rinzu.stdio.writeLine(empty);
    rinzu.stdio.writeLine(Choice.empty());
    rinzu.stdio.writeLine(once());
}
"#;
    let expected = concat!(
        "()\ntrue\nfalse\n-128\n-32768\n-2147483648\n-9223372036854775808\n9223372036854775807\n",
        "255\n65535\n4294967295\n18446744073709551615\n0.100000001\n0.10000000000000001\n-0.0\n1.0\nNaN\ninf\n-inf\n",
        "b\"\"\nb\"\\x00\\xff\\\"\\\\\"\n",
        "Packet(text: \"日本語\\u{00}\\n🙂\", data: b\"\\x00\\xff\\\"\\\\\", choice: Choice.item(value: 7), values: [1, -2], entries: {\"a\": 1, \"z\": 2})\n",
        "{1: 10, 10: 100, 2: 20}\n{\"a\": {\"a\": 1, \"z\": 3}, \"b\": {\"x\": 2}}\n[]\nChoice.empty\nonce\n9\n"
    );
    for (name, main) in [("values_sync", "fn"), ("values_async", "async fn")] {
        expect_both(name, &source.replace("MAIN", main), expected.as_bytes());
    }
}

#[test]
fn wide_records_and_deep_recursive_values_survive_gc_on_both_backends() {
    let fields = (0..129)
        .map(|i| format!("const f{i}: Int"))
        .collect::<Vec<_>>()
        .join(", ");
    let values = (0..129)
        .map(|i| format!("f{i}: {i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let source = format!(
        r#"
import rinzu.stdio;
class Wide {{ {fields} }}
class Node {{ const children: List<Node> }}
MAIN main() {{
    rinzu.stdio.writeLine(Wide({values}));
    var node = Node(children: []); var depth = 0;
    while depth < 256 {{ node = Node(children: [node]); depth = depth + 1; }}
    rinzu.stdio.writeLine(node);
}}
"#
    );
    let mut expected = format!("Wide({values})\n");
    expected.push_str(&"Node(children: [".repeat(256));
    expected.push_str("Node(children: [])");
    expected.push_str(&"])".repeat(256));
    expected.push('\n');
    for (name, main) in [("deep_sync", "fn"), ("deep_async", "async fn")] {
        expect_both(name, &source.replace("MAIN", main), expected.as_bytes());
    }
}

#[test]
fn error_function_and_future_display_preserve_observation_and_worker_roots() {
    let common = r#"
import rinzu.stdio;
fn target() -> Int { panic("neverCalled"); return 99; }
class Box { const error: Error, const errors: List<Error> }
class Pending { const value: Future<Int> }
fn caught() -> Error { try { let invalid = CInt8(999); } catch [error: Error, ...rest] { return error; } return caught(); }
fn showWorker(text: String) -> Int {
    var index = 0;
    while index < 20 { let garbage = ["temporary", text]; index = index + 1; }
    try {rinzu.stdio.writeLine(text); rinzu.stdio.writeLine(caught()); return 42;}catch unexpectedWorkerErrors{panic("unexpected failure"); return 0;}
}
MAIN main() {
    let callback = target; rinzu.stdio.writeLine(callback);
    let error = caught(); rinzu.stdio.writeLine(error);
    rinzu.stdio.writeLine(Box(error: error, errors: [error]));
    EXTRA
}
"#;
    let expected = concat!(
        "<function>\nError(message: \"numeric conversion is out of range or loses required integer precision\")\n",
        "Box(error: Error(message: \"numeric conversion is out of range or loses required integer precision\"), errors: [Error(message: \"numeric conversion is out of range or loses required integer precision\")])\n"
    );
    expect_both(
        "other_sync",
        &common.replace("MAIN", "fn").replace("EXTRA", ""),
        expected.as_bytes(),
    );
    let extra = r#"
    let future = answer(); let alias = future;
    rinzu.stdio.writeLine(alias);
    rinzu.stdio.writeLine(Pending(value: alias));
    let futures: List<Future<Int>> = [future,alias]; rinzu.stdio.writeLine(futures);
    let result = await future; rinzu.stdio.writeLine(result);
    let worker = spawn showWorker("worker 日本語\u{0}🙂");
    let completed = await worker; rinzu.stdio.writeLine(completed);
"#;
    let source = common
        .replace(
            "import rinzu.stdio;",
            "import rinzu.stdio;\nasync fn answer() -> Int { return 7; }",
        )
        .replace("MAIN", "async fn")
        .replace("EXTRA", extra);
    expect_both("other_async",&source,format!("{expected}<Future<Int>>\nPending(value: <Future<Int>>)\n[<Future<Int>>, <Future<Int>>]\n7\nworker 日本語\0🙂\nError(message: \"numeric conversion is out of range or loses required integer precision\")\n42\n").as_bytes());
}

fn sdk_stdout(app: &Path, binary: &str, expected: &[u8]) {
    let entry = app.join("src/main.rinzu");
    let source = std::fs::read_to_string(&entry).unwrap();
    std::fs::write(entry, checked_entry::source(&source)).unwrap();
    same_stdout(&cli("run", app).stdout, expected, "SDK JIT");
    cli("build", app);
    let aot = Command::new(app.join("build").join(binary))
        .output()
        .unwrap();
    assert!(
        aot.status.success(),
        "{}",
        String::from_utf8_lossy(&aot.stderr)
    );
    same_stdout(&aot.stdout, expected, "SDK AOT");
    cli("verify-aot", app);
}

#[test]
fn displaying_a_future_does_not_observe_its_failure() {
    let root = package(
        "unobserved_future",
        r#"
import rinzu.stdio;
async fn failed() -> Int { let invalid = CInt8(999); return 7; }
async fn main() { let future = failed(); rinzu.stdio.writeLine(future); }
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
fn sdk_opaque_display_use_sync_and_compiled_async_jit() {
    let root = std::env::temp_dir().join(format!("rinzu-stdio-sdk-{}", std::process::id()));
    opaque_fixture::create(&root);
    let api = root.join("objects/src/api.rinzu");
    std::fs::write(
        &api,
        format!(
            "import rinzu.stdio;\n{}\n{}",
            std::fs::read_to_string(&api).unwrap(),
            r#"
fn display() {
    let object = create(42); let holder = Holder(value: object);
    release(object);
    rinzu.stdio.writeLine(object); rinzu.stdio.writeLine(holder);
    try { let missing = absent(); } catch [error:Error,...rest] { rinzu.stdio.writeLine(error); }
}
export display;
"#
        ),
    )
    .unwrap();
    for main in ["fn", "async fn"] {
        std::fs::write(
            root.join("app/src/main.rinzu"),
            format!("import objects; {main} main() {{ objects.display(); }}"),
        )
        .unwrap();
        sdk_stdout(&root.join("app"),"opaque_app",b"<objects::Object>\nobjects::Holder(value: <objects::Object>)\nError(message: \"native value is NULL\")\n");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn sdk_record_callback_display_use_sync_and_compiled_async_jit() {
    let root = std::env::temp_dir().join(format!("rinzu-stdio-record-sdk-{}", std::process::id()));
    record_fixture::create(&root);
    let api = root.join("records/src/api.rinzu");
    std::fs::write(&api,format!("import rinzu.stdio;\n{}\n{}",std::fs::read_to_string(&api).unwrap(),r#"
fn printedCallback(bytes: Bytes(4)) -> CSize fallback 4 { rinzu.stdio.writeLine(bytes); return byteLength(bytes); }
fn display() { let result = withCallback(original(),printedCallback); rinzu.stdio.writeLine(result); }
export display;
"#)).unwrap();
    let packet = "records::Packet(a: -128, b: -32768, c: -2147483648, d: -9223372036854775808, e: 255, f: 65535, g: 4294967295, h: 18446744073709551615, i: -0.0, j: NaN, k: true)\n";
    let expected = format!("{}{packet}", "b\"\\x00\\xff\\x07\\x00\"\n".repeat(32));
    for main in ["fn", "async fn"] {
        std::fs::write(
            root.join("app/src/main.rinzu"),
            format!("import records; {main} main() {{ records.display(); }}"),
        )
        .unwrap();
        sdk_stdout(&root.join("app"), "record_app", expected.as_bytes());
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn sdk_shared_record_display_use_sync_and_compiled_async_jit() {
    let root = std::env::temp_dir().join(format!("rinzu-stdio-shared-sdk-{}", std::process::id()));
    writeback_fixture::create(&root);
    let api = root.join("records/src/api.rinzu");
    std::fs::write(
        &api,
        format!(
            "import rinzu.stdio;\n{}\n{}",
            std::fs::read_to_string(&api).unwrap(),
            r#"
fn display() {
    let output = Packet(x:1,valid:false); let alias = output;
    let snapshot = {x=output.x,valid=output.valid};
    rinzu.stdio.writeLine(output);
    let status = change(output,40);
    rinzu.stdio.writeLine(alias);
    rinzu.stdio.writeLine(snapshot);
}
export display;
"#
        ),
    )
    .unwrap();
    let expected = "records::Packet(x: 1, valid: false)\nrecords::Packet(x: 41, valid: true)\n{valid=false, x=1}\n";
    for main in ["fn", "async fn"] {
        std::fs::write(
            root.join("app/src/main.rinzu"),
            format!("import records; {main} main() {{ records.display(); }}"),
        )
        .unwrap();
        sdk_stdout(&root.join("app"), "writeback_app", expected.as_bytes());
    }
    std::fs::remove_dir_all(root).unwrap();
}
