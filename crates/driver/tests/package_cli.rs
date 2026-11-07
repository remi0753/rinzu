#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture(PathBuf);

#[test]
fn exported_callback_contracts_constrain_consumers_and_api_diffs() {
    let f = Fixture::new();
    let source = r#"
        error InvalidPort {}
        error ReadFailed {}
        let defaultValidator: fn (Int) throws(InvalidPort) -> Int = fn (value: Int) nothrow -> Int { return value; };
        fn apply(op: fn (Int) throws(InvalidPort) -> Int, value: Int) throws(InvalidPort) -> Int { return op(value); }
        fn adapt(op: fn (Int) throws(InvalidPort, ReadFailed) -> Int) {}
        export InvalidPort;
        export ReadFailed;
        export defaultValidator;
        export apply;
        export adapt;
    "#;
    std::fs::write(f.path("sdk/src/model.rinzu"), source).unwrap();
    std::fs::write(
        f.path("app/src/main.rinzu"),
        r#"
        import model;
        fn main() {
            let minimum = 0;
            let validator: fn (Int) throws(model.InvalidPort) -> Int = fn (value: Int) throws(model.InvalidPort) -> Int {
                if value < minimum { throw model.InvalidPort(message: "negative"); }
                return value;
            };
            var rejected = false;
            try {
                if model.apply(validator, 42) != 42 { panic("callback"); }
                if model.apply(model.defaultValidator, 42) != 42 { panic("default callback"); }
                let invalidResult = model.apply(validator, -1);
            }
            catch [invalid: model.InvalidPort, ...rest] { rejected = true; }
            if !rejected { panic("missing Error"); }
        }
    "#,
    )
    .unwrap();
    success(f.cli("run", "app", &[]));
    success(f.cli("verify-aot", "app", &[]));

    let baseline = f.path("callbacks.json");
    success(f.cli("api", "sdk", &["--output", baseline.to_str().unwrap()]));
    let api = json(&baseline);
    assert!(
        api["exports"]["model.defaultValidator"]["source_value"]
            .as_str()
            .unwrap()
            .contains("throws(InvalidPort)")
    );
    assert!(
        api["exports"]["model.adapt"]["source_template"]["signature"]
            .as_str()
            .unwrap()
            .contains("throws(")
    );
    let after = f.path("callbacks-after.json");
    let reordered = source.replace(
        "throws(InvalidPort, ReadFailed)",
        "throws(ReadFailed, InvalidPort)",
    );
    std::fs::write(f.path("sdk/src/model.rinzu"), reordered).unwrap();
    success(f.cli(
        "api",
        "sdk",
        &[
            "--output",
            after.to_str().unwrap(),
            "--compare",
            baseline.to_str().unwrap(),
        ],
    ));
    assert_eq!(
        json(&f.path("callbacks-after.report.json"))["breaking_candidates"],
        serde_json::json!([])
    );
    let narrower = source.replace("throws(InvalidPort, ReadFailed) -> Int", "nothrow -> Int");
    std::fs::write(f.path("sdk/src/model.rinzu"), narrower).unwrap();
    success(f.cli(
        "api",
        "sdk",
        &[
            "--output",
            after.to_str().unwrap(),
            "--compare",
            baseline.to_str().unwrap(),
        ],
    ));
    assert!(
        json(&f.path("callbacks-after.report.json"))["breaking_candidates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|change| change.as_str().unwrap().contains("model.adapt"))
    );
}

#[test]
fn interfaces_and_error_contracts_survive_sdk_cache_api_and_aot() {
    let f = Fixture::new();
    let source = "error ReadFailed{}interface Reader{fn read(self:Self) throws(ReadFailed)->String;}class Memory interface Reader{const text:String fn read(self:Memory) nothrow->String{return self.text;}}enum Input interface Reader{Text(text:String),fn read(self:Input) nothrow->String{switch self{case Input.Text(text):{return text;}}}}fn read<R interface Reader>(reader:R) throws(ReadFailed)->String{return reader.read();}export Reader;export ReadFailed;export Memory;export Input;export read;";
    std::fs::write(f.path("sdk/src/model.rinzu"), source).unwrap();
    std::fs::write(f.path("app/src/main.rinzu"),"import model;fn main(){let memory=model.Memory(text:\"ok\");if memory.read()!=\"ok\"{panic(\"direct\");}try{if model.read(memory)!=model.read(model.Input.Text(text:\"ok\")){panic(\"generic\");}}catch errors{panic(\"unexpected\");}}").unwrap();
    for _ in 0..2 {
        success(f.cli("check", "app", &[]));
    }
    success(f.cli("run", "app", &[]));
    success(f.cli("verify-aot", "app", &[]));
    let api_path = f.path("contracts.json");
    let markdown_path = f.path("contracts.md");
    success(f.cli(
        "api",
        "sdk",
        &[
            "--output",
            api_path.to_str().unwrap(),
            "--markdown",
            markdown_path.to_str().unwrap(),
        ],
    ));
    let api = json(&api_path);
    assert!(
        std::fs::read_to_string(&markdown_path)
            .unwrap()
            .contains("Error contract: `nothrow` (declared)")
    );
    assert_eq!(api["version"], 4);
    assert_eq!(
        api["exports"]["model.Reader"]["source_template"]["kind"],
        "interface"
    );
    assert_eq!(
        api["exports"]["model.read"]["source_template"]["interface_constraints"]["R"],
        serde_json::json!(["Reader"])
    );
    assert_eq!(
        api["exports"]["model.Memory.read"]["function"]["error_contract"],
        serde_json::json!({"declared":true,"types":[]})
    );
    std::fs::write(
        f.path("sdk/src/model.rinzu"),
        source.replace(
            "fn read(self:Memory) nothrow->String",
            "fn read(self:Memory) throws(ReadFailed)->String",
        ),
    )
    .unwrap();
    success(f.cli("api", "sdk", &["--compare", api_path.to_str().unwrap()]));
    let report = json(&f.path("sdk/build/package-api-v4.report.json"));
    assert!(
        report["breaking_candidates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str().unwrap().contains("model.Memory.read"))
    );
    std::fs::write(
        f.path("app/src/main.rinzu"),
        "import model;fn main(){let memory=model.Memory(text:\"ok\");let text=memory.read();}",
    )
    .unwrap();
    rejected(f.cli("check", "app", &[]), "RZ0413");
}

#[test]
fn api_v4_hides_private_storage_and_preserves_generic_owner_groups() {
    let f = Fixture::new();
    std::fs::write(f.path("sdk/src/model.rinzu"),"class Internal{value:Int}class Counter{private const hiddenValue:Internal fn create()->Counter{return Counter(hiddenValue:Internal(value:3));}fn read(self:Counter)->Int{return self.hiddenValue.value;}}class Box<T>{private const item:T fn create(value:T)->Box<T>{return Box<T>(item:value);}fn get(self:Box<T>)->T{return self.item;}}export Counter;export Box;").unwrap();
    let snapshot = f.path("privacy-api.json");
    let markdown = f.path("privacy-api.md");
    success(f.cli(
        "api",
        "sdk",
        &[
            "--output",
            snapshot.to_str().unwrap(),
            "--markdown",
            markdown.to_str().unwrap(),
        ],
    ));
    let api = json(&snapshot);
    assert_eq!(api["version"], 4);
    assert_eq!(
        api["exports"]["model.Counter"]["constructor_visibility"],
        "owner"
    );
    assert_eq!(
        api["exports"]["model.Counter"]["source_type"]["private_fields"],
        serde_json::json!([0])
    );
    assert!(
        api["exports"]["model.Counter"]["private_graph"]
            .as_str()
            .is_some_and(|s| s.len() == 64)
    );
    let serialized = std::fs::read_to_string(&snapshot).unwrap();
    assert!(
        !serialized.contains("hiddenValue") && !serialized.contains("Internal"),
        "{serialized}"
    );
    let text = std::fs::read_to_string(&markdown).unwrap();
    assert!(!text.contains("hiddenValue"));
    assert_eq!(
        api["exports"]["model.Box.get"]["source_template"]["owner_type_parameters"],
        serde_json::json!([["T", []]])
    );
    assert_eq!(
        api["exports"]["model.Box.get"]["source_template"]["type_parameters"],
        serde_json::json!([])
    );
}

#[test]
fn api_reference_distinguishes_native_data_and_class_methods() {
    let f = Fixture::new();
    let source = "native{opaque Handle;record Packet{value:Int}}class Owner{const handle:Handle,const packet:Packet fn read(self:Owner)->Int{return self.packet.value;}}\n/// First context declaration.\ntype Context={owner:Owner};\n/// Second context declaration.\ntype Session=Context;export Handle;export Packet;export Owner;export Context;export Session;";
    std::fs::write(f.path("sdk/src/model.rinzu"), source).unwrap();
    let output = f.path("native-api.json");
    let markdown = f.path("native-api.md");
    success(f.cli(
        "api",
        "sdk",
        &[
            "--output",
            output.to_str().unwrap(),
            "--markdown",
            markdown.to_str().unwrap(),
        ],
    ));
    let api = json(&output);
    for name in [
        "Handle",
        "Packet",
        "Owner",
        "Context",
        "Session",
        "Owner.read",
    ] {
        assert!(
            api["exports"].get(format!("model.{name}")).is_some(),
            "{api}"
        );
    }
    assert!(api["exports"].get("model.Handle.read").is_none());
    assert!(api["exports"].get("model.Packet.read").is_none());
    assert_eq!(
        api["exports"]["model.Context"]["identity"],
        api["exports"]["model.Session"]["identity"]
    );
    let text = std::fs::read_to_string(markdown).unwrap();
    for role in [
        "opaque ·",
        "native-record ·",
        "class ·",
        "type-alias ·",
        "opaque Handle;",
    ] {
        assert!(text.contains(role), "{role}: {text}");
    }
    assert!(
        text.contains("First context declaration.") && text.contains("Second context declaration."),
        "{text}"
    );
    assert!(
        text.contains("type Context={owner:Owner};") && text.contains("type Session=Context;"),
        "{text}"
    );
    for invalid in [
        "native{userdata Context;}",
        "fn Handle.read()->Int{return 1;}",
        "type Alias=Packet;fn Alias.read()->Int{return 1;}",
    ] {
        std::fs::write(f.path("sdk/src/model.rinzu"), format!("{source}{invalid}")).unwrap();
        let rejected_output = f.path("rejected-api.json");
        let result = f.cli(
            "api",
            "sdk",
            &["--output", rejected_output.to_str().unwrap()],
        );
        assert!(!result.status.success(), "{invalid}");
        assert!(!rejected_output.exists());
    }
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "rinzu-package-v2-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("sdk/src")).unwrap();
        std::fs::create_dir_all(root.join("app/src")).unwrap();
        std::fs::write(root.join("sdk/rinzu.toml"), "[package]\nname=\"model\"\nlanguage=\"0.5\"\nkind=\"sdk\"\nentry=\"src/model.rinzu\"\n").unwrap();
        std::fs::write(root.join("sdk/src/model.rinzu"), "class Box<T:Equality>{const value:T}enum Choice<T>{Empty,Value(value:T)}fn id<T>(value:T)->T{return value;}fn privateValue()->Int{return 1;}fn answer()->Int{return privateValue();}export Box;export Choice;export id;export answer;let ENABLED=true;export ENABLED;").unwrap();
        std::fs::write(root.join("app/rinzu.toml"), "[package]\nname=\"consumer\"\nlanguage=\"0.5\"\n[dependencies]\nmodel={path=\"../sdk\"}\n").unwrap();
        std::fs::write(root.join("app/src/main.rinzu"), "import model;fn main(){let n=model.id<Int>(42);let b=model.Box<Int>(value:n);let c=model.Choice<String>.Value(value:\"ok\");if b.value!=42 || model.answer()!=1{panic(\"failed\");}}").unwrap();
        Self(root)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn cli(&self, command: &str, package: &str, flags: &[&str]) -> Output {
        Command::new(frozen_cli::path())
            .arg(command)
            .env("RINZU_CACHE_DIR", self.path("user-cache"))
            .arg(self.path(package))
            .args(flags)
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn success(output: Output) {
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
fn rejected(output: Output, message: &str) {
    assert!(!output.status.success(), "unexpected success");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(message),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn json(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn owner_exports_private_methods_reexports_docs_and_jit_aot_share_one_api() {
    let f = Fixture::new();
    let sdk_source = r#"
native { fn echo(value:CInt)->CInt symbol "visibility_echo"; }
class Counter {
    value: Int
    /// SECRET_PRIVATE_METHOD_DOCUMENTATION
    private fn raw(self: Counter) -> Int { return self.value; }
    /// Public read operation.
    fn read(self: Counter) -> Int { return self.raw(); }
    fn reader(self: Counter) -> fn() -> Int {
        return fn() -> Int { return self.raw(); };
    }
    async fn next(self: Counter) -> Int { return self.raw(); }
    fn nativeRead(self: Counter) -> Int { return Int(echo(CInt(self.raw()))); }
}
export Counter;
"#;
    std::fs::write(f.path("sdk/src/model.rinzu"), sdk_source).unwrap();
    let manifest = f.path("sdk/rinzu.toml");
    std::fs::write(
        &manifest,
        format!(
            "{}\n[native]\nsources=[\"native.c\"]\nheaders=[\"native.h\"]\n",
            std::fs::read_to_string(&manifest).unwrap()
        ),
    )
    .unwrap();
    std::fs::write(
        f.path("sdk/native.h"),
        "#include <stdint.h>\nint32_t visibility_echo(int32_t value);\n",
    )
    .unwrap();
    std::fs::write(
        f.path("sdk/native.c"),
        "#include \"native.h\"\nint32_t visibility_echo(int32_t value){return value;}\n",
    )
    .unwrap();
    std::fs::create_dir_all(f.path("facade/src")).unwrap();
    std::fs::write(f.path("facade/rinzu.toml"), "[package]\nname=\"facade\"\nkind=\"sdk\"\nlanguage=\"0.5\"\n[dependencies]\nmodel={path=\"../sdk\"}\n").unwrap();
    std::fs::write(
        f.path("facade/src/facade.rinzu"),
        "import model;export model.Counter;",
    )
    .unwrap();
    std::fs::write(f.path("app/rinzu.toml"), "[package]\nname=\"consumer\"\nlanguage=\"0.5\"\n[dependencies]\nfacade={path=\"../facade\"}\n").unwrap();
    std::fs::write(
        f.path("app/src/main.rinzu"),
        r#"
import facade;
async fn main(){
    try {
        let counter=facade.Counter(value:1);
        counter.value=2;
        let unbound=facade.Counter.read;
        let closure=counter.reader();
        let next=await counter.next();
        if counter.read()!=2 || unbound(counter)!=2 || closure()!=2 || next!=2 || counter.nativeRead()!=2 {
            panic("private dispatch mismatch");
        }
    } catch errors { panic("unexpected failure"); }
}
"#,
    )
    .unwrap();
    let output = f.cli("run", "app", &[]);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("compiled JIT continuation runtime"),
        "{output:?}"
    );
    success(output);
    success(f.cli("verify-aot", "app", &[]));
    success(f.cli("build", "app", &[]));
    let output = Command::new(f.path("app/build/consumer")).output().unwrap();
    assert!(output.stdout.is_empty(), "{output:?}");
    success(output);
    let api = f.path("api.json");
    let markdown = f.path("api.md");
    success(f.cli(
        "api",
        "facade",
        &[
            "--output",
            api.to_str().unwrap(),
            "--markdown",
            markdown.to_str().unwrap(),
        ],
    ));
    let api = json(&api);
    assert_eq!(
        api["exports"]["facade.Counter.read"]["identity"],
        "model::Counter.read"
    );
    assert!(api["exports"].get("facade.Counter.raw").is_none());
    let markdown = std::fs::read_to_string(markdown).unwrap();
    assert!(markdown.contains("facade.Counter.read"));
    assert!(markdown.contains("Public read operation."));
    assert!(!markdown.contains("SECRET_PRIVATE_METHOD_DOCUMENTATION"));
    std::fs::write(
        f.path("facade/src/facade.rinzu"),
        "import model;export model.Counter;export model.Counter.read;",
    )
    .unwrap();
    rejected(
        f.cli("check", "facade", &[]),
        "cannot be exported individually",
    );
    std::fs::write(
        f.path("sdk/src/model.rinzu"),
        format!("{sdk_source}fn outside(counter:Counter)->Int{{return counter.raw();}}"),
    )
    .unwrap();
    rejected(f.cli("check", "sdk", &[]), "private method");
}

#[test]
fn markdown_reference_filters_private_exports_and_runs_captured_doc_examples() {
    let f = Fixture::new();
    let source = "\u{feff}/// Public box 日本語🙂\r\nclass Box<T: Equality> {\r\n/// Kept value.\r\nconst value: T\r\n}\r\n/// Identity helper.\r\n/// ```rinzu-test\r\n/// import model;\r\n/// fn main() { if model.id<Int>(42) != 42 { panic(\"failed\"); } }\r\n/// ```\r\nfn id<T>(value: T) -> T { return value; }\r\n/// SECRET_PRIVATE_DOCUMENTATION\r\nfn secret() -> Int { return 7; }\r\nexport Box; export id;\r\n";
    std::fs::write(f.path("sdk/src/model.rinzu"), source).unwrap();
    let snapshot = f.path("before.json");
    let markdown = f.path("reference.md");
    success(f.cli(
        "api",
        "sdk",
        &[
            "--output",
            snapshot.to_str().unwrap(),
            "--markdown",
            markdown.to_str().unwrap(),
        ],
    ));
    let first = std::fs::read_to_string(&markdown).unwrap();
    assert!(first.ends_with('\n') && !first.ends_with("\n\n"));
    assert!(first.contains("Public box 日本語🙂"));
    assert!(first.contains("const value: T"));
    assert!(first.contains("Kept value."));
    assert!(!first.contains("SECRET_PRIVATE_DOCUMENTATION"));
    assert!(first.contains("sdk/src/model.rinzu#L2"));
    assert_eq!(json(&snapshot)["version"], 4);
    success(f.cli(
        "test",
        "sdk",
        &["--doc", markdown.to_str().unwrap(), "--format", "json"],
    ));
    // Comment-only edits change rendered prose, not the API v2 shape.
    std::fs::write(
        f.path("sdk/src/model.rinzu"),
        source.replace("Identity helper.", "Updated identity helper."),
    )
    .unwrap();
    let after = f.path("after.json");
    success(f.cli(
        "api",
        "sdk",
        &[
            "--output",
            after.to_str().unwrap(),
            "--compare",
            snapshot.to_str().unwrap(),
            "--markdown",
            markdown.to_str().unwrap(),
        ],
    ));
    assert_eq!(
        json(&f.path("after.report.json"))["breaking_candidates"],
        serde_json::json!([])
    );
    assert!(
        std::fs::read_to_string(&markdown)
            .unwrap()
            .contains("Updated identity helper.")
    );
    assert_eq!(json(&snapshot), json(&after));
}

#[test]
fn markdown_reexports_keep_origin_and_reject_colliding_or_invalid_outputs() {
    let f = Fixture::new();
    std::fs::write(
        f.path("sdk/src/model.rinzu"),
        "/// Origin documentation.\nfn answer()->Int{return 42;}export answer;\n",
    )
    .unwrap();
    std::fs::create_dir_all(f.path("facade/src")).unwrap();
    std::fs::write(f.path("facade/rinzu.toml"), "[package]\nname=\"facade\"\nkind=\"sdk\"\nlanguage=\"0.5\"\n[dependencies]\nmodel={path=\"../sdk\"}\n").unwrap();
    std::fs::write(
        f.path("facade/src/facade.rinzu"),
        "import model;export model.answer;\n",
    )
    .unwrap();
    let output = f.path("reference.json");
    let markdown = f.path("reference.md");
    success(f.cli(
        "api",
        "facade",
        &[
            "--output",
            output.to_str().unwrap(),
            "--markdown",
            markdown.to_str().unwrap(),
        ],
    ));
    let text = std::fs::read_to_string(&markdown).unwrap();
    assert!(text.contains("facade.answer"));
    assert!(text.contains("Origin documentation."));
    assert!(text.contains("sdk/src/model.rinzu#L2"));
    let original = std::fs::read(&output).unwrap();
    rejected(
        f.cli(
            "api",
            "facade",
            &[
                "--output",
                output.to_str().unwrap(),
                "--markdown",
                output.to_str().unwrap(),
            ],
        ),
        "outputs must differ",
    );
    assert_eq!(std::fs::read(&output).unwrap(), original);
    std::fs::create_dir_all(f.path("nested")).unwrap();
    let alias = f.path("nested/../reference.json");
    rejected(
        f.cli(
            "api",
            "facade",
            &[
                "--output",
                output.to_str().unwrap(),
                "--markdown",
                alias.to_str().unwrap(),
            ],
        ),
        "outputs must differ",
    );
    std::os::unix::fs::symlink(&f.0, f.path("linked")).unwrap();
    let alias = f.path("linked/reference.json");
    rejected(
        f.cli(
            "api",
            "facade",
            &[
                "--output",
                output.to_str().unwrap(),
                "--markdown",
                alias.to_str().unwrap(),
            ],
        ),
        "outputs must differ",
    );
    let report = output.with_extension("report.json");
    rejected(
        f.cli(
            "api",
            "facade",
            &[
                "--output",
                output.to_str().unwrap(),
                "--compare",
                output.to_str().unwrap(),
                "--markdown",
                report.to_str().unwrap(),
            ],
        ),
        "outputs must differ",
    );
    assert_eq!(std::fs::read(&output).unwrap(), original);
    let alias = f.path("linked/reference.md");
    success(f.cli(
        "api",
        "facade",
        &[
            "--output",
            output.to_str().unwrap(),
            "--markdown",
            alias.to_str().unwrap(),
        ],
    ));
    assert!(
        std::fs::read_to_string(&markdown)
            .unwrap()
            .contains("[source](sdk/src/model.rinzu#L2)")
    );
    rejected(
        f.cli("api", "facade", &["--markdown"]),
        "--markdown requires",
    );
    rejected(
        f.cli(
            "api",
            "facade",
            &[
                "--markdown",
                markdown.to_str().unwrap(),
                "--markdown",
                markdown.to_str().unwrap(),
            ],
        ),
        "--markdown",
    );
    std::fs::write(f.path("app/src/main.rinzu"), "fn main(){}\n").unwrap();
    rejected(
        f.cli("api", "app", &["--markdown", markdown.to_str().unwrap()]),
        "requires a source SDK",
    );
}

#[test]
fn public_templates_lock_refresh_api_report_and_all_backends() {
    let f = Fixture::new();
    success(f.cli("lock", "app", &[]));
    let lock_path = f.path("app/rinzu.lock");
    let lock = std::fs::read(&lock_path).unwrap();
    assert_eq!(json(&lock_path)["packages"]["model"]["path"], "../sdk");
    assert!(
        json(&lock_path)["context"]["target.os-build"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
    success(f.cli("lock", "app", &["--check"]));
    assert_eq!(lock, std::fs::read(&lock_path).unwrap());
    success(f.cli("check", "app", &[]));
    success(f.cli("run", "app", &[]));
    success(f.cli("verify-aot", "app", &[]));
    success(f.cli("build", "app", &[]));
    success(Command::new(f.path("app/build/consumer")).output().unwrap());
    // Application bodies are outside the dependency lock.
    let main = f.path("app/src/main.rinzu");
    let body = std::fs::read_to_string(&main).unwrap();
    std::fs::write(main, format!("{body}\n// application-only change\n")).unwrap();
    success(f.cli("lock", "app", &["--check"]));
    success(f.cli("api", "sdk", &[]));
    let api = f.path("sdk/build/package-api-v4.json");
    let baseline = f.path("old-api.json");
    std::fs::copy(&api, &baseline).unwrap();
    let value = json(&api);
    assert_eq!(
        value["exports"]["model.id"]["source_template"]["kind"],
        "function"
    );
    assert_eq!(
        value["exports"]["model.Box"]["source_template"]["type_parameters"][0][1][0],
        "Equality"
    );
    assert!(value["exports"].get("model.privateValue").is_none());
    assert_eq!(value["exports"]["model.ENABLED"]["source_value"], "Bool");
    let source = f.path("sdk/src/model.rinzu");
    let original = std::fs::read_to_string(&source).unwrap();
    std::fs::write(
        &source,
        format!("{original}\n// private source changes lock, not API\n"),
    )
    .unwrap();
    for command in ["check", "run", "build"] {
        rejected(f.cli(command, "app", &[]), "package lock mismatch");
    }
    rejected(f.cli("lock", "app", &["--check"]), "package lock mismatch");
    assert_eq!(lock, std::fs::read(&lock_path).unwrap());
    success(f.cli("api", "sdk", &["--compare", baseline.to_str().unwrap()]));
    let report = f.path("sdk/build/package-api-v4.report.json");
    assert_eq!(json(&report)["breaking_candidates"], serde_json::json!([]));
    assert_eq!(json(&report)["behavior_equivalence"], false);
    std::fs::write(
        &source,
        format!("{original}\nfn extra()->Int{{return 2;}}export extra;\n"),
    )
    .unwrap();
    success(f.cli("api", "sdk", &["--compare", baseline.to_str().unwrap()]));
    assert_eq!(
        json(&report)["additions"],
        serde_json::json!(["model.extra"])
    );
    std::fs::write(
        &source,
        original.replace(
            "fn answer()->Int{return privateValue();}",
            "fn answer()->CInt32{return 1;}",
        ),
    )
    .unwrap();
    success(f.cli("api", "sdk", &["--compare", baseline.to_str().unwrap()]));
    assert!(
        json(&report)["breaking_candidates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s.as_str().unwrap().contains("model.answer"))
    );
    // Explicit refresh works even when the existing lock is stale.
    success(f.cli("lock", "app", &[]));
    success(f.cli("lock", "app", &["--check"]));
    success(f.cli("lock", "sdk", &[]));
    assert_eq!(
        json(&f.path("sdk/rinzu.lock"))["packages"]["model"]["path"],
        "."
    );
    success(f.cli("check", "sdk", &[]));
    success(f.cli("build", "sdk", &[]));
}

#[test]
fn diamond_reexports_file_moves_deletion_and_cycles_recheck() {
    let f = Fixture::new();
    for name in ["left", "right"] {
        std::fs::create_dir_all(f.path(&format!("{name}/src"))).unwrap();
        std::fs::write(f.path(&format!("{name}/rinzu.toml")), format!("[package]\nname=\"{name}\"\nlanguage=\"0.5\"\nkind=\"sdk\"\n[dependencies]\nmodel={{path=\"../sdk\"}}\n")).unwrap();
        std::fs::write(
            f.path(&format!("{name}/src/{name}.rinzu")),
            "import model;export model.Box;export model.id;",
        )
        .unwrap();
    }
    std::fs::write(f.path("app/rinzu.toml"), "[package]\nname=\"consumer\"\nlanguage=\"0.5\"\n[dependencies]\nleft={path=\"../left\"}\nright={path=\"../right\"}\n").unwrap();
    std::fs::write(f.path("app/src/main.rinzu"), "import left;import right;fn main(){let a=left.Box<Int>(value:left.id<Int>(42));let b=right.Box<Int>(value:42);if a!=b{panic(\"wrongIdentity\");}}").unwrap();
    success(f.cli("lock", "app", &[]));
    assert_eq!(
        json(&f.path("app/rinzu.lock"))["packages"]
            .as_object()
            .unwrap()
            .len(),
        3
    );
    success(f.cli("run", "app", &[]));
    success(f.cli("api", "left", &[]));
    assert_eq!(
        json(&f.path("left/build/package-api-v4.json"))["exports"]["left.Box"]["identity"],
        "model::Box"
    );
    success(f.cli("api", "sdk", &[]));
    let baseline = f.path("before-move.json");
    std::fs::copy(f.path("sdk/build/package-api-v4.json"), &baseline).unwrap();
    std::fs::rename(f.path("sdk/src/model.rinzu"), f.path("sdk/src/moved.rinzu")).unwrap();
    let manifest = f.path("sdk/rinzu.toml");
    std::fs::write(
        &manifest,
        std::fs::read_to_string(&manifest)
            .unwrap()
            .replace("src/model.rinzu", "src/moved.rinzu"),
    )
    .unwrap();
    rejected(f.cli("lock", "app", &["--check"]), "package lock mismatch");
    success(f.cli("api", "sdk", &["--compare", baseline.to_str().unwrap()]));
    assert_eq!(
        json(&f.path("sdk/build/package-api-v4.report.json"))["breaking_candidates"],
        serde_json::json!([])
    );
    let source = f.path("sdk/src/moved.rinzu");
    std::fs::write(
        &source,
        std::fs::read_to_string(&source)
            .unwrap()
            .replace("export answer;", ""),
    )
    .unwrap();
    success(f.cli("api", "sdk", &["--compare", baseline.to_str().unwrap()]));
    assert!(
        json(&f.path("sdk/build/package-api-v4.report.json"))["breaking_candidates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str().unwrap().contains("removed model.answer"))
    );
    success(f.cli("lock", "app", &[]));
    success(f.cli("run", "app", &[]));
    std::fs::write(f.path("left/rinzu.toml"), "[package]\nname=\"left\"\nlanguage=\"0.5\"\nkind=\"sdk\"\n[dependencies]\nright={path=\"../right\"}\n").unwrap();
    std::fs::write(f.path("right/rinzu.toml"), "[package]\nname=\"right\"\nlanguage=\"0.5\"\nkind=\"sdk\"\n[dependencies]\nleft={path=\"../left\"}\n").unwrap();
    rejected(f.cli("check", "app", &[]), "cycle");
}

#[test]
fn actual_header_source_archive_and_corrupt_native_cache_are_revalidated() {
    let f = Fixture::new();
    for directory in ["sdk/native", "sdk/lib"] {
        std::fs::create_dir_all(f.path(directory)).unwrap();
    }
    std::fs::write(f.path("sdk/rinzu.toml"), "[package]\nname=\"model\"\nlanguage=\"0.5\"\nkind=\"sdk\"\n[native]\nsources=[\"native/api.c\"]\nheaders=[\"native/api.h\"]\ninclude-dirs=[\"native\"]\nsearch-paths=[\"lib\"]\nlibraries=[\"fixture\"]\n").unwrap();
    std::fs::write(
        f.path("sdk/src/model.rinzu"),
        "native{fn raw()->Int symbol \"answer\";}fn answer()->Int{return raw();}export answer;",
    )
    .unwrap();
    std::fs::write(f.path("sdk/native/api.h"), "#include <stdint.h>\n#define FACTOR 1\nint64_t answer(void);\nint64_t linked_value(void);\n").unwrap();
    let native = f.path("sdk/native/api.c");
    std::fs::write(
        &native,
        "#include \"api.h\"\nint64_t answer(void){return linked_value()*FACTOR;}\n",
    )
    .unwrap();
    std::fs::write(
        f.path("app/src/main.rinzu"),
        "import model;import rinzu.stdio;fn main(){try{rinzu.stdio.writeLine(model.answer());}catch errors{panic(\"failed\");}}",
    )
    .unwrap();
    let archive = |value: i32| {
        let source = f.path("sdk/lib/value.c");
        let object = f.path("sdk/lib/value.o");
        std::fs::write(
            &source,
            format!("#include <stdint.h>\nint64_t linked_value(void){{return {value};}}\n"),
        )
        .unwrap();
        success(
            Command::new("xcrun")
                .args([
                    "--sdk",
                    "macosx",
                    "clang",
                    "-target",
                    "arm64-apple-macos11",
                    "-c",
                ])
                .arg(&source)
                .arg("-o")
                .arg(&object)
                .output()
                .unwrap(),
        );
        success(
            Command::new("ar")
                .arg("rcs")
                .arg(f.path("sdk/lib/libfixture.a"))
                .arg(object)
                .output()
                .unwrap(),
        );
    };
    archive(1);
    success(f.cli("lock", "app", &[]));
    let output = f.cli("run", "app", &[]);
    assert!(output.stdout.starts_with(b"1\n"));
    success(output);
    let caches = std::fs::read_dir(f.path("user-cache/source-v4"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect::<Vec<_>>();
    assert!(!caches.is_empty());
    for directory in &caches {
        std::fs::write(directory.join("facade.dylib"), b"invalid native image").unwrap();
        std::fs::write(directory.join("receipt.json"), b"{broken").unwrap();
    }
    let output = f.cli("run", "app", &[]);
    assert!(output.stdout.starts_with(b"1\n"));
    success(output);
    for directory in &caches {
        assert_eq!(
            json(&directory.join("receipt.json"))["schema"],
            "rinzu.source-cache-v4"
        );
    }
    let header = f.path("sdk/native/api.h");
    let original = std::fs::read_to_string(&header).unwrap();
    std::fs::write(&header, original.replace("FACTOR 1", "FACTOR 2")).unwrap();
    rejected(f.cli("lock", "app", &["--check"]), "package lock mismatch");
    std::fs::write(&header, original.replace("int64_t answer", "double answer")).unwrap();
    assert!(!f.cli("check", "app", &[]).status.success());
    std::fs::write(&header, &original).unwrap();
    let original_source = std::fs::read_to_string(&native).unwrap();
    std::fs::write(&native, original_source.replace("*FACTOR", "*FACTOR+1")).unwrap();
    rejected(f.cli("lock", "app", &["--check"]), "package lock mismatch");
    std::fs::write(&native, original_source).unwrap();
    archive(3);
    rejected(f.cli("lock", "app", &["--check"]), "package lock mismatch");
    success(f.cli("lock", "app", &[]));
    let output = f.cli("run", "app", &[]);
    assert!(output.stdout.starts_with(b"3\n"));
    success(output);
    success(f.cli("verify-aot", "app", &[]));
}

#[test]
fn native_data_lock_requires_explicit_refresh_and_regenerates_v26_bundles() {
    let f = Fixture::new();
    std::fs::write(f.path("sdk/rinzu.toml"), "[package]\nname=\"model\"\nlanguage=\"0.5\"\nkind=\"sdk\"\nentry=\"src/model.rinzu\"\n[native]\nheaders=[\"api.h\"]\nsources=[\"api.c\"]\n").unwrap();
    std::fs::write(
        f.path("sdk/src/model.rinzu"),
        r#"
        native { opaque Handle; record Packet {x:Int,valid:CBool}
            fn update(value:Packet)->Int symbol "update";
            fn context(value:Context)->Context symbol "context";
        }
        class Owner {value:Int fn read(self:Owner)->Int{return self.value;}}
        type Context={owner:Owner};
        export Handle;export Packet;export Owner;export Context;export update;export context;
    "#,
    )
    .unwrap();
    std::fs::write(f.path("sdk/api.h"), "#pragma once\n#include <stdint.h>\ntypedef struct Packet {int64_t x;_Bool valid;} Packet;\nint64_t update(Packet *value);\nvoid *context(void *value);\n").unwrap();
    std::fs::write(f.path("sdk/api.c"), "#include \"api.h\"\nint64_t update(Packet *value){value->x++;return 0;}void *context(void *value){return value;}\n").unwrap();
    std::fs::write(
        f.path("app/src/main.rinzu"),
        r#"
        import model;
        fn main(){try {
            let packet=model.Packet(x:1,valid:true);let alias=packet;
            let status=model.update(packet);
            let value:model.Context={owner=model.Owner(value:40)};
            let returned=model.context(value);
            if alias.x!=2 || returned.owner.read()!=40 {panic("wrong native data");}
        }catch[error:Error,...rest]{panic(error.message);}}
    "#,
    )
    .unwrap();
    success(f.cli("lock", "app", &[]));
    let lock_path = f.path("app/rinzu.lock");
    let current = json(&lock_path);
    let mut old = current.clone();
    old["context"]["rinzu.private-abi"] = "rinzu-baseline-0.5-r171".into();
    old["packages"]["model"]["build_inputs"]["rinzu.native-data-contract"] = "0".into();
    let old_bytes = serde_json::to_vec_pretty(&old).unwrap();
    std::fs::write(&lock_path, &old_bytes).unwrap();
    for command in ["check", "run", "build"] {
        rejected(f.cli(command, "app", &[]), "package lock mismatch");
        assert_eq!(std::fs::read(&lock_path).unwrap(), old_bytes);
    }
    rejected(f.cli("lock", "app", &["--check"]), "package lock mismatch");
    success(f.cli("lock", "app", &[]));
    assert_eq!(json(&lock_path), current);
    success(f.cli("lock", "app", &["--check"]));
    success(f.cli("run", "app", &[]));
    success(f.cli("build", "app", &[]));
    success(Command::new(f.path("app/build/consumer")).output().unwrap());
    let bundle = f.path("app/build/consumer.source-packages");
    let manifest = json(&bundle.join("manifest.json"));
    let descriptor = json(&bundle.join(manifest["packages"][0]["descriptor"].as_str().unwrap()));
    assert_eq!(descriptor["minor"], 6);
    assert_eq!(
        descriptor["interface"]["backend"]["native_data_contract"],
        1
    );
    let functions = descriptor["interface"]["functions"].as_array().unwrap();
    assert!(functions.iter().any(|function| {
        function["conversion_plan"]
            .as_array()
            .unwrap()
            .iter()
            .any(|step| {
                step.as_str()
                    .unwrap()
                    .contains("native-record-loan-writeback-v1")
            })
    }));
    assert!(functions.iter().any(|function| {
        function["conversion_plan"]
            .as_array()
            .unwrap()
            .iter()
            .any(|step| {
                step.as_str()
                    .unwrap()
                    .contains("context-token-current-call-result-v1")
            })
    }));
    success(f.cli("lock", "sdk", &[]));
    success(f.cli("lock", "sdk", &["--check"]));
    success(f.cli("build", "sdk", &[]));
}
