use rinzu_compiler::{
    FileId, SourceFile, check_package, compile_source, format_source, parse_source,
    source_documentation,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

fn source(text: &str) -> SourceFile {
    SourceFile::new(FileId(1), "visibility.rinzu", text)
}

const COUNTER: &str = r#"
class Counter {
    value: Int
    private fn raw(self: Counter) -> Int { return self.value; }
    private fn initial() -> Int { return 7; }
    fn create() -> Counter { return Counter(value: Counter.initial()); }
    fn read(self: Counter) -> Int { return self.raw(); }
    fn reader(self: Counter) -> fn() -> Int {
        return fn() -> Int {
            let nested = fn() -> Int { return self.raw(); };
            return nested();
        };
    }
    fn unbound() -> fn(Counter) -> Int { return Counter.raw; }
    fn readOther(self: Counter, other: Counter) -> Int { return other.raw(); }
    async fn next(self: Counter) -> Int { return self.raw(); }
}
export Counter;
"#;

#[test]
fn private_access_is_lexically_limited_to_the_owner_including_nested_closures() {
    compile_source(&source(&format!(
        "{COUNTER} async fn main() {{ let c=Counter.create();let n=c.read();let f=c.reader();let a=f();let unbound=Counter.unbound();let b=unbound(c);let next=await c.next(); }}"
    )))
    .unwrap();
    for expression in [
        "c.raw()",
        "Counter.raw(c)",
        "Counter.raw",
        "Counter.initial()",
        "Counter.initial",
        "fn()->Int{return c.raw();}",
        "fn()->fn()->Int{return fn()->Int{return Counter.raw(c);};}",
    ] {
        for caller in [
            format!("fn outside(c:Counter){{let value={expression};}}"),
            format!(
                "class Other{{const value:Int fn outside(self:Other,c:Counter){{let value={expression};}}}}"
            ),
        ] {
            let text = format!("{COUNTER}{caller}fn main(){{}}");
            let errors = compile_source(&source(&text)).unwrap_err();
            assert!(
                errors.iter().any(|e| e.message.contains("private method")),
                "{text}: {errors:#?}"
            );
        }
    }
}

#[test]
fn private_modifier_formats_and_documents_the_original_declaration() {
    let text = r#"class Example{const private:Int
/// Internal asynchronous operation.
private async fn read(self:Example)->Int{return self.private;}
async fn run(self:Example)->Int{return await self.read();}}
fn main(){let private=1;}"#;
    let formatted = format_source(&source(text)).unwrap();
    assert_eq!(format_source(&source(&formatted)).unwrap(), formatted);
    assert!(formatted.contains("private async fn read"));
    let syntax = parse_source(&source(&formatted)).unwrap();
    let rinzu_compiler::ast::Declaration::Function(method) = &syntax.declarations[1] else {
        panic!("method declaration")
    };
    assert!(method.is_private);
    assert_eq!(method.lexical_owner.as_deref(), Some("Example"));
    let docs = source_documentation(&source(&formatted)).unwrap();
    let method = docs.iter().find(|doc| doc.name == "Example.read").unwrap();
    assert_eq!(method.documentation, "Internal asynchronous operation.");
    assert!(method.signature.starts_with("private async fn"));
    compile_source(&source(&formatted)).unwrap();
    let errors = compile_source(&source("private fn helper(){}fn main(){}")).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("private can only modify"))
    );
}

#[test]
fn completion_only_offers_private_methods_inside_the_owning_types_methods() {
    let text = "class Counter{value:Int private fn raw(self:Counter)->Int{return self.value;}fn read(self:Counter)->Int{let inside=1;return self.raw();}}class Other{value:Int fn read(self:Other){let other=1;}}fn main(){let outside=1;}";
    let file = source(text);
    let syntax = parse_source(&file).unwrap();
    let sources = std::collections::BTreeMap::from([(file.path().to_owned(), file.clone())]);
    let index =
        rinzu_compiler::build_editor_index(&sources, &[(file.clone(), syntax.clone())], &[&syntax]);
    for (marker, visible) in [
        ("let inside", true),
        ("let other", false),
        ("let outside", false),
    ] {
        let offset = text.find(marker).unwrap();
        let position = rinzu_compiler::SourceRange::empty(FileId(1), offset);
        assert_eq!(
            index
                .visible_symbols(position)
                .iter()
                .any(|symbol| symbol.name == "Counter.raw"),
            visible,
            "{marker}"
        );
    }
    let file = source(
        "class Example{value:Int private fn private(self:Example)->Int{return self.value;}fn read(self:Example)->Int{return self.private();}}fn main(){}",
    );
    let syntax = parse_source(&file).unwrap();
    let sources = std::collections::BTreeMap::from([(file.path().to_owned(), file.clone())]);
    let index =
        rinzu_compiler::build_editor_index(&sources, &[(file.clone(), syntax.clone())], &[&syntax]);
    let definition = index.targets["Example.private"];
    assert_eq!(
        &file.text()[definition.start_byte as usize..definition.end_byte as usize],
        "private"
    );
    assert_eq!(
        definition.start_byte as usize,
        file.text().find("fn private").unwrap() + 3
    );
    compile_source(&file).unwrap();
}

#[test]
fn exported_owner_publishes_methods_and_retains_used_private_bodies() {
    let sdk = Package::new("sdk", "api", &[("api", COUNTER)]);
    let (_, artifact) = check_package(&sdk.0).unwrap();
    let interface = artifact.source_interface.unwrap();
    let exports = &interface.modules["fixture"];
    for name in [
        "Counter",
        "Counter.create",
        "Counter.read",
        "Counter.reader",
        "Counter.next",
    ] {
        assert!(exports.contains_key(name), "{exports:#?}");
    }
    assert!(!exports.contains_key("Counter.raw"));
    assert!(!exports.contains_key("Counter.initial"));
    assert!(
        artifact
            .ir
            .functions
            .iter()
            .any(|f| f.name == "Counter.raw")
    );
    let app = Package::new(
        "application",
        "main",
        &[(
            "main",
            r#"
import fixture;
async fn main(){
    let c=fixture.Counter(value:1);
    c.value=2;
    let n=c.read();
    let f=c.reader();
    let nested=f();
    let unbound=fixture.Counter.read;
    let explicit=unbound(c);
    let next=await c.next();
}
"#,
        )],
    );
    app.depend(&sdk);
    check_package(&app.0).unwrap();
    for expression in ["c.raw()", "fixture.Counter.raw(c)", "fixture.Counter.raw"] {
        app.write(
            "main",
            &format!(
                "import fixture;fn main(){{let c=fixture.Counter(value:1);let value={expression};}}"
            ),
        );
        let errors = check_package(&app.0).unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("private method")));
    }
}

#[test]
fn individual_method_exports_are_rejected_even_for_public_or_unexported_owners() {
    for declaration in [
        "class Owner{value:Int fn read(self:Owner)->Int{return self.value;}}",
        "class Owner{value:Int private fn read(self:Owner)->Int{return self.value;}}",
        "class Owner{value:Int fn read()->Int{return 1;}}",
        "enum Owner{Ready}fn Owner.read(self:Owner)->Int{return 1;}",
        "native{record Owner{value:Int}}fn Owner.read(self:Owner)->Int{return self.value;}",
        "native{opaque Owner;}fn Owner.read(self:Owner)->Int{return 1;}",
    ] {
        for owner_export in ["", "export Owner;"] {
            let text = format!("{declaration}{owner_export}export Owner.read;fn main(){{}}");
            let errors = compile_source(&source(&text)).unwrap_err();
            assert!(
                errors
                    .iter()
                    .any(|e| e.message.contains("cannot be exported individually")),
                "{text}: {errors:#?}"
            );
            let sdk = Package::new("sdk", "api", &[("api", &text)]);
            let errors = check_package(&sdk.0).unwrap_err();
            assert!(
                errors
                    .iter()
                    .any(|e| e.message.contains("cannot be exported individually")),
                "{text}: {errors:#?}"
            );
        }
    }
    let sdk = Package::new("sdk", "api", &[("api", COUNTER)]);
    for export in [
        "export fixture.Counter.read;",
        "export fixture.Counter.raw;",
    ] {
        let facade = Package::new(
            "sdk",
            "api",
            &[("api", &format!("import fixture;{export}"))],
        );
        facade.depend(&sdk);
        assert!(check_package(&facade.0).is_err());
    }
    for import in ["import model;", "import \"./model.rinzu\" as model;"] {
        let facade = Package::new(
            "sdk",
            "api",
            &[
                ("api", &format!("{import}export model.Counter.read;")),
                ("model", COUNTER),
            ],
        );
        let errors = check_package(&facade.0).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("cannot be exported individually"))
        );
    }
}

#[test]
fn every_public_signature_requires_explicitly_exported_dependency_types() {
    compile_source(&source(
        "class T{value:Int}class Box<T>{const value:T}export Box;fn main(){}",
    ))
    .unwrap();
    for definition in [
        "class Public{const value:Hidden}export Public;",
        "class Public{value:Int fn make(self:Public)->Hidden{return Hidden(value:1);}}export Public;",
        "class Public{value:Int fn useValue(self:Public,value:Hidden){}}export Public;",
        "class Public{value:Int fn make(self:Public)->{items:List<Hidden>}{return {items=[]};}}export Public;",
        "class Public{value:Int fn useCallback(self:Public,f:fn(Hidden)->Int){}}export Public;",
        "enum Public{Value(value:Hidden)}export Public;",
        "error Public{payload:Hidden}export Public;",
        "type Public={values:Map<Int,Hidden>};export Public;",
        "let value:Hidden=Hidden(value:1);export value;",
    ] {
        let text = format!("class Hidden{{const value:Int}}{definition}fn main(){{}}");
        let errors = compile_source(&source(&text)).unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains("private type")),
            "{text}: {errors:#?}"
        );
        for kind in ["application", "sdk"] {
            let package = Package::new(kind, "main", &[("main", &text)]);
            let errors = check_package(&package.0).unwrap_err();
            assert!(
                errors.iter().any(|e| e.message.contains("private type")),
                "{kind}: {text}: {errors:#?}"
            );
        }
        compile_source(&source(&format!("{text}export Hidden;"))).unwrap();
    }
    for private in ["", "private "] {
        let text = format!(
            "class Hidden{{value:Int}}class Public{{value:Int {private}fn helper(self:Public)->Int{{let local=Hidden(value:self.value);return local.value;}}}}export Public;fn main(){{}}"
        );
        compile_source(&source(&text)).unwrap();
    }
    compile_source(&source("class Hidden{value:Int}class Public{value:Int private fn hidden(self:Public)->Hidden{return Hidden(value:self.value);}fn read(self:Public)->Int{let value=self.hidden();return value.value;}}export Public;fn main(){}" )).unwrap();
}

#[test]
fn reexports_include_methods_across_multiple_modules_and_sdk_dependencies() {
    let sdk = Package::new(
        "sdk",
        "api",
        &[
            ("api", "import facade;export facade.Counter;"),
            ("facade", "import model;export model.Counter;"),
            ("model", COUNTER),
        ],
    );
    let (_, artifact) = check_package(&sdk.0).unwrap();
    let interface = artifact.source_interface.unwrap();
    assert_eq!(
        interface.modules["fixture"]["Counter.read"],
        "model::Counter.read"
    );
    assert!(!interface.modules["fixture"].contains_key("Counter.raw"));
    assert!(
        artifact
            .module_graph
            .iter()
            .find(|m| m.is_entry)
            .unwrap()
            .exports
            .contains_key("Counter.read")
    );
    let facade = Package::new(
        "sdk",
        "api",
        &[("api", "import fixture;export fixture.Counter;")],
    );
    facade.name("facade");
    facade.depend(&sdk);
    let (_, artifact) = check_package(&facade.0).unwrap();
    let interface = artifact.source_interface.unwrap();
    assert_eq!(
        interface.modules["facade"]["Counter.read"],
        "fixture::model::Counter.read"
    );
    assert!(!interface.modules["facade"].contains_key("Counter.raw"));
}

#[test]
fn enum_methods_preserve_owner_exports_and_private_access_rules() {
    let text = r#"
enum Event{Ready}
private fn Event.raw(self:Event)->Int{return 1;}
fn Event.read(self:Event)->Int{return self.raw();}
export Event;
"#;
    let sdk = Package::new("sdk", "api", &[("api", text)]);
    let (_, artifact) = check_package(&sdk.0).unwrap();
    let interface = artifact.source_interface.unwrap();
    for name in ["Event.read"] {
        assert!(interface.modules["fixture"].contains_key(name));
    }
    for name in ["Event.raw"] {
        assert!(!interface.modules["fixture"].contains_key(name));
    }
    let app = Package::new(
        "application",
        "main",
        &[(
            "main",
            "import fixture;fn main(){let e=fixture.Event.Ready();let a=e.read();}",
        )],
    );
    app.depend(&sdk);
    check_package(&app.0).unwrap();
    for (owner, result) in [("Event", "Int")] {
        let errors = compile_source(&source(&format!(
            "{text}fn outside(value:{owner})->{result}{{return value.raw();}}fn main(){{}}"
        )))
        .unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("private method")));
    }
}

#[test]
fn imported_native_types_and_transparent_aliases_cannot_gain_methods() {
    for declaration in ["opaque Owner;", "record Owner{value:Int}"] {
        let sdk = Package::new(
            "sdk",
            "api",
            &[("api", &format!("native{{{declaration}}}export Owner;"))],
        );
        for method in [
            "fn fixture.Owner.read()->Int{return 1;}",
            "type Alias=fixture.Owner;fn Alias.read(self:Alias)->Int{return 1;}",
            "type Alias=fixture.Owner;type Second=Alias;private async fn Second.read(self:Second)->Int{return 1;}",
        ] {
            let app = Package::new(
                "application",
                "main",
                &[("main", &format!("import fixture;{method}fn main(){{}}"))],
            );
            app.depend(&sdk);
            let errors = check_package(&app.0).unwrap_err();
            assert!(
                errors
                    .iter()
                    .any(|e| e.message.contains("cannot own methods")
                        || e.message.contains("local Owner.method")),
                "{method}: {errors:#?}"
            );
        }
        let sdk = Package::new(
            "sdk",
            "api",
            &[(
                "api",
                &format!("native{{{declaration}}}fn Owner.read()->Int{{return 1;}}export Owner;"),
            )],
        );
        assert!(check_package(&sdk.0).is_err());
    }
}

struct Package(PathBuf);

impl Package {
    fn new(kind: &str, entry: &str, modules: &[(&str, &str)]) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "rinzu-method-visibility-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("rinzu.toml"), format!("[package]\nname=\"fixture\"\nlanguage=\"0.5\"\nkind=\"{kind}\"\nentry=\"src/{entry}.rinzu\"\n")).unwrap();
        let package = Self(root);
        for (name, text) in modules {
            package.write(name, text);
        }
        package
    }

    fn write(&self, name: &str, text: &str) {
        std::fs::write(self.0.join(format!("src/{name}.rinzu")), text).unwrap();
    }

    fn name(&self, name: &str) {
        let manifest = self.0.join("rinzu.toml");
        std::fs::write(
            &manifest,
            std::fs::read_to_string(&manifest)
                .unwrap()
                .replace("name=\"fixture\"", &format!("name=\"{name}\"")),
        )
        .unwrap();
    }

    fn depend(&self, sdk: &Self) {
        let manifest = self.0.join("rinzu.toml");
        std::fs::write(
            &manifest,
            format!(
                "{}\n[dependencies]\nfixture={{path=\"../{}\"}}\n",
                std::fs::read_to_string(&manifest).unwrap(),
                sdk.0.file_name().unwrap().to_str().unwrap()
            ),
        )
        .unwrap();
    }
}

impl Drop for Package {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
