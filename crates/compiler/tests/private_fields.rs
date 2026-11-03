use rinzu_compiler::{FileId, SourceFile, compile_source};
const CLASS: &str = r#"
class Counter {
    private value:Int,
    private const limit:Int
    fn create(value:Int)->Counter{return Counter(value:value,limit:100);}
    fn read(self:Counter)->Int{return self.value;}
    fn set(self:Counter,value:Int){self.value=value;}
    fn readOther(self:Counter,other:Counter)->Int{return other.value;}
    fn getter(self:Counter)->fn()->Int{return fn()->Int{return self.value;};}
}
"#;
fn compile(
    text: &str,
) -> Result<rinzu_compiler::FrontendArtifact, Vec<rinzu_compiler::Diagnostic>> {
    compile_source(&SourceFile::new(FileId(305), "private-fields.rinzu", text))
}
#[test]
fn factories_and_owner_closures_can_use_private_storage() {
    let artifact=compile(&format!("{CLASS} fn main(){{let c=Counter.create(1);c.set(2);let n=c.read();let get=c.getter();let result=get();}}")).unwrap();
    assert_eq!(
        artifact.checked.source_types["Counter"].private_fields,
        [0, 1]
    );
    assert_eq!(artifact.checked.source_types["Counter"].fields.len(), 2);
}
#[test]
fn free_functions_other_types_and_direct_construction_cannot_bypass_the_boundary() {
    for expression in [
        "let x=c.value;",
        "c.value=4;",
        "let x=Counter(value:4,limit:100);",
        "let get=fn()->Int{return c.value;};",
    ] {
        let text = format!("{CLASS} fn main(){{let c=Counter.create(1);{expression}}}");
        let errors = compile(&text).unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains("private")),
            "{errors:?}"
        );
    }
    compile("class Public{value:Int}fn main(){let p=Public(value:1);p.value=2;}").unwrap();
}

#[test]
fn file_imports_expose_factories_but_keep_private_types_and_fields_internal() {
    let root = std::env::temp_dir().join(format!("rinzu-private-fields-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("rinzu.toml"),"[package]\nname=\"privacy\"\nlanguage=\"0.5\"\nkind=\"application\"\nentry=\"src/main.rinzu\"\n").unwrap();
    std::fs::write(root.join("src/api.rinzu"),"class Internal{value:Int}class Counter{private const state:Internal fn create()->Counter{return Counter(state:Internal(value:3));}fn read(self:Counter)->Int{return self.state.value;}}export Counter;").unwrap();
    let main =
        "import \"./api.rinzu\" as api;fn main(){let c=api.Counter.create();let n=c.read();}";
    std::fs::write(root.join("src/main.rinzu"), main).unwrap();
    rinzu_compiler::check_package(&root).unwrap();
    std::fs::write(
        root.join("src/main.rinzu"),
        main.replace("let n=c.read();", "let n=c.state;"),
    )
    .unwrap();
    let errors = rinzu_compiler::check_package(&root).unwrap_err();
    assert!(
        errors.iter().any(|e| e.message.contains("private field")),
        "{errors:?}"
    );
    std::fs::remove_dir_all(root).unwrap();
}
