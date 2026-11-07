#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};

const PACKAGE: &str = "[package]\nname = \"test_app\"\nlanguage = \"0.5\"\n";
const DUPLICATE: &str = "fn main() {\n    let value = 1;\n    let value = 2;\n}\n";
const DUPLICATE_BLOCK: &str = concat!(
    "app/src/main.rinzu:3:5: RZ0201: local `value` shadows an existing name\n",
    "  3 |     let value = 2;\n",
    "    |     ^^^^^^^^^^^^^^\n",
    "  note: app/src/main.rinzu:2:5: existing declaration is here\n",
    "  help: choose a name not used elsewhere in this function\n",
);
const REFRESH: &str = r#"error ReadFailed {}

fn refresh(load: fn () throws(ReadFailed) -> Int) nothrow -> Int {
    try {
        let preview = load();
    }
    catch failures {}
    return load();
}

fn main() {}
"#;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "rinzu-compile-diagnostics-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        Self(root.canonicalize().unwrap())
    }

    fn file(&self, name: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }

    fn app(&self, source: &str) {
        self.file("app/rinzu.toml", PACKAGE);
        self.file("app/src/main.rinzu", source);
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(frozen_cli::path());
        command
            .current_dir(&self.0)
            .args(args)
            .env("RINZU_CACHE_DIR", self.0.join("cache"))
            .env_remove("RINZU_COMPILE_TIMINGS");
        command
    }

    fn cli(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn stdin(&self, args: &[&str], bytes: &[u8]) -> Output {
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(bytes).unwrap();
        child.wait_with_output().unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn rejected(output: Output) -> String {
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    String::from_utf8(output.stderr).unwrap()
}

#[test]
fn compilation_commands_share_one_diagnostic_and_one_summary() {
    let fixture = Fixture::new();
    fixture.app(DUPLICATE);
    let mut commands = vec![
        vec!["check", "app"],
        vec!["run", "app"],
        vec!["run", "app", "--diagnostic-report", "report.json"],
        vec!["build", "app"],
        vec!["verify-aot", "app"],
        vec!["bundle", "app"],
        vec!["api", "app"],
        vec!["lock", "app"],
        vec!["test", "app"],
        vec!["verify-async", "app"],
        vec!["release", "app"],
    ];
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        commands.push(vec!["verify-appkit", "app"]);
    }
    for args in commands {
        let stderr = rejected(fixture.cli(&args));
        assert_eq!(
            stderr,
            format!("{DUPLICATE_BLOCK}rinzu: input failed to check\n"),
            "{args:?}"
        );
    }
    assert!(!fixture.0.join("report.json").exists());
}

#[test]
fn dev_uses_the_same_locations_in_preflight_and_child_run() {
    let fixture = Fixture::new();
    fixture.app(DUPLICATE);
    let stderr = rejected(fixture.cli(&["dev", "app", "--once"]));
    assert!(stderr.contains(DUPLICATE_BLOCK), "{stderr}");
    assert_eq!(stderr.matches("RZ0201:").count(), 1, "{stderr}");
    let stderr =
        rejected(fixture.cli(&["dev", "app", "--restart", "cooperative", "--stdin", "null"]));
    assert!(stderr.starts_with(DUPLICATE_BLOCK), "{stderr}");
    assert_eq!(stderr.matches("RZ0201:").count(), 1, "{stderr}");
    assert!(
        stderr.ends_with("rinzu: initial dev preflight failed\n"),
        "{stderr}"
    );
}

#[test]
fn source_bytes_and_paths_resolve_including_stdin_unicode_and_eof() {
    let fixture = Fixture::new();
    let name = "日本 語.rinzu";
    let line = "fn main() { let title = \"😀\"; @ }";
    let invalid = [
        b"\xef\xbb\xbf// header\r\n".as_slice(),
        line.as_bytes(),
        b"\r\n",
    ]
    .concat();
    fixture.file(name, &invalid);
    for (args, expected_name) in [
        (vec!["check", name], name),
        (vec!["fmt", name], name),
        (vec!["fmt", "-"], "<stdin>"),
    ] {
        let stderr = if expected_name == "<stdin>" {
            rejected(fixture.stdin(&args, &invalid))
        } else {
            rejected(fixture.cli(&args))
        };
        let column = line.split_once('@').unwrap().0.chars().count() + 1;
        assert!(
            stderr.starts_with(&format!("{expected_name}:2:{column}: RZ0002:")),
            "{stderr}"
        );
        assert!(
            stderr.contains(&format!("\n  2 | {line}\n  help: ")),
            "{stderr}"
        );
    }

    let bad_bytes = b"\xef\xbb\xbf// header\r\n// \xf0\x9f\x98\x80\xff\r\n";
    fixture.file(name, bad_bytes);
    for (stderr, expected_name) in [
        (rejected(fixture.cli(&["check", name])), name),
        (rejected(fixture.stdin(&["fmt", "-"], bad_bytes)), "<stdin>"),
    ] {
        assert!(stderr.starts_with(&format!(
            "{expected_name}:2:5: RZ0001: source file is not valid UTF-8\n  help: save the file as UTF-8\n"
        )), "{stderr}");
    }

    let eof = rejected(fixture.stdin(&["fmt", "-"], b"fn main() {\r\n"));
    assert!(eof.starts_with("<stdin>:2:1: RZ0102:"), "{eof}");
    assert!(eof.contains("\n  2 | \n    | ^\n  help: "), "{eof}");
    let outside = Fixture::new();
    let path = outside.file("bad.rinzu", "@");
    let stderr = rejected(fixture.cli(&["check", path.to_str().unwrap()]));
    assert!(
        stderr.starts_with(&format!("{}:1:1: RZ0002:", path.display())),
        "{stderr}"
    );
}

#[test]
fn manifest_and_native_input_diagnostics_name_their_own_input() {
    let fixture = Fixture::new();
    fixture.file("app/rinzu.toml", "[dependencies]\n");
    assert_eq!(
        rejected(fixture.cli(&["check", "app"])),
        concat!(
            "app/rinzu.toml: RZ0602: missing `[package]` section\n",
            "  help: add package name and language under `[package]`\n",
            "rinzu: input failed to check\n",
        )
    );

    fixture.app("fn main() {}");
    fixture.file(
        "app/rinzu.toml",
        format!("{PACKAGE}[dependencies]\nsupport = {{ path = \"../SDK 日本\" }}\n"),
    );
    fixture.file(
        "SDK 日本/rinzu.toml",
        b"\xef\xbb\xbf[package]\r\nname = 123\r\n",
    );
    assert_eq!(
        rejected(fixture.cli(&["check", "app"])),
        concat!(
            "SDK 日本/rinzu.toml:2:8: RZ0602: value for `name` must be a quoted string\n",
            "  2 | name = 123\n",
            "    |        ^^^\n",
            "  help: surround the value with double quotes\n",
            "rinzu: input failed to check\n",
        )
    );

    fixture.app("fn main() {}\nfn test_unit() {}");
    fixture.file("app/bad.txt", "");
    for args in [
        vec!["check", "app", "--native-object", "bad.txt"],
        vec!["test", "app", "--native-object", "bad.txt"],
        vec!["test", "app", "--unit", "--native-object", "bad.txt"],
        vec!["lock", "app", "--native-object", "bad.txt"],
    ] {
        let output = fixture.cli(&args);
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.starts_with(concat!(
            "app/bad.txt: RZ0602: unsupported native object extension\n",
            "  help: pass an existing regular native file or search directory; CLI paths may be absolute\n",
        )), "{args:?}: {stderr}");
        assert_eq!(stderr.matches("RZ0602:").count(), 1);
    }

    fs::create_dir(fixture.0.join("empty")).unwrap();
    assert!(rejected(fixture.cli(&["check", "empty"])).starts_with("empty: RZ0601:"));
    assert!(
        rejected(fixture.cli(&["check", "missing.rinzu"])).starts_with("missing.rinzu: RZ0603:")
    );
}

#[test]
fn import_errors_point_into_the_dependency_and_back_to_the_import() {
    let fixture = Fixture::new();
    fixture.file(
        "entry.rinzu",
        "import \"./別 ファイル.rinzu\" as other;\nfn main() {}",
    );
    fixture.file("別 ファイル.rinzu", "fn exported() {\n    @\n}\n");
    let stderr = rejected(fixture.cli(&["check", "entry.rinzu"]));
    assert!(
        stderr.starts_with("別 ファイル.rinzu:2:5: RZ0002:"),
        "{stderr}"
    );
    assert!(
        stderr.contains(
            "\n  2 |     @\n    |     ^\n  note: entry.rinzu:1:8: imported from here\n  help: "
        ),
        "{stderr}"
    );
}

#[test]
fn unit_and_doc_text_diagnostics_match_the_json_source_locations() {
    let fixture = Fixture::new();
    fixture.app("fn main() {}\nfn test_wrong() {\n    let value: Int = \"no\";\n}");
    fixture.file(
        "app/説明 文.md",
        "# 😀\r\n\r\n```rinzu-test\r\nfn main() {\r\n    let value: Int = \"no\";\r\n}\r\n```\r\n",
    );
    for (flags, path, line) in [
        (vec!["--unit"], "app/src/main.rinzu", 3),
        (vec!["--doc", "説明 文.md"], "app/説明 文.md", 5),
    ] {
        if flags[0] == "--doc" {
            fixture.app("fn main() {}");
        }
        let mut args = vec!["test", "app"];
        args.extend(flags);
        let output = fixture.cli(&args);
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(output.stdout, b"tests: 0 selected, 0 passed, 0 failed\n");
        let stderr = String::from_utf8(output.stderr).unwrap();
        let heading = format!("{path}:{line}:22: RZ0301:");
        assert!(stderr.starts_with(&heading), "{stderr}");
        assert!(stderr.contains(&format!("\n  {line} |     let value: Int = \"no\";\n    |                      ^^^^\n  help: ")), "{stderr}");
        args.extend(["--format", "json"]);
        let output = fixture.cli(&args);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stderr.is_empty(), "{output:?}");
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["schema"], "rinzu.test-report");
        let rows = report["diagnostics"].as_array().unwrap();
        assert_eq!(rows.len(), 1, "{report:#}");
        let d = &rows[0];
        assert_eq!(d["code"], "RZ0301");
        let physical = fixture.0.join(path);
        assert_eq!(d["location"]["path"], json!(physical));
        assert_eq!(d["location"]["line"], line);
        assert_eq!(d["location"]["column"], 22);
        let text = fs::read_to_string(&physical).unwrap();
        let start = text.find("\"no\"").unwrap();
        assert_eq!(d["location"]["startByte"], start);
        assert_eq!(d["location"]["endByte"], start + 4);
    }
}

#[test]
fn rz0414_cli_points_to_the_call_that_reaches_the_boundary() {
    let fixture = Fixture::new();
    fixture.file("a.rinzu", REFRESH);
    let expected = concat!(
        "a.rinzu:8:12: RZ0414: ReadFailed can escape `refresh`, which declares `nothrow`\n",
        "  8 |     return load();\n",
        "    |            ^^^^^^\n",
        "  note: a.rinzu:3:51: `refresh` declares `nothrow` here\n",
        "  help: recover the Error from this call, or use `throws(ReadFailed)` if callers should handle it\n",
        "rinzu: input failed to check\n",
    );
    for command in ["run", "check", "check"] {
        assert_eq!(rejected(fixture.cli(&[command, "a.rinzu"])), expected);
    }
    fixture.file("a.rinzu", REFRESH.replace("return load();", "return 0;"));
    let output = fixture.cli(&["check", "a.rinzu"]);
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
}

#[test]
fn rz0414_callback_lists_the_sdk_parameter_before_the_actual_contract() {
    let fixture = Fixture::new();
    fixture.file(
        "callbacks/rinzu.toml",
        "[package]\nname = \"example.callbacks\"\nkind = \"sdk\"\nlanguage = \"0.5\"\n",
    );
    fixture.file(
        "callbacks/src/callbacks.rinzu",
        r#"error InvalidPort {}

type PortValidator = fn (Int) throws(InvalidPort) -> Int;

fn apply(validate: PortValidator, value: Int) throws(InvalidPort) -> Int {
    return validate(value);
}

export InvalidPort;
export PortValidator;
export apply;
"#,
    );
    fixture.file(
        "app/rinzu.toml",
        format!("{PACKAGE}[dependencies]\nexample.callbacks = {{ path = \"../callbacks\" }}\n"),
    );
    fixture.file(
        "app/src/main.rinzu",
        r#"import example.callbacks as cb;

error ReadFailed {}

fn loadPort(value: Int) throws(ReadFailed) -> Int {
    return value;
}

fn main() {
    try {
        let port = cb.apply(loadPort, 42);
    }
    catch problems {}
}
"#,
    );
    let stderr = rejected(fixture.cli(&["check", "app"]));
    assert_eq!(
        stderr,
        concat!(
            "app/src/main.rinzu:11:29: RZ0414: callback contract `throws(ReadFailed)` exceeds expected `throws(example.callbacks.InvalidPort)`\n",
            "  11 |         let port = cb.apply(loadPort, 42);\n",
            "     |                             ^^^^^^^^\n",
            "  note: callbacks/src/callbacks.rinzu:5:20: parameter `validate` expects `PortValidator`\n",
            "  note: app/src/main.rinzu:5:25: `loadPort` declares `throws(ReadFailed)` here\n",
            "  help: pass a callback whose ordinary Errors fit the expected bound\n",
            "rinzu: input failed to check\n",
        )
    );

    // Even when this argument fits the SDK parameter, the SDK's declared
    // ordinary Error bound applies at the application's synchronous call.
    fixture.file(
        "app/src/main.rinzu",
        concat!(
            "import example.callbacks as cb;\n",
            "fn value(input: Int) nothrow -> Int { return input; }\n",
            "fn boundary() nothrow -> Int {\n",
            "    return cb.apply(value, 42);\n",
            "}\n",
            "fn main() {}\n",
        ),
    );
    assert_eq!(
        rejected(fixture.cli(&["check", "app"])),
        concat!(
            "app/src/main.rinzu:4:12: RZ0414: example.callbacks.InvalidPort can escape `boundary`, which declares `nothrow`\n",
            "  4 |     return cb.apply(value, 42);\n",
            "    |            ^^^^^^^^^^^^^^^^^^^\n",
            "  note: app/src/main.rinzu:3:15: `boundary` declares `nothrow` here\n",
            "  help: recover the Error from this call, or use `throws(example.callbacks.InvalidPort)` if callers should handle it\n",
            "rinzu: input failed to check\n",
        ),
    );
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn native_compile_and_positionless_link_failures_keep_tool_details() {
    let fixture = Fixture::new();
    fixture.file(
        "sdk/rinzu.toml",
        concat!(
            "[package]\nname = \"filter\"\nkind = \"sdk\"\nlanguage = \"0.5\"\n",
            "[native]\nsources = [\"native/filter.c\"]\n",
        ),
    );
    fixture.file(
        "sdk/src/filter.rinzu",
        "native { fn filter() -> Int symbol \"filter\"; } export filter;",
    );
    let source = fixture.file(
        "sdk/native/filter.c",
        "#include <stdint.h>\nint64_t filter(void) {\n  return state;\n}\n",
    );
    let stderr = rejected(fixture.cli(&["check", "sdk"]));
    assert!(
        stderr.starts_with("sdk/native/filter.c: error: native compilation failed (clang)\n"),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!("\n  {}:3:10: error:", source.display())),
        "{stderr}"
    );
    assert!(stderr.contains("undeclared identifier 'state'"), "{stderr}");
    assert!(stderr.ends_with("rinzu: compilation failed\n"), "{stderr}");

    fixture.app("fn main() {}");
    let stderr = rejected(fixture.cli(&["check", "app", "-l", "rinzu_missing_diagnostics_test"]));
    assert!(
        stderr.starts_with("error: native library selection (linking) failed (clang)\n"),
        "{stderr}"
    );
    assert!(stderr.contains("\n  ld:"), "{stderr}");
    assert!(
        stderr.contains("rinzu_missing_diagnostics_test"),
        "{stderr}"
    );
    assert!(stderr.ends_with("rinzu: compilation failed\n"), "{stderr}");
}

#[test]
fn compiler_errors_preserve_their_actual_message_span_and_recovery_hint() {
    let fixture = Fixture::new();
    let cases = [
        (
            "RZ0002",
            "fn main() { @ }",
            "@",
            "invalid token character '@'",
            "remove the character or replace it with a language 0.5 token",
        ),
        (
            "RZ0003",
            "fn main() {} /* comment",
            "/* comment",
            "unterminated block comment",
            "add `*/` before the end of the file",
        ),
        (
            "RZ0004",
            "fn main() { let value = \"unfinished",
            "\"unfinished",
            "unterminated String literal",
            "add a closing double quote on the same line",
        ),
        (
            "RZ0005",
            "fn main() { let value = \"\\q\"; }",
            "\"\\q",
            "invalid literal escape",
            "use one of `\\\\`, `\\\"`, `\\n`, `\\r`, `\\t`, or `\\u{...}`",
        ),
        (
            "RZ0006",
            "fn main() { let value = 9223372036854775808; }",
            "9223372036854775808",
            "integer value 9223372036854775808 is outside Int range [-9223372036854775808, 9223372036854775807]",
            "use a representable value",
        ),
        (
            "RZ0101",
            "fn main() { let value = ; }",
            ";",
            "expected an expression, found `;`",
            "replace this token with an expression",
        ),
        (
            "RZ0202",
            "fn main() { missing(); }",
            "missing()",
            "unknown name `missing`",
            "declare the name or qualify it through an imported module",
        ),
        (
            "RZ0203",
            "fn main() { let value: Missing = 0; }",
            "Missing",
            "unknown type `Missing`",
            "use a built-in type or a record/enum declared in this module",
        ),
        (
            "RZ0301",
            "fn main() { let value: Int = \"no\"; }",
            "\"no\"",
            "expected `Int`, found `String`",
            "change the expression or its declared type so both types match exactly",
        ),
        (
            "RZ0302",
            "fn main() { let value = \"text\" - \"more\"; }",
            "\"text\" - \"more\"",
            "operator is not defined for `String` operands",
            "use an operator defined for two operands of exactly the same supported type",
        ),
        (
            "RZ0303",
            "fn main() { let value = 0; value = 1; }",
            "value = 1;",
            "`value` is immutable",
            "declare it with `var` if reassignment is required",
        ),
        (
            "RZ0304",
            "fn main() { break; }",
            "break;",
            "loop control is only valid inside `while` or `for`",
            "move this statement into a loop body or remove it",
        ),
        (
            "RZ0310",
            "let value: Int = 1 / 0; fn main() {}",
            "1 / 0",
            "integer constant arithmetic traps",
            "use an acyclic module integer constant expression",
        ),
        (
            "RZ0401",
            "async fn main() { await 1; }",
            "1",
            "`await` requires `Future<T>`, found `Int`",
            "await an async call or another Future value",
        ),
        (
            "RZ0402",
            "async fn worker() {} fn main() { let pending = spawn worker(); }",
            "spawn worker()",
            "async function `worker` cannot be a spawn callee",
            "spawn a synchronous WorkerSafe function",
        ),
        (
            "RZ0403",
            "fn main() { try {} finally { return; } }",
            "return;",
            "`return` is not allowed in `finally`",
            "keep `finally` synchronous and allow it to fall through normally",
        ),
        (
            "RZ0404",
            "fn helper() {}",
            "fn helper() {}",
            "entry module has no `main` function",
            "add `fn main() { ... }` or `async fn main() { ... }`",
        ),
        (
            "RZ0405",
            "enum Choice { A, B } fn main() { switch Choice.A() { case Choice.A: {} } }",
            "case Choice.A: {}",
            "switch is missing variants: B",
            "add one case for every missing variant; language 0.1 has no default case",
        ),
        (
            "RZ0406",
            "error E {} async fn work() { throw E(message: \"bad\"); } fn main() { let pending = work(); }",
            "work()",
            "this task can fail after its observation path is lost",
            "await an alias on every managed exit, return the Future, or pass it to a proven observer",
        ),
        (
            "RZ0407",
            "native { fn call(value: Int) -> List<Int> symbol \"call\"; } fn main() {}",
            "List<Int>",
            "type or effect is unsupported at this native ABI position",
            "callbacks must be synchronous and use supported parameter and result types",
        ),
        (
            "RZ0409",
            "class Counter { fn value(self: Counter) -> Int { return 0; } } fn main() { let c = Counter(); let v = c.value; }",
            "c.value",
            "method `Counter.value` needs an explicit call or fn expression",
            "write receiver.value(...) or fn(arguments) { return receiver.value(arguments); }; field access does not call a getter",
        ),
        (
            "RZ0411",
            "fn main() { try {} catch [] {} }",
            "[]",
            "catch [] cannot match a nonempty failure list",
            "match a typed element or bind the whole failure list",
        ),
        (
            "RZ0412",
            "error A {} error B {} fn main() { try { throw A(message: \"a\"); } catch [b: B] {} }",
            "try { throw A(message: \"a\"); } catch [b: B] {}",
            "catch patterns do not cover the possible Error list [A]",
            "cover every possible ordered list, or add a whole-list catch binding",
        ),
        (
            "RZ0413",
            "error E {} fn main() { throw E(message: \"bad\"); }",
            "{ throw E(message: \"bad\"); }",
            "a normal Error can escape the Main entry",
            "catch every normal Error before Main returns, including callees, awaited failures, catch handlers and finally",
        ),
        (
            "RZ0415",
            "interface I { fn run(self: Self) nothrow; } class C interface I {} fn main() {}",
            "class C interface I {}",
            "`C` is missing required method `run`",
            "declare conformance on the class or enum and provide matching public instance methods",
        ),
        (
            "RZ0605",
            "import \"./bad.txt\" as invalid; fn main() {}",
            "\"./bad.txt\"",
            "file import must name a non-empty `.rinzu` path without control characters",
            "use a regular String path such as \"./lib/math.rinzu\"",
        ),
    ];
    for (code, source, span, message, help) in cases {
        fixture.file("source.rinzu", source);
        let stderr = rejected(fixture.cli(&["check", "source.rinzu"]));
        let start = source.rfind(span).unwrap();
        let column = start + 1;
        let caret = "^".repeat(span.len());
        let note = if code == "RZ0406" {
            "  note: source.rinzu:1:67: this exit does not preserve an observation path\n"
        } else {
            ""
        };
        assert_eq!(
            stderr,
            format!(
                "source.rinzu:1:{column}: {code}: {message}\n  1 | {source}\n    | {:start$}{caret}\n{note}  help: {help}\nrinzu: input failed to check\n",
                "",
            ),
            "{code}, {source}"
        );
    }
}

#[test]
fn import_cycle_renders_the_closing_edge_and_the_earlier_edge_once() {
    let fixture = Fixture::new();
    fixture.file(
        "left.rinzu",
        "import \"./right.rinzu\" as right;\nfn main() {}",
    );
    fixture.file("right.rinzu", "import \"./left.rinzu\" as left;");
    let stderr = rejected(fixture.cli(&["check", "left.rinzu"]));
    assert!(
        stderr.starts_with("right.rinzu:1:8: RZ0604: import cycle; import chain:"),
        "{stderr}"
    );
    assert!(
        stderr.contains("\n  1 | import \"./left.rinzu\" as left;\n"),
        "{stderr}"
    );
    assert!(
        stderr.contains("\n  note: left.rinzu:1:8: imported from here\n  help: "),
        "{stderr}"
    );
    assert_eq!(stderr.matches("RZ0604:").count(), 1);
}

#[test]
fn test_worker_resolves_diagnostics_from_its_own_check_in_each_mode() {
    let fixture = Fixture::new();
    fixture.app(&format!("{DUPLICATE}fn test_changed() {{}}\n"));
    for json_mode in [false, true] {
        let scratch = fixture.0.join(format!("worker-{json_mode}"));
        fs::create_dir(&scratch).unwrap();
        let request = json!({
            "root":fixture.0.join("app"),
            "unit":true,
            "documents":[],
            "identity":"unit:main::test_changed",
            "native_flags":[],
            "json":json_mode,
            "diagnostic_cwd":fixture.0,
            "receipts":[]
        });
        let mut child = fixture
            .command(&["__test-worker", scratch.to_str().unwrap()])
            .current_dir(&scratch)
            .env("RINZU_TEST_TMP", &scratch)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(request.to_string().as_bytes())
            .unwrap();
        assert!(rejected(child.wait_with_output().unwrap()).is_empty());
        let result: Value =
            serde_json::from_slice(&fs::read(scratch.join("result.json")).unwrap()).unwrap();
        assert!(result.get("error").is_none(), "{result:#}");
        if json_mode {
            let diagnostics = result["diagnostics"].as_array().unwrap();
            assert_eq!(diagnostics.len(), 1, "{result:#}");
            assert_eq!(diagnostics[0]["code"], "RZ0201");
            let expected_path = fixture.0.join("app/src/main.rinzu");
            assert_eq!(diagnostics[0]["location"]["path"], json!(expected_path));
            assert_eq!(diagnostics[0]["location"]["line"], 3);
            assert_eq!(diagnostics[0]["location"]["column"], 5);
            assert_eq!(diagnostics[0]["related"][0]["location"]["line"], 2);
            assert!(result.get("diagnosticBlocks").is_none(), "{result:#}");
        } else {
            assert_eq!(result["diagnosticBlocks"], json!([DUPLICATE_BLOCK]));
            assert!(result.get("diagnostics").is_none(), "{result:#}");
        }
    }
}

#[test]
fn verification_worker_forwards_a_diagnostic_as_one_complete_block() {
    let fixture = Fixture::new();
    fixture.app(DUPLICATE);
    let request = json!({
        "version":2,
        "input":{
            "path":fixture.0.join("app"),
            "options":{},
            "native":{"search_paths":[],"libraries":[],"objects":[],"headers":[]},
            "mode":"Package",
            "entry":fixture.0.join("app/src/main.rinzu"),
            "inputs":[]
        },
        "executable":fixture.0.join("output")
    });
    fixture.file("request.json", request.to_string());
    let output = fixture.cli(&["__verify-aot-build", "request.json", "response.json"]);
    assert!(output.status.success(), "{output:?}");
    assert!(
        output.stdout.is_empty() && output.stderr.is_empty(),
        "{output:?}"
    );
    let response: Value =
        serde_json::from_slice(&fs::read(fixture.0.join("response.json")).unwrap()).unwrap();
    assert_eq!(response["diagnostic_blocks"], json!([DUPLICATE_BLOCK]));
    assert_eq!(response["result"]["Err"], "input failed to check");
}

#[test]
fn several_lexer_errors_are_written_in_order_with_one_summary() {
    let fixture = Fixture::new();
    let stderr = rejected(fixture.stdin(&["fmt", "-"], b"@\n@\n"));
    let headings: Vec<_> = stderr
        .lines()
        .filter(|line| line.starts_with("<stdin>"))
        .collect();
    assert_eq!(
        headings,
        [
            "<stdin>:1:1: RZ0002: invalid token character '@'",
            "<stdin>:2:1: RZ0002: invalid token character '@'",
        ]
    );
    assert_eq!(stderr.matches("  help: ").count(), 2);
    assert_eq!(stderr.matches("rinzu: input failed to format\n").count(), 1);
    assert!(
        stderr.ends_with("rinzu: input failed to format\n"),
        "{stderr}"
    );
}
