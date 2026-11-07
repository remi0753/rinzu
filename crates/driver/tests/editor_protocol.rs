#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "rinzu-editor-protocol-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("src")).unwrap();
        Self(root.canonicalize().unwrap())
    }
    fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }
    fn package(&self) {
        self.file("rinzu.toml", "[package]\nname=\"edit\"\nlanguage=\"0.5\"\n");
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct Client {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    id: u64,
}
impl Client {
    fn new(workspace: Option<&Path>) -> Self {
        let mut command = Command::new(frozen_cli::path());
        command.args(["editor", "--stdio"]);
        if let Some(workspace) = workspace {
            command.arg(workspace);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        Self {
            input: child.stdin.take(),
            output: BufReader::new(child.stdout.take().unwrap()),
            child,
            id: 0,
        }
    }
    fn raw(&mut self, bytes: &[u8]) -> Value {
        let input = self.input.as_mut().unwrap();
        input.write_all(bytes).unwrap();
        input.flush().unwrap();
        let mut line = String::new();
        assert!(
            self.output.read_line(&mut line).unwrap() > 0,
            "server exited"
        );
        serde_json::from_str(&line).unwrap()
    }
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        let id = self.id;
        let request = json!({"id":id,"method":method,"params":params});
        let response = self.raw(format!("{request}\n").as_bytes());
        assert_eq!(response["id"], id);
        assert_eq!(response["schema"], "rinzu-editor/1");
        response
    }
    fn ok(&mut self, method: &str, params: Value) -> Value {
        let response = self.request(method, params);
        assert!(response.get("error").is_none(), "{response:#}");
        response
    }
    fn open(&mut self, path: &Path, version: u64, text: &str) -> Value {
        self.ok("open", json!({"path":path,"version":version,"text":text}))
    }
    fn query(&mut self, method: &str, path: &Path, version: u64, position: Value) -> Value {
        self.ok(
            method,
            json!({"path":path,"version":version,"position":position,"includeDeclaration":true}),
        )
    }
    fn shutdown(&mut self) {
        self.ok("shutdown", json!({}));
        self.input.take();
        assert!(self.child.wait().unwrap().success());
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn position(text: &str, needle: &str) -> Value {
    let before = &text[..text.find(needle).unwrap()];
    json!({"line":before.bytes().filter(|b|*b==b'\n').count(),"character":before.rsplit('\n').next().unwrap().encode_utf16().count()})
}
fn labels(response: &Value) -> Vec<&str> {
    response["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["label"].as_str().unwrap())
        .collect()
}

fn formatted(path: &Path, text: &str) -> String {
    rinzu_compiler::format_source(&rinzu_compiler::SourceFile::new(
        rinzu_compiler::FileId(1),
        path,
        text,
    ))
    .unwrap()
}

#[test]
fn unicode_lexical_errors_have_precise_editor_ranges() {
    let fixture = Fixture::new();
    fixture.package();
    let entry = fixture.file("src/main.rinzu", "fn main() {}");
    let mut client = Client::new(Some(&fixture.0));
    for (index, text) in [
        "fn main() { 😀; }",
        "fn main() { let x = b\"é\"; }",
        "fn main() { let x = \"\\😀\"; }",
    ]
    .into_iter()
    .enumerate()
    {
        let version = index as u64 + 1;
        if index == 0 {
            client.open(&entry, version, text);
        } else {
            client.ok(
                "change",
                json!({"path":entry,"version":version,"text":text}),
            );
        }
        let response = client.query("diagnostics", &entry, version, json!(null));
        let diagnostics = response["result"]["diagnostics"].as_array().unwrap();
        assert_eq!(diagnostics.len(), 1, "{response:#}");
        let primary = &diagnostics[0]["primary"];
        assert_eq!(primary["fileLevel"], false, "{response:#}");
        assert_eq!(primary["range"]["start"]["line"], 0);
        let start = primary["range"]["start"]["character"].as_u64().unwrap();
        let end = primary["range"]["end"]["character"].as_u64().unwrap();
        assert!(end > start, "{response:#}");
        if index == 0 {
            assert_eq!((start, end), (12, 14));
        }
    }
    client.shutdown();
}

#[test]
fn explicit_unsaved_file_graph_uses_overlay_navigation_and_atomic_initialize() {
    let fixture = Fixture::new();
    fixture.file("rinzu.toml", "invalid nearby manifest must be ignored");
    let entry = fixture.0.join("entry.rinzu");
    let target = fixture.0.join("model.rinzu");
    let model = formatted(
        &target,
        "fn value()->Int{return 42;}fn hidden()->Int{return 9;}export value;",
    );
    fixture.file("model.rinzu", &model);
    let link = fixture.0.join("alias.rinzu");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let facade = fixture.0.join("src/facade.rinzu");
    let facade_text = formatted(
        &facade,
        "import \"../alias.rinzu\" as model;export model.value;",
    );
    fixture.file("src/facade.rinzu", &facade_text);
    let text = formatted(
        &entry,
        "import \"./src/facade.rinzu\" as api;import \"./model.rinzu\" as direct;fn main(){let a:Int=api.value();let b:Int=direct.value();}",
    );
    let mut client = Client::new(Some(&fixture.0));
    let initialized = client.ok("initialize", json!({"entry":entry}));
    assert_eq!(initialized["result"]["entry"], json!(entry));
    client.open(&entry, 1, &text);
    let valid = client.query("diagnostics", &entry, 1, json!(null));
    assert_eq!(valid["result"]["diagnostics"], json!([]), "{valid:#}");
    assert_eq!(valid["result"]["inputs"].as_array().unwrap().len(), 3);
    assert!(
        valid["result"]["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|input| input["path"] == json!(entry) && input["kind"] == "overlay")
    );
    let target_location = client.query("definition", &entry, 1, position(&text, "value()"));
    assert_eq!(
        target_location["result"]["locations"][0]["path"],
        json!(target)
    );
    let items = client.query("completion", &entry, 1, json!({"line":0,"character":0}));
    assert!(labels(&items).contains(&"api.value"));
    assert!(labels(&items).contains(&"direct.value"));
    assert!(!labels(&items).contains(&"direct.hidden"));
    client.open(&link, 1, &model);
    let references = client.query("references", &link, 1, position(&model, "value()"));
    let locations = references["result"]["locations"].as_array().unwrap();
    assert!(
        locations
            .iter()
            .filter(|location| location["path"] == json!(entry))
            .count()
            >= 2,
        "{references:#}"
    );
    let wrong = formatted(
        &target,
        "fn value()->String{return \"changed\";}export value;",
    );
    client.ok("change", json!({"path":link,"version":2,"text":wrong}));
    let errors = client.query("diagnostics", &entry, 1, json!(null));
    assert!(
        !errors["result"]["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(errors["result"]["syntaxOnly"], true);
    let restored = formatted(&target, "fn value()->Int{return 7;}export value;");
    client.ok("change", json!({"path":link,"version":3,"text":restored}));
    assert_eq!(
        client.query("diagnostics", &entry, 1, json!(null))["result"]["diagnostics"],
        json!([])
    );
    let generation = client.ok("initialize", json!({}))["generation"].clone();
    let rejected = client.request("initialize", json!({"workspace":null,"entry":fixture.0}));
    assert_eq!(rejected["error"]["code"], "InvalidPath");
    assert_eq!(rejected["generation"], generation);
    let preserved = client.ok("initialize", json!({}));
    assert_eq!(preserved["result"]["workspace"], json!(fixture.0));
    assert_eq!(preserved["result"]["entry"], json!(entry));
    client.ok("close", json!({"path":link,"version":3}));
    assert_eq!(
        client.query("diagnostics", &entry, 1, json!(null))["result"]["diagnostics"],
        json!([])
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), model);
    assert!(!entry.exists());
    client.shutdown();
}

#[test]
fn path_import_edits_keep_literal_escapes_and_distinct_aliases() {
    let fixture = Fixture::new();
    let path = fixture.0.join("src/imports.rinzu");
    let text = rinzu_compiler::format_source(&rinzu_compiler::SourceFile::new(
        rinzu_compiler::FileId(1),
        &path,
        r#"import rinzu.option as opt;import "./z.rinzu" as z;import "./same\u{2e}rinzu" as second;import "./same\u{2e}rinzu" as first;fn main(){}"#,
    ))
    .unwrap();
    let mut client = Client::new(None);
    client.open(&path, 1, &text);
    let result = client.query("organizeImports", &path, 1, json!(null));
    let edited = result["result"]["edits"][0]["newText"].as_str().unwrap();
    assert!(edited.contains(r#"import "./same\u{2e}rinzu" as first;"#));
    assert!(edited.contains(r#"import "./same\u{2e}rinzu" as second;"#));
    assert!(edited.find("as first").unwrap() < edited.find("as second").unwrap());
    assert!(edited.find("as second").unwrap() < edited.find("as z").unwrap());
    assert!(edited.find("as z").unwrap() < edited.find("import rinzu.option as opt;").unwrap());
    client.ok("change", json!({"path":path,"version":2,"text":edited}));
    assert_eq!(
        client.query("organizeImports", &path, 2, json!(null))["result"]["edits"],
        json!([]),
    );
    assert!(!path.exists());
    client.shutdown();
}

#[test]
fn unicode_diagnostics_bom_and_rich_secondary_locations_are_original_bytes() {
    let fixture = Fixture::new();
    let path = fixture.0.join("src/日本.rinzu");
    let text = "// 😀日本\r\nfn wrong()->Int{return \"😀\";}";
    let mut client = Client::new(None);
    client.ok(
        "open",
        json!({"path":path,"version":1,"text":text,"bom":true}),
    );
    let response = client.query("diagnostics", &path, 1, json!(null));
    let errors = response["result"]["diagnostics"].as_array().unwrap();
    assert!(!errors.is_empty());
    let d = errors.iter().find(|d| d["code"] == "RZ0301").unwrap();
    let primary = &d["primary"];
    let byte = primary["originalBytes"]["start"].as_u64().unwrap() as usize;
    let logical = byte - 3;
    let expected = position(text, &text[logical..]);
    assert_eq!(primary["range"]["start"], expected);
    assert_eq!(primary["path"], json!(path));
    assert_eq!(primary["fileLevel"], false);
    assert!(d["suggestions"].is_array());
    assert!(!path.exists());
    client.shutdown();
}

#[test]
fn unsaved_dependency_changes_definition_completions_and_invalid_code_recovery() {
    let fixture = Fixture::new();
    fixture.package();
    let main_text = "import model as m;fn main(){let n:Int=m.value();}";
    let main = fixture.file("src/main.rinzu", main_text);
    let dependency = fixture.file(
        "src/model.rinzu",
        "fn value()->String{return \"disk\";}fn hidden()->Int{return 4;}export value;",
    );
    let mut client = Client::new(Some(&fixture.0));
    client.open(&main, 1, main_text);
    assert!(
        !client.query("diagnostics", &main, 1, json!(null))["result"]["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    client.open(
        &dependency,
        1,
        "fn value()->Int{return 3;}fn hidden()->Int{return 4;}export value;",
    );
    let valid = client.query("diagnostics", &main, 1, json!(null));
    assert_eq!(valid["result"]["diagnostics"], json!([]));
    assert_eq!(valid["result"]["syntaxOnly"], false);
    let target = client.query("definition", &main, 1, position(main_text, "value()"));
    assert_eq!(target["result"]["locations"][0]["path"], json!(dependency));
    let items = client.query("completion", &main, 1, json!({"line":0,"character":0}));
    assert!(labels(&items).contains(&"m.value"));
    assert!(!labels(&items).contains(&"m.hidden"));
    assert!(!labels(&items).contains(&"model.value"));
    let again = client.query("diagnostics", &main, 1, json!(null));
    assert_eq!(again["result"]["parseCache"]["misses"], 0);
    assert!(again["result"]["parseCache"]["hits"].as_u64().unwrap() >= 2);
    client.ok(
        "change",
        json!({"path":dependency,"version":2,"text":"fn value( {"}),
    );
    let invalid = client.query("definition", &main, 1, position(main_text, "value()"));
    assert_eq!(invalid["result"]["syntaxOnly"], true);
    assert_eq!(invalid["result"]["locations"], json!([]));
    client.ok(
        "change",
        json!({"path":dependency,"version":3,"text":"fn value()->Int{return 8;}export value;"}),
    );
    assert_eq!(
        client.query("diagnostics", &main, 1, json!(null))["result"]["diagnostics"],
        json!([])
    );
    client.ok("close", json!({"path":dependency,"version":3}));
    assert!(
        !client.query("diagnostics", &main, 1, json!(null))["result"]["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        std::fs::read_to_string(dependency)
            .unwrap()
            .contains("String")
    );
    client.shutdown();
}

#[test]
fn recursive_sdk_overlay_has_public_navigation_and_manifest_receipts() {
    let fixture = Fixture::new();
    fixture.file(
        "rinzu.toml",
        "[package]\nname=\"edit\"\nlanguage=\"0.5\"\n[dependencies]\nmodel={path=\"model\"}\n",
    );
    fixture.file("model/rinzu.toml","[package]\nname=\"model\"\nlanguage=\"0.5\"\nkind=\"sdk\"\nentry=\"src/model.rinzu\"\n[dependencies]\nbase={path=\"../base\"}\n");
    fixture.file(
        "base/rinzu.toml",
        "[package]\nname=\"base\"\nlanguage=\"0.5\"\nkind=\"sdk\"\nentry=\"src/base.rinzu\"\n",
    );
    let base = fixture.file(
        "base/src/base.rinzu",
        "fn value()->String{return \"old\";}export value;",
    );
    let model = fixture.file(
        "model/src/model.rinzu",
        "import base as dep;type Producer=fn () nothrow->Int;fn value()->Int{return dep.value();}fn hidden()->Int{return 7;}export Producer;export value;",
    );
    let main_text =
        "import model as m;fn main(){let producer:m.Producer=m.value;let n=producer();}";
    let main = fixture.file("src/main.rinzu", main_text);
    let mut client = Client::new(Some(&fixture.0));
    client.open(&main, 1, main_text);
    client.open(&base, 1, "fn value()->Int{return 42;}export value;");
    let response = client.query("diagnostics", &main, 1, json!(null));
    assert_eq!(response["result"]["diagnostics"], json!([]), "{response:#}");
    assert_eq!(
        response["result"]["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|i| i["kind"] == "disk-manifest")
            .count(),
        3
    );
    for name in ["Producer", "value;"] {
        let target = client.query("definition", &main, 1, position(main_text, name));
        assert_eq!(target["result"]["locations"][0]["path"], json!(model));
    }
    let items = client.query("completion", &main, 1, json!({"line":0,"character":0}));
    assert!(labels(&items).contains(&"m.Producer"));
    assert!(labels(&items).contains(&"m.value"));
    assert!(!labels(&items).contains(&"m.hidden"));
    client.shutdown();
}

#[test]
fn generic_closure_shadowing_references_exclude_comments_and_literals() {
    let fixture = Fixture::new();
    let path = fixture.0.join("src/loose.rinzu");
    let text = "fn id<T>(a:T)->T{return a;}fn test()->Int{let item:Int=7;let read=fn(item:Int)->Int{return item;};let capture=fn()->Int{return item;};let text=\"item\";return id<Int>(read(capture()));}// item\n";
    let mut client = Client::new(None);
    client.open(&path, 1, text);
    assert_eq!(
        client.query("diagnostics", &path, 1, json!(null))["result"]["diagnostics"],
        json!([])
    );
    let captured = client.query("definition", &path, 1, position(text, "item;};let text"));
    assert_eq!(
        captured["result"]["locations"][0]["originalBytes"]["start"],
        text.find("item:Int=7").unwrap()
    );
    let shadow = client.query("definition", &path, 1, position(text, "item;};let capture"));
    assert_eq!(
        shadow["result"]["locations"][0]["originalBytes"]["start"],
        text.find("fn(item:Int)").unwrap() + 3
    );
    let references = client.query("references", &path, 1, position(text, "item:Int=7"));
    let locations = references["result"]["locations"].as_array().unwrap();
    assert_eq!(locations.len(), 2, "{references:#}");
    assert!(
        locations
            .iter()
            .all(|l| l["originalBytes"]["start"].as_u64().unwrap()
                < (text.find("let text").unwrap() as u64))
    );
    let generic = client.query("definition", &path, 1, position(text, "id<Int>"));
    assert_eq!(
        generic["result"]["locations"][0]["originalBytes"]["start"],
        3
    );
    let end = client.query("completion", &path, 1, json!({"line":1,"character":0}));
    assert!(labels(&end).contains(&"id"));
    assert!(!labels(&end).contains(&"read"));
    client.shutdown();
}

#[test]
fn format_and_import_edits_preserve_comments_bom_literals_and_are_idempotent() {
    let fixture = Fixture::new();
    let path = fixture.0.join("src/format.rinzu");
    let text = "// 頭😀\r\nimport z; import a; import z;\r\n// 保持\r\nimport c;\r\nfn f(){let raw=r##\"a\r\n\0\"##;let value=f\"日本-{1}\";}";
    let mut client = Client::new(None);
    client.ok(
        "open",
        json!({"path":path,"version":1,"text":text,"bom":true}),
    );
    let organized = client.query("organizeImports", &path, 1, json!(null));
    let next = organized["result"]["edits"][0]["newText"].as_str().unwrap();
    assert!(
        next.contains("import a;\r\nimport z;\r\n// 保持\r\nimport c;"),
        "{next:?}"
    );
    assert!(next.contains("r##\"a\r\n\0\"##"));
    assert!(!next.starts_with('\u{feff}'));
    client.ok("change", json!({"path":path,"version":2,"text":next}));
    assert_eq!(
        client.query("organizeImports", &path, 2, json!(null))["result"]["edits"],
        json!([])
    );
    let formatted = client.query("format", &path, 2, json!(null));
    let formatted = formatted["result"]["edits"][0]["newText"].as_str().unwrap();
    assert!(formatted.contains("// 頭😀"));
    assert!(formatted.contains("r##\"a\r\n\0\"##"));
    client.ok("change", json!({"path":path,"version":3,"text":formatted}));
    assert_eq!(
        client.query("format", &path, 3, json!(null))["result"]["edits"],
        json!([])
    );
    client.ok(
        "change",
        json!({"path":path,"version":4,"text":"fn invalid( {"}),
    );
    assert_eq!(
        client.query("format", &path, 4, json!(null))["result"]["edits"],
        json!([])
    );
    assert_eq!(
        client.query("organizeImports", &path, 4, json!(null))["result"]["edits"],
        json!([])
    );
    assert!(!path.exists());
    client.shutdown();
}

#[test]
fn rename_virtual_buffers_disk_refresh_and_invalid_utf8_are_explicit() {
    let fixture = Fixture::new();
    fixture.package();
    let text = "import old;fn main(){let n=old.value();}";
    let main = fixture.file("src/main.rinzu", text);
    let old = fixture.file("src/old.rinzu", "fn value()->Int{return 1;}export value;");
    let new = fixture.0.join("src/new.rinzu");
    let mut client = Client::new(Some(&fixture.0));
    client.open(&main, 1, text);
    client.open(&old, 1, "fn value()->Int{return 2;}export value;");
    client.ok("close", json!({"path":old,"version":1}));
    std::fs::rename(&old, &new).unwrap();
    client.open(&new, 1, "fn value()->Int{return 2;}export value;");
    let changed = "import new;fn main(){let n=new.value();}";
    client.ok("change", json!({"path":main,"version":2,"text":changed}));
    client.ok("refresh", json!({}));
    assert_eq!(
        client.query("definition", &main, 2, position(changed, "value()"))["result"]["locations"]
            [0]["path"],
        json!(new)
    );
    client.ok("close", json!({"path":new,"version":1}));
    std::fs::write(&new, [0xef, 0xbb, 0xbf, b'a', 0xff]).unwrap();
    client.ok("refresh", json!({}));
    let invalid = client.query("diagnostics", &main, 2, json!(null));
    let d = invalid["result"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["code"] == "RZ0001")
        .unwrap();
    assert_eq!(d["primary"]["path"], json!(new));
    assert_eq!(d["primary"]["fileLevel"], true);
    assert_eq!(d["primary"]["originalBytes"]["start"], 4);
    client.shutdown();
}

#[test]
fn stale_versions_invalid_positions_frames_and_limits_recover_without_mutation() {
    let fixture = Fixture::new();
    let path = fixture.0.join("src/a.rinzu");
    let mut client = Client::new(None);
    let text = "// 😀\nfn f(){}";
    let opened = client.open(&path, 10, text);
    let generation = opened["generation"].clone();
    for method in ["change", "close", "diagnostics"] {
        let response = client.request(method, json!({"path":path,"version":9,"text":"bad"}));
        assert_eq!(response["error"]["code"], "ContentModified");
        assert_eq!(response["generation"], generation);
    }
    let surrogate = client.request(
        "completion",
        json!({"path":path,"version":10,"position":{"line":0,"character":4}}),
    );
    assert_eq!(surrogate["error"]["code"], "InvalidPosition");
    assert_eq!(client.raw(b"not JSON\n")["error"]["code"], "ParseError");
    assert_eq!(
        client.request("unknown", json!({}))["error"]["code"],
        "MethodNotFound"
    );
    let too_big = "x".repeat(1024 * 1024 + 1);
    assert_eq!(
        client.request("change", json!({"path":path,"version":11,"text":too_big}))["error"]["code"],
        "LimitExceeded"
    );
    let mut huge = vec![b' '; 8 * 1024 * 1024 + 1];
    huge.push(b'\n');
    assert_eq!(client.raw(&huge)["error"]["code"], "LimitExceeded");
    for i in 1..32 {
        client.open(&fixture.0.join(format!("src/{i}.rinzu")), 1, "fn f(){}");
    }
    assert_eq!(
        client.request(
            "open",
            json!({"path":fixture.0.join("src/extra.rinzu"),"version":1,"text":"fn f(){}"})
        )["error"]["code"],
        "LimitExceeded"
    );
    assert_eq!(
        client.query("diagnostics", &path, 10, json!(null))["result"]["diagnostics"],
        json!([])
    );
    client.shutdown();
}

#[test]
fn eof_and_invalid_cli_usage_exit_with_the_expected_status() {
    let output = Command::new(frozen_cli::path())
        .args(["editor", "--stdio"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    let output = Command::new(frozen_cli::path())
        .arg("editor")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--stdio"));
}

#[test]
fn imported_collection_modules_offer_qualified_completions() {
    let fixture = Fixture::new();
    fixture.package();
    let source = "import rinzu.list;\nimport rinzu.map as maps;\nimport rinzu.option as opt;\nfn main() { let count = rinzu.list.length([1]); let value = opt.getOrElse(Option<Int>.Some(value: count), fn () -> Int { return 0; }); }\n";
    let entry = fixture.file("src/main.rinzu", source);
    let mut client = Client::new(Some(&fixture.0));
    client.open(&entry, 1, source);
    let response = client.query("completion", &entry, 1, position(source, "let count"));
    let items = labels(&response);
    assert!(items.contains(&"rinzu.list.map"), "{response:#}");
    assert!(items.contains(&"maps.DuplicateKey"), "{response:#}");
    assert!(items.contains(&"opt.getOrElse"), "{response:#}");
    assert!(!items.contains(&"rinzu.map.DuplicateKey"));
    assert!(!items.contains(&"listLength"));
    client.shutdown();
}

#[test]
fn rz0414_uses_the_edited_propagation_site_and_original_contract_coordinates() {
    let fixture = Fixture::new();
    let path = fixture.file("src/編集中 の例.rinzu", "fn main() {}\n");
    let text = concat!(
        "error E {}\r\n",
        "fn boundary(load: fn () throws(E) -> Int) nothrow -> Int {\r\n",
        "    try { let preview = load(); } catch failures {}\r\n",
        "    let label = \"😀\"; return load();\r\n",
        "}\r\n",
        "fn main() {}\r\n",
    );
    let mut client = Client::new(None);
    client.ok(
        "open",
        json!({"path":path,"version":1,"text":text,"bom":true}),
    );
    let response = client.query("diagnostics", &path, 1, json!(null));
    let diagnostics = response["result"]["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 1, "{response:#}");
    let diagnostic = &diagnostics[0];
    assert_eq!(diagnostic["code"], "RZ0414");
    assert_eq!(
        diagnostic["message"],
        "E can escape `boundary`, which declares `nothrow`"
    );
    for (location, offset, length) in [
        (
            &diagnostic["primary"],
            text.rfind("load()").unwrap(),
            "load()".len(),
        ),
        (
            &diagnostic["secondary"][0]["location"],
            text.find("nothrow").unwrap(),
            "nothrow".len(),
        ),
    ] {
        let before = &text[..offset];
        let line = before.bytes().filter(|&byte| byte == b'\n').count();
        let on_line = before.rsplit('\n').next().unwrap();
        let utf16 = on_line.encode_utf16().count();
        assert_eq!(
            *location,
            json!({
                "path":path,
                "range":{"start":{"line":line,"character":utf16},"end":{"line":line,"character":utf16+length}},
                "scalarStart":{"line":line+1,"column":on_line.chars().count()+1},
                "originalBytes":{"start":offset+3,"end":offset+length+3},
                "fileLevel":false
            })
        );
    }
    assert_eq!(diagnostic["secondary"].as_array().unwrap().len(), 1);
    assert_eq!(
        diagnostic["secondary"][0]["message"],
        "`boundary` declares `nothrow` here"
    );
    assert_eq!(
        diagnostic["suggestions"][0]["message"],
        "recover the Error from this call, or use `throws(E)` if callers should handle it"
    );

    let fixed = text.replace("return load();", "return 0;");
    client.ok("change", json!({"path":path,"version":2,"text":fixed}));
    let response = client.query("diagnostics", &path, 2, json!(null));
    assert_eq!(response["result"]["diagnostics"], json!([]));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "fn main() {}\n");
    client.shutdown();
}
