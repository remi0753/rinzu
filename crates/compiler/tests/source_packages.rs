use rinzu_compiler::check_package;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn private_native_abi_types_survive_in_sdk_descriptors_without_becoming_exports() {
    let package = Package::new(
        "sdk",
        "private_native",
        &[(
            "private_native",
            r#"
native{
    record Output{status:CInt,value:Int}
    opaque Handle;
    fn update(handle:Handle,output:Output)->Int symbol "private_update";
    fn callback(handler:fn(Output)->Handle)->Int symbol "private_callback";
}
fn publicApi()->Int{return 0;}
export publicApi;
"#,
        )],
    );
    let (_, artifact) = check_package(&package.0).unwrap();
    let source = artifact.source_interface.unwrap();
    for (name, kind) in [("Output", "native-record"), ("Handle", "opaque")] {
        assert_eq!(source.types[name].kind, kind);
        assert!(
            !source
                .modules
                .values()
                .any(|exports| exports.values().any(|value| value == name))
        );
    }
    assert_eq!(source.native_records["Output"].len(), 2);
}

#[test]
fn structural_aliases_are_transparent_across_module_exports() {
    let package = Package::new(
        "application",
        "main",
        &[
            (
                "main",
                "import model;type Reordered={y:Int,x:Int};fn main(){let p:Reordered=model.create();let same=p=={x:Int=1,y:Int=2};let n=model.read({x=3,y=4});}",
            ),
            (
                "model",
                "type Point={x:Int,y:Int};fn create()->Point{return {y=2,x=1};} fn read(p:Point)->Int{return p.x;}export Point;export create;export read;",
            ),
        ],
    );
    check_package(&package.0).unwrap();
    std::fs::write(
        package.0.join("src/main.rinzu"),
        "import model;fn main(){let p:model.Point={y=2,x=1};let n=model.read(p);}",
    )
    .unwrap();
    check_package(&package.0).unwrap();
    std::fs::write(
        package.0.join("src/main.rinzu"),
        "import model;fn main(){let p:model.Point={x=1};}",
    )
    .unwrap();
    assert!(
        check_package(&package.0)
            .unwrap_err()
            .iter()
            .any(|e| e.message.contains("missing fields"))
    );
}

#[test]
fn native_context_schemas_survive_without_exported_aliases_or_managed_call_sites() {
    let package = Package::new(
        "sdk",
        "context",
        &[(
            "context",
            r#"
class PrivateEditor{value:Int}
native{
    fn input(context:{editor:PrivateEditor,text:String})->Int symbol "context_input";
    fn callback(handler:fn({event:Int})->Int)->Int symbol "context_callback";
}
fn publicApi()->Int{return 0;}
export publicApi;
"#,
        )],
    );
    let (_, artifact) = check_package(&package.0).unwrap();
    let source = artifact.source_interface.unwrap();
    assert!(source.type_aliases.is_empty());
    let contexts: Vec<_> = source
        .signatures
        .iter()
        .flat_map(|signature| &signature.parameters)
        .filter_map(|abi| match abi {
            rinzu_compiler::SourceAbiType::UserData(name) => Some(name),
            rinzu_compiler::SourceAbiType::Callback { parameters, .. } => {
                match parameters.first() {
                    Some(rinzu_compiler::SourceAbiType::UserData(name)) => Some(name),
                    _ => None,
                }
            }
            _ => None,
        })
        .collect();
    assert_eq!(contexts.len(), 2);
    for context in contexts {
        assert_eq!(source.types[context].kind, "record");
        assert!(
            !source
                .modules
                .values()
                .any(|exports| exports.values().any(|value| value == context))
        );
    }
    assert!(
        source
            .types
            .keys()
            .any(|name| name.ends_with("PrivateEditor"))
    );
}

#[test]
fn value_constants_cross_application_modules_with_nominal_identity() {
    let package = Package::new(
        "application",
        "main",
        &[
            (
                "main",
                "import helper; fn main() {let config=helper.CONFIG;let title=config.title;let enabled=helper.ENABLED;}",
            ),
            (
                "helper",
                "class Config {const title:String} let CONFIG:Config=Config(title:\"module\");let ENABLED=true; export Config; export CONFIG; export ENABLED;",
            ),
        ],
    );
    check_package(&package.0).unwrap();
}

struct Package(PathBuf);

#[test]
fn supplant_facades_retain_dependency_contracts_without_publishing_them() {
    let base = Package::new(
        "sdk",
        "api",
        &[(
            "api",
            "type Context={value:Int};native{supplant fn invoke(value:Context,handler:fn(Context)->Int)->Int symbol \"invoke\";}export Context;export invoke;",
        )],
    );
    base.name("base");
    let facade = Package::new(
        "sdk",
        "api",
        &[(
            "api",
            "import base;async fn exercise(value:base.Context)->Int{let extra:Int=1;return await base.invoke(value,fn(context:base.Context)->Int fallback -1{return context.value+extra;});}export exercise;export base.Context;",
        )],
    );
    facade.name("facade");
    facade.depend("base", &base);
    let (_, artifact) = check_package(&facade.0).unwrap();
    let source = artifact.source_interface.unwrap();
    assert!(source.signatures.is_empty());
    assert!(
        source
            .lowered_native_signatures
            .iter()
            .any(|s| s.name == "base::invoke" && s.is_supplant)
    );
    let callbacks = source
        .lowered_functions
        .iter()
        .flat_map(|f| &f.blocks)
        .flat_map(|b| &b.operations)
        .filter_map(|op| match &op.kind {
            rinzu_compiler::OperationKind::ReserveSupplant { callbacks, .. } => Some(callbacks),
            _ => None,
        })
        .flatten();
    let mut captured = 0;
    for callback in callbacks {
        if let Some(rinzu_compiler::Type::Named(name)) = &callback.capture {
            assert!(source.types.contains_key(name), "{name}");
            captured += 1;
        }
    }
    assert!(captured > 0);
}

#[test]
fn sdk_interfaces_keep_generated_callback_declarations_and_capture_schemas() {
    for (native, function, entry) in [("fn", "fn", ""), ("supplant fn", "async fn", "await ")] {
        let code = format!(
            r#"
        native {{{native} invoke(handler:fn()->Int)->Int symbol "invoke";}}
        class Counter {{value:Int}}
        {function} exercise()->Int {{
            let counter=Counter(value:1);
            return {entry}invoke(fn()->Int {{counter.value=counter.value+1;return counter.value;}});
        }}
        export exercise;
    "#
        );
        let package = Package::new("sdk", "api", &[("api", &code)]);
        let (_, artifact) = check_package(&package.0).unwrap();
        let source = artifact.source_interface.unwrap();
        let callbacks = source
            .lowered_functions
            .iter()
            .flat_map(|function| &function.blocks)
            .flat_map(|block| &block.operations)
            .filter_map(|operation| match &operation.kind {
                rinzu_compiler::OperationKind::PrepareNativeCall { callbacks, .. }
                | rinzu_compiler::OperationKind::ReserveSupplant { callbacks, .. } => {
                    Some(callbacks)
                }
                _ => None,
            })
            .flatten()
            .collect::<Vec<_>>();
        assert!(!callbacks.is_empty());
        for callback in callbacks {
            for target in &callback.targets {
                assert!(
                    source
                        .functions
                        .iter()
                        .any(|function| function.declaration.name == *target),
                    "missing generated callback: {target}"
                );
            }
            let Some(rinzu_compiler::Type::Named(capture)) = &callback.capture else {
                panic!("missing capture")
            };
            assert!(
                source.types.contains_key(capture),
                "missing capture schema: {capture}"
            );
        }
        assert!(
            !source
                .modules
                .values()
                .any(|exports| exports.contains_key("Counter"))
        );
    }
}
#[test]
fn exported_native_reference_markers_cannot_be_recovered_through_aliases_or_wrappers() {
    let package = Package::new(
        "application",
        "main",
        &[
            ("main", "import model;fn main(){}"),
            (
                "model",
                "native{record Point{x:Int}opaque Handle;} \
                 type Alias=Point;class Holder{const point:Point} \
                 enum Choice{empty,point(value:Point)}type Context={point:Point}; \
                 export Point;export Handle;export Alias;export Holder;export Choice;export Context;",
            ),
        ],
    );
    check_package(&package.0).unwrap();
    for owner in [
        "model.Point",
        "model.Handle",
        "model.Alias",
        "model.Holder",
        "model.Choice",
        "model.Context",
    ] {
        for marker in ["Equality", "Key", "Transferable"] {
            std::fs::write(
                package.0.join("src/main.rinzu"),
                format!(
                    "import model;fn require<T:{marker}>(value:T){{}} \
                     fn probe(value:{owner}){{require<{owner}>(value);}}fn main(){{}}"
                ),
            )
            .unwrap();
            let diagnostics = check_package(&package.0).unwrap_err();
            assert!(
                diagnostics.iter().any(|diagnostic| {
                    diagnostic.message.contains(&format!("sealed `{marker}`"))
                }),
                "{owner} unexpectedly satisfied {marker}: {diagnostics:?}"
            );
        }
    }
}

#[test]
fn ordinary_declarations_require_export_between_modules() {
    for usage in [
        "let x=helper.hidden();",
        "let x:helper.Hidden=helper.Hidden(x:1);",
        "let x=helper.secret;",
    ] {
        let package = Package::new(
            "application",
            "main",
            &[
                ("main", &format!("import helper;fn main(){{{usage}}}")),
                (
                    "helper",
                    "class Hidden {const x:Int}let secret=1;fn hidden()->Int{return secret;}",
                ),
            ],
        );
        assert!(check_package(&package.0).is_err(), "accepted {usage}");
    }
}

#[test]
fn public_source_templates_specialize_at_the_caller_and_keep_constraints() {
    let sdk = Package::new(
        "sdk",
        "fixture",
        &[(
            "fixture",
            "class Box<T:Equality>{const value:T}enum Choice<T>{Empty,Value(value:T)}fn id<T>(value:T)->T{return value;}fn fallbackValue<T>(value:T)->Int fallback 7{return 7;}export Box;export Choice;export id;export fallbackValue;",
        )],
    );
    sdk.name("templates");
    let (_, artifact) = check_package(&sdk.0).unwrap();
    let source = artifact.source_interface.unwrap();
    assert_eq!(source.templates.len(), 4);
    assert_eq!(
        source.templates["fallbackValue"].signature,
        "fn(T)->Int fallback 7"
    );
    assert_eq!(source.templates["Box"].type_parameters[0].1, ["Equality"]);
    assert!(artifact.ir.functions.is_empty());
    let app = Package::new(
        "application",
        "main",
        &[(
            "main",
            "import templates;fn main(){let n=templates.id<Int>(42);let b=templates.Box<Int>(value:n);let c=templates.Choice<String>.Value(value:\"yes\");}",
        )],
    );
    app.depend("templates", &sdk);
    let (_, artifact) = check_package(&app.0).unwrap();
    assert!(
        artifact
            .ir
            .functions
            .iter()
            .any(|f| f.name == "templates::id<Int>")
    );
    std::fs::write(app.0.join("src/main.rinzu"), "import templates;class Mutable{value:Int}fn main(){let b=templates.Box<Mutable>(value:Mutable(value:1));}").unwrap();
    assert!(
        check_package(&app.0)
            .unwrap_err()
            .iter()
            .any(|e| e.message.contains("Equality"))
    );
    std::fs::write(sdk.0.join("src/fixture.rinzu"), "class Secret{const value:Int}fn leak<T>(value:T)->Secret{return Secret(value:1);}export leak;").unwrap();
    assert!(
        check_package(&sdk.0)
            .unwrap_err()
            .iter()
            .any(|e| e.message.contains("private type"))
    );
}

#[test]
fn public_generic_errors_export_their_payload_and_preserve_boundaries() {
    let sdk = Package::new(
        "sdk",
        "fixture",
        &[(
            "fixture",
            "error Failed<T>{payload:T}error Optional<T>{payload?:T}error Empty<T>{}export Failed;export Optional;export Empty;",
        )],
    );
    sdk.name("genericerrors");
    let (_, artifact) = check_package(&sdk.0).unwrap();
    let interface = artifact.source_interface.unwrap();
    let required = &interface.templates["Failed"];
    assert_eq!(required.kind, "error");
    assert_eq!(required.signature, "error(payload:T)");
    assert_eq!(required.fields[4], ("payload".into(), "T".into()));
    assert_eq!(required.const_fields, [0, 1, 2, 3, 4]);
    let optional = &interface.templates["Optional"];
    assert_eq!(optional.signature, "error(payload?:T)");
    assert_eq!(optional.fields[4].1, "Option<T>");
    assert_eq!(interface.templates["Empty"].signature, "error()");
    let app = Package::new(
        "application",
        "main",
        &[(
            "main",
            r#"
        import genericerrors;
        fn make()->genericerrors.Failed<Int>{return genericerrors.Failed<Int>(message:"failed",payload:7);}
        fn raise(){throw make();}
        fn main(){
            try{raise();}catch[problem:genericerrors.Failed<Int>]{let value:Int=problem.payload;}
            let optional=genericerrors.Optional<Int>(message:"missing");
            let payload:Option<Int> = optional.payload;
        }
    "#,
        )],
    );
    app.depend("genericerrors", &sdk);
    check_package(&app.0).unwrap();
    for (body, code) in [
        (
            r#"fn main(){try{throw genericerrors.Failed<String>(message:"failed",payload:"text");}catch[genericerrors.Failed<Int>]{} }"#,
            rinzu_compiler::DiagnosticCode::NonExhaustiveCatch,
        ),
        (
            r#"async fn safe(){}fn main(){let invalid=genericerrors.Failed<Future<Unit>>(message:"bad",payload:safe());}"#,
            rinzu_compiler::DiagnosticCode::TypeMismatch,
        ),
        (
            r#"fn main(){let problem=genericerrors.Failed<Int>(message:"bad",payload:7);problem.message="changed";}"#,
            rinzu_compiler::DiagnosticCode::TypeMismatch,
        ),
        (
            r#"fn worker(){throw genericerrors.Failed<Int>(message:"bad",payload:7);}async fn main(){try{await spawn worker();}catch errors{}}"#,
            rinzu_compiler::DiagnosticCode::ErrorEscapesBoundary,
        ),
        (
            r#"native{fn invoke(action:fn()->Int)->Int symbol "invoke";}fn risky()->Int{throw genericerrors.Failed<Int>(message:"bad",payload:7);}fn main(){let value=invoke(risky);}"#,
            rinzu_compiler::DiagnosticCode::ErrorEscapesBoundary,
        ),
    ] {
        std::fs::write(
            app.0.join("src/main.rinzu"),
            format!("import genericerrors;{body}"),
        )
        .unwrap();
        let errors = check_package(&app.0).unwrap_err();
        assert!(
            errors.iter().any(|error| error.code == code),
            "{body}: {errors:#?}"
        );
    }
    std::fs::write(app.0.join("src/main.rinzu"), r#"import genericerrors;native{fn invoke(action:fn()->Int)->Int symbol "invoke";}fn risky()->Int fallback -1{throw genericerrors.Failed<Int>(message:"bad",payload:7);}fn main(){let value=invoke(risky);}"#).unwrap();
    check_package(&app.0).unwrap();
}
#[test]
fn methods_keep_owner_names_and_private_boundaries_across_modules() {
    let model = "class Editor {value:Int\n    fn create()->Editor {return Editor(value:0);}\n    fn edit(self:Editor,n:Int)->Int {self.value=n;return self.value;}\n    private fn hidden(self:Editor)->Int {return self.value;}\n}    export Editor;";
    let package = Package::new(
        "application",
        "main",
        &[
            (
                "main",
                "import model;fn main(){let e=model.Editor.create();let n=e.edit(42);let unbound=model.Editor.edit;let value=unbound(e,43);let f=fn(n:Int)->Int {return model.Editor.edit(e,n);};let result=f(44);}",
            ),
            ("model", model),
        ],
    );
    let (_, artifact) = check_package(&package.0).unwrap();
    assert!(
        artifact
            .ir
            .functions
            .iter()
            .any(|f| f.name == "model::Editor.edit")
    );
    for call in [
        "e.hidden()",
        "model.Editor.hidden(e)",
        "model.Editor.hidden",
        "fn()->Int {return model.Editor.hidden(e);}",
    ] {
        std::fs::write(
            package.0.join("src/main.rinzu"),
            format!("import model;fn main(){{let e=model.Editor.create();let value={call};}}"),
        )
        .unwrap();
        let errors = check_package(&package.0).unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains("private method")),
            "{call}\n{errors:#?}"
        );
    }
    std::fs::write(
        package.0.join("src/main.rinzu"),
        "import model;fn Editor.foreign(self:model.Editor){} fn main(){}",
    )
    .unwrap();
    assert!(
        check_package(&package.0)
            .unwrap_err()
            .iter()
            .any(|e| e.message.contains("same module"))
    );
}

#[test]
fn source_sdk_methods_reexport_without_flattening_or_losing_visibility() {
    let sdk = Package::new(
        "sdk",
        "fixture",
        &[
            (
                "fixture",
                "import model;export model.First;export model.Second;",
            ),
            (
                "model",
                "class First {const value:Int\n    fn create()->First{return First(value:1);}\n    fn close(self:First)->Int{return self.value;}\n    private fn secret(self:First)->Int {return self.value;}\n} class Second {const value:Int\n    fn create()->Second{return Second(value:2);}\n    fn close(self:Second)->Int{return self.value;}\n}      export First;export Second;",
            ),
        ],
    );
    let (_, artifact) = check_package(&sdk.0).unwrap();
    let interface = artifact.source_interface.unwrap();
    assert!(interface.modules["fixture"].contains_key("First.close"));
    assert!(interface.modules["fixture"].contains_key("Second.close"));
    assert!(!interface.modules["fixture"].contains_key("close"));
    let app = Package::new(
        "application",
        "main",
        &[(
            "main",
            "import fixture;fn main(){let first=fixture.First.create();let second=fixture.Second.create();let a=first.close();let b=second.close();}",
        )],
    );
    app.depend("fixture", &sdk);
    check_package(&app.0).unwrap();
    for call in [
        "first.secret()",
        "fixture.First.secret(first)",
        "fixture.First.secret",
        "fn()->Int {return fixture.First.secret(first);}",
    ] {
        std::fs::write(
            app.0.join("src/main.rinzu"),
            format!(
                "import fixture;fn main(){{let first=fixture.First.create();let result={call};}}"
            ),
        )
        .unwrap();
        assert!(
            check_package(&app.0)
                .unwrap_err()
                .iter()
                .any(|e| e.message.contains("private method")),
            "{call}"
        );
    }
}

#[test]
fn local_module_and_type_names_can_be_shadowed_by_method_receivers() {
    let package = Package::new(
        "application",
        "main",
        &[
            (
                "main",
                "import model;fn main(){let model=model.Editor.create();let Editor=model;let a=model.edit(1);let b=Editor.edit(2);}",
            ),
            (
                "model",
                "class Editor {value:Int\n    fn create()->Editor {return Editor(value:0);}\n    fn edit(self:Editor,n:Int)->Int {self.value=n;return self.value;}\n}   export Editor;",
            ),
        ],
    );
    check_package(&package.0).unwrap();
}

#[test]
fn method_export_owner_prefix_cannot_hide_a_public_module() {
    let sdk = Package::new(
        "sdk",
        "fixture",
        &[
            (
                "fixture",
                "class data {const value:Int\n    fn create()->Int {return 1;}\n}  export data;",
            ),
            ("data", "fn create()->Int {return 2;} export create;"),
        ],
    );
    sdk.append_manifest("[exports]\nmodules=[\"fixture\",\"data\"]\n");
    let errors = check_package(&sdk.0).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("data") && e.message.contains("public module prefix")),
        "{errors:#?}"
    );
}

#[test]
fn sqlite_classes_wrap_opaque_handles_and_keep_native_abi_and_method_effects() {
    use rinzu_compiler::{FileId, SourceAbiType, SourceFile, parse_source};
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../sdk/sqlite");
    let text = std::fs::read_to_string(root.join("src/sqlite.rinzu")).unwrap();
    let module = parse_source(&SourceFile::new(FileId(1), "sqlite.rinzu", &text)).unwrap();
    for name in ["Connection", "Statement"] {
        assert!(module.declarations.iter().any(|declaration| matches!(declaration,
            rinzu_compiler::ast::Declaration::Record(record) if record.name==name && record.mutable
        )));
        assert!(module.declarations.iter().any(|declaration| matches!(declaration,
            rinzu_compiler::ast::Declaration::Function(method) if method.lexical_owner.as_deref()==Some(name)
        )));
    }
    let (_, artifact) = check_package(&root).unwrap();
    let interface = artifact.source_interface.unwrap();
    let exports = &interface.modules["rinzu.sqlite"];
    for name in ["rawQuery", "rawColumn"] {
        let signature = interface
            .signatures
            .iter()
            .find(|signature| signature.name.ends_with(name))
            .unwrap();
        assert!(
            matches!(
                signature.parameters[2],
                rinzu_compiler::SourceAbiType::Record(_)
            ),
            "{signature:?}"
        );
    }
    let open = interface
        .signatures
        .iter()
        .find(|signature| signature.name.ends_with("rawOpen"))
        .unwrap();
    assert!(
        matches!(&open.result,SourceAbiType::Opaque(name) if name.ends_with("ConnectionHandle"))
    );
    let observe = interface
        .signatures
        .iter()
        .find(|signature| signature.name.ends_with("rawObserve"))
        .unwrap();
    assert!(matches!(&observe.parameters[2], SourceAbiType::UserData(_)));
    let SourceAbiType::Callback { parameters, .. } = &observe.parameters[3] else {
        panic!("missing observer callback")
    };
    assert_eq!(parameters, &observe.parameters[2..3]);
    for export in module.exports {
        assert!(exports.contains_key(&export.path.join(".")));
    }
    for (method, free) in [
        ("Connection.open", "open"),
        ("Connection.openMemory", "openMemory"),
        ("Connection.prepare", "prepare"),
        ("Connection.execute", "execute"),
        ("Connection.queryInt", "queryInt"),
        ("Connection.close", "close"),
        ("Connection.isOpen", "isOpen"),
        ("Connection.executeWithObserver", "executeWithObserver"),
        ("Statement.step", "step"),
        ("Statement.columnInt", "columnInt"),
        ("Statement.finalize", "finalize"),
        ("Statement.isOpen", "isStatementOpen"),
    ] {
        assert!(exports.contains_key(method));
        let facts = |key: &str| {
            &interface
                .functions
                .iter()
                .find(|f| f.declaration.name == exports[key])
                .unwrap()
                .facts
        };
        assert_eq!(facts(method), facts(free), "{method}");
    }
}
impl Package {
    fn append_manifest(&self, text: &str) {
        let path = self.0.join("rinzu.toml");
        std::fs::write(&path, std::fs::read_to_string(&path).unwrap() + text).unwrap();
    }
    fn name(&self, name: &str) {
        let path = self.0.join("rinzu.toml");
        std::fs::write(
            &path,
            std::fs::read_to_string(&path)
                .unwrap()
                .replace("name = \"fixture\"", &format!("name = \"{name}\"")),
        )
        .unwrap();
    }
    fn depend(&self, name: &str, dependency: &Package) {
        self.append_manifest(&format!(
            "[dependencies]\n{name} = {{ path = \"../{}\" }}\n",
            dependency.0.file_name().unwrap().to_str().unwrap()
        ));
    }
    fn new(kind: &str, entry: &str, modules: &[(&str, &str)]) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "rinzu-source-package-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("rinzu.toml"), format!(
            "[package]\nname = \"fixture\"\nlanguage = \"0.2\"\nkind = \"{kind}\"\nentry = \"src/{entry}.rinzu\"\n"
        )).unwrap();
        for (name, source) in modules {
            std::fs::write(root.join(format!("src/{name}.rinzu")), source).unwrap();
        }
        Self(root)
    }
}
impl Drop for Package {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn legacy_sdk_metadata_is_rejected_even_with_a_valid_source_entry_and_existing_json() {
    for key in ["exports", "binding", "host-adapter"] {
        let sdk = Package::new(
            "sdk",
            "fixture",
            &[(
                "fixture",
                "import rinzu.timer;\nasync fn timer(milliseconds: Int) -> Unit { await rinzu.timer.sleep(milliseconds); }\nnative {  } export timer;",
            )],
        );
        std::fs::write(sdk.0.join("legacy.json"), "{}").unwrap();
        sdk.append_manifest(&format!("[native]\n{key} = \"legacy.json\"\n"));
        let application = Package::new(
            "application",
            "main",
            &[(
                "main",
                "import fixture; async fn main() {await fixture.timer(0);}",
            )],
        );
        application.depend("fixture", &sdk);
        for path in [&sdk.0, &application.0] {
            let diagnostics = check_package(path).unwrap_err();
            assert!(
                diagnostics.iter().any(|diagnostic| {
                    diagnostic.code == rinzu_compiler::DiagnosticCode::ManifestInvalid
                        && diagnostic
                            .message
                            .contains(&format!("unknown native key `{key}`"))
                }),
                "{key}: {diagnostics:?}"
            );
        }
    }
}

#[test]
fn module_constants_and_function_references_respect_lexical_shadowing() {
    let package = Package::new(
        "application",
        "main",
        &[
            (
                "main",
                "import helper; fn main() { let result = helper.run(); }",
            ),
            (
                "helper",
                "let width = 4; fn identity(width: Int) -> Int { return width; } fn run() -> Int { let bytes: Bytes(width) = b\"x\"; let width = width + 1; let call = identity; return call(width); } export run;",
            ),
        ],
    );
    let (_, artifact) = check_package(&package.0).unwrap();
    assert!(
        artifact
            .ir
            .functions
            .iter()
            .any(|f| f.name == "helper::identity")
    );
}

#[test]
fn sdk_modules_have_native_names_storage_context_and_no_entry_requirement() {
    let package = Package::new(
        "sdk",
        "fixture",
        &[
            (
                "fixture",
                "import implementation; export implementation.read;",
            ),
            (
                "implementation",
                "native { record Pair { x: Int } fn fill(out: Pair) -> Int symbol \"fill\"; } fn read() -> Int { let out = Pair(x: 1); let status = fill(out); return out.x; } export read;",
            ),
        ],
    );
    let (_, artifact) = check_package(&package.0).unwrap();
    assert_eq!(
        artifact.ir.source_native_signatures[0].name,
        "implementation::fill"
    );
    assert_eq!(
        artifact.syntax.exports[0].path,
        vec!["implementation::read"]
    );
}

#[test]
fn application_native_declarations_have_a_checked_owner_interface() {
    let source = rinzu_compiler::format_source(&rinzu_compiler::SourceFile::new(
        rinzu_compiler::FileId(1),
        "main.rinzu",
        "native { fn value() -> Int symbol \"value\"; } fn main() {}",
    ))
    .unwrap();
    let package = Package::new("application", "main", &[("main", &source)]);
    let (_, artifact) = check_package(&package.0).unwrap();
    let source = artifact.source_interface.unwrap();
    assert_eq!(source.signatures.len(), 1);
    assert_eq!(source.signatures[0].name, "value");
    assert!(source.modules.is_empty());
}

#[test]
fn public_modules_are_unique_exist_and_include_entry() {
    for modules in [
        "[\"fixture\", \"fixture\"]",
        "[\"other\"]",
        "[\"fixture\", \"missing\"]",
    ] {
        let package = Package::new("sdk", "fixture", &[("fixture", ""), ("other", "")]);
        let path = package.0.join("rinzu.toml");
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str(&format!("[exports]\nmodules = {modules}\n"));
        std::fs::write(path, text).unwrap();
        assert!(check_package(&package.0).is_err(), "accepted {modules}");
    }
    let package = Package::new(
        "sdk",
        "fixture",
        &[
            ("fixture", ""),
            ("other", "fn value() -> Int { return 1; } export value;"),
        ],
    );
    let path = package.0.join("rinzu.toml");
    let text =
        std::fs::read_to_string(&path).unwrap() + "[exports]\nmodules = [\"fixture\", \"other\"]\n";
    std::fs::write(path, text).unwrap();
    let (_, artifact) = check_package(&package.0).unwrap();
    assert!(
        artifact
            .ir
            .functions
            .iter()
            .any(|f| f.name == "other::value")
    );
}

#[test]
fn sdk_checks_unimported_private_source_files() {
    let package = Package::new(
        "sdk",
        "fixture",
        &[
            ("fixture", ""),
            ("private", "fn broken() -> Int { return missing; }"),
        ],
    );
    assert!(check_package(&package.0).is_err());
}

#[test]
fn entry_export_cannot_hide_public_module_prefix() {
    let package = Package::new(
        "sdk",
        "fixture",
        &[("fixture", "fn other() {} export other;"), ("other", "")],
    );
    let path = package.0.join("rinzu.toml");
    let text =
        std::fs::read_to_string(&path).unwrap() + "[exports]\nmodules = [\"fixture\", \"other\"]\n";
    std::fs::write(path, text).unwrap();
    assert!(
        check_package(&package.0)
            .unwrap_err()
            .iter()
            .any(|d| d.message.contains("conflicts"))
    );
}

#[test]
fn reexports_preserve_record_and_function_identity_through_multiple_modules() {
    let package = Package::new(
        "sdk",
        "fixture",
        &[
            (
                "fixture",
                "import facade; export facade.Pair; export facade.read; fn run() -> Int { let pair = facade.Pair(x: 7); return facade.read(pair); }",
            ),
            (
                "facade",
                "import original; export original.Pair; export original.read;",
            ),
            (
                "original",
                "class Pair { const x: Int } fn read(pair: Pair) -> Int { return pair.x; } export Pair; export read;",
            ),
        ],
    );
    let (_, artifact) = check_package(&package.0).unwrap();
    assert_eq!(
        artifact
            .syntax
            .exports
            .iter()
            .filter(|e| e.path == ["original::Pair"])
            .count(),
        3
    );
    assert_eq!(
        artifact
            .ir
            .functions
            .iter()
            .filter(|f| f.name == "original::read")
            .count(),
        1
    );
}

#[test]
fn unresolved_and_colliding_exports_are_rejected() {
    for source in [
        "export missing;",
        "import other; fn value() {} export other.value;",
        "import other; import original; export other.value; export original.value;",
    ] {
        let package = Package::new(
            "sdk",
            "fixture",
            &[
                ("fixture", source),
                ("other", "fn value() {}"),
                ("original", "fn value() {}"),
            ],
        );
        assert!(check_package(&package.0).is_err(), "accepted {source}");
    }
}

#[test]
fn public_signatures_cannot_leak_private_nominal_types() {
    for source in [
        "class Secret { const x: Int } fn expose(value: Secret) {} export expose;",
        "class Secret { const x: Int } class Public { const value: Secret } export Public;",
        "native { opaque Secret; fn expose() -> Secret symbol \"expose\"; } export expose;",
    ] {
        let package = Package::new("sdk", "fixture", &[("fixture", source)]);
        assert!(
            check_package(&package.0)
                .unwrap_err()
                .iter()
                .any(|d| d.message.contains("private type"))
        );
    }
    let package = Package::new(
        "sdk",
        "fixture",
        &[(
            "fixture",
            "let width = 4; fn expose(value: Bytes(width)) {} export expose;",
        )],
    );
    check_package(&package.0).unwrap();
}

#[test]
fn source_dependencies_link_public_modules_and_preserve_nominal_identity() {
    let sdk = Package::new(
        "sdk",
        "fixture",
        &[
            (
                "fixture",
                "import original; export original.Pair; export original.read;",
            ),
            (
                "original",
                "class Pair { const x: Int } fn read(pair: Pair) -> Int { return pair.x; } export Pair; export read;",
            ),
            ("color", "fn value() -> Int { return 2; } export value;"),
        ],
    );
    sdk.name("source_sdk");
    sdk.append_manifest("[exports]\nmodules = [\"fixture\", \"color\"]\n");
    let app = Package::new(
        "application",
        "main",
        &[(
            "main",
            "import source_sdk; import source_sdk.color; fn main() { let pair = source_sdk.Pair(x: 7); let value = source_sdk.read(pair); let color = source_sdk.color.value(); }",
        )],
    );
    app.depend("source_sdk", &sdk);
    let (_, artifact) = check_package(&app.0).unwrap();
    assert!(
        artifact
            .ir
            .functions
            .iter()
            .any(|function| function.name == "source_sdk::original::read")
    );
    assert!(artifact.sdk_packages[0].source.is_some());
    for source in [
        "import source_sdk.original; fn main() {}",
        "import source_sdk; fn main() { let value = source_sdk.private(); }",
    ] {
        std::fs::write(app.0.join("src/main.rinzu"), source).unwrap();
        assert!(check_package(&app.0).is_err());
    }
}

#[test]
fn cross_package_reexports_share_original_nominal_type() {
    let original = Package::new(
        "sdk",
        "fixture",
        &[(
            "fixture",
            "class Pair { const x: Int } fn read(pair: Pair) -> Int { return pair.x; } export Pair; export read;",
        )],
    );
    original.name("original_sdk");
    let facade = Package::new(
        "sdk",
        "fixture",
        &[(
            "fixture",
            "import original_sdk; export original_sdk.Pair; export original_sdk.read;",
        )],
    );
    facade.name("facade_sdk");
    facade.depend("original_sdk", &original);
    let app = Package::new(
        "application",
        "main",
        &[(
            "main",
            "import facade_sdk; fn main() { let pair = facade_sdk.Pair(x: 1); let value = facade_sdk.read(pair); }",
        )],
    );
    app.depend("facade_sdk", &facade);
    let (_, artifact) = check_package(&app.0).unwrap();
    assert_eq!(
        artifact
            .ir
            .functions
            .iter()
            .filter(|function| function.name == "original_sdk::read")
            .count(),
        1
    );
    std::fs::write(
        app.0.join("src/main.rinzu"),
        "import original_sdk; fn main() {}",
    )
    .unwrap();
    assert!(
        check_package(&app.0).is_err(),
        "transitive dependency became implicitly importable"
    );
}

#[test]
fn linking_retains_export_and_callback_graphs_but_checks_unreachable_bodies() {
    let sdk = Package::new(
        "sdk",
        "fixture",
        &[(
            "fixture",
            "fn callback(value: Int) -> Int { return value; } fn exported() -> Int { let call = callback; return call(1); } fn unused() {} export exported;",
        )],
    );
    let (_, artifact) = check_package(&sdk.0).unwrap();
    let names: Vec<_> = artifact
        .ir
        .functions
        .iter()
        .map(|function| function.name.as_str())
        .collect();
    assert!(names.contains(&"callback"));
    assert!(names.contains(&"exported"));
    assert!(!names.contains(&"unused"));
    std::fs::write(
        sdk.0.join("src/fixture.rinzu"),
        "fn exported() {} export exported; fn unused() { missing(); }",
    )
    .unwrap();
    assert!(check_package(&sdk.0).is_err());
}

#[test]
fn toolchain_core_import_needs_no_dependency_and_cannot_be_replaced() {
    let package = Package::new(
        "application",
        "main",
        &[(
            "main",
            "import rinzu.core; fn main() { let size = rinzu.core.utf8ByteLength(\"x\"); let alias = utf8ByteLength; let second = alias(\"y\"); }",
        )],
    );
    check_package(&package.0).unwrap();
    package.append_manifest("[dependencies]\nrinzu.core = { path = \"../core\" }\n");
    assert!(
        check_package(&package.0)
            .unwrap_err()
            .iter()
            .any(|d| d.message.contains("cannot be replaced"))
    );
}

#[test]
fn native_input_arrays_are_normalized_and_paths_stay_within_sdk_root() {
    let sdk = Package::new("sdk", "fixture", &[("fixture", "")]);
    std::fs::create_dir_all(sdk.0.join("native/include")).unwrap();
    std::fs::write(sdk.0.join("native/shim.m"), "").unwrap();
    std::fs::write(sdk.0.join("native/include/api.h"), "").unwrap();
    std::fs::write(sdk.0.join("native/library.a"), "").unwrap();
    sdk.append_manifest("[native]\nsources = [\"native/shim.m\"]\nheaders = [\"native/include/api.h\"]\nobjects = [\"native/library.a\"]\ninclude-dirs = [\"native/include/..\"]\nsearch-paths = [\"native\"]\nconfig-tools = [\"sdl2-config\"]\n");
    let (manifest, _) = check_package(&sdk.0).unwrap();
    assert_eq!(
        manifest.native.include_dirs,
        [std::path::PathBuf::from("native")]
    );
    assert_eq!(manifest.native.headers.len(), 1);
    assert_eq!(manifest.native.objects.len(), 1);
    let path = sdk.0.join("rinzu.toml");
    let original = std::fs::read_to_string(&path).unwrap();
    for replacement in ["../outside", "/tmp"] {
        std::fs::write(&path, original.replace("native/include/..", replacement)).unwrap();
        assert!(check_package(&sdk.0).is_err());
    }
    for tool in ["pkg-config", "sdl2-config --libs"] {
        std::fs::write(&path, original.replace("sdl2-config", tool)).unwrap();
        assert!(check_package(&sdk.0).is_err());
    }
}

#[cfg(unix)]
#[test]
fn native_input_symlink_escape_is_rejected() {
    let sdk = Package::new("sdk", "fixture", &[("fixture", "")]);
    let outside = Package::new("sdk", "fixture", &[("fixture", "")]);
    std::fs::write(outside.0.join("shim.c"), "").unwrap();
    std::os::unix::fs::symlink(outside.0.join("shim.c"), sdk.0.join("shim.c")).unwrap();
    sdk.append_manifest("[native]\nsources = [\"shim.c\"]\n");
    assert!(
        check_package(&sdk.0)
            .unwrap_err()
            .iter()
            .any(|diagnostic| diagnostic.message.contains("escapes"))
    );
}

#[test]
fn cli_native_inputs_are_saved_in_the_application_build_graph() {
    let package = Package::new("application", "main", &[("main", "fn main() {}")]);
    std::fs::create_dir_all(package.0.join("native")).unwrap();
    std::fs::write(package.0.join("native/shim.o"), "").unwrap();
    let mut inputs = rinzu_compiler::NativeCliInputs {
        search_paths: vec!["native/..".into()],
        libraries: vec!["objc".into()],
        objects: vec!["native/shim.o".into()],
        headers: Vec::new(),
    };
    let (_, artifact) =
        rinzu_compiler::check_package_with_native_inputs(&package.0, &inputs).unwrap();
    assert_eq!(
        artifact.native_inputs.search_paths,
        [package.0.canonicalize().unwrap()]
    );
    assert_eq!(
        artifact.native_inputs.objects,
        [package.0.join("native/shim.o").canonicalize().unwrap()]
    );
    inputs.search_paths = vec!["..".into()];
    assert!(rinzu_compiler::check_package_with_native_inputs(&package.0, &inputs).is_ok());
    inputs.search_paths.clear();
    inputs.libraries = vec!["-Wl,rpath,x".into()];
    assert!(rinzu_compiler::check_package_with_native_inputs(&package.0, &inputs).is_err());
}

#[test]
fn public_alias_retains_checked_function_effects_and_actual_fallback_target() {
    let package = Package::new(
        "sdk",
        "fixture",
        &[
            (
                "fixture",
                "import implementation; export implementation.answer;",
            ),
            (
                "implementation",
                "fn answer() -> CUInt64 fallback 18446744073709551615 { return 42; } export answer;",
            ),
        ],
    );
    let (_, artifact) = check_package(&package.0).unwrap();
    assert_eq!(artifact.ir.source_types, artifact.checked.source_types);
    let source = artifact.source_interface.unwrap();
    let function = source
        .functions
        .iter()
        .find(|function| function.declaration.name == "implementation::answer")
        .unwrap();
    assert!(function.facts.worker_safe);
    assert!(function.facts.cleanup_safe);
    assert_eq!(
        source.modules["fixture"]["answer"],
        function.declaration.name
    );
    assert_eq!(
        function.callback_fallback,
        Some(rinzu_compiler::FallbackConstant::Integer {
            scalar: rinzu_compiler::AbiScalar::U64,
            value: u64::MAX as i128
        })
    );
}

#[test]
fn managed_snapshots_of_native_records_remain_transferable() {
    for api in [
        include_str!("../../driver/tests/fixtures/native_record_sdk/api.rinzu"),
        "native { record Pair { x: Int } } type Snapshot = { x: Int }; \
         class Sample { const value: Snapshot } \
         fn read(pair: Pair) -> Sample { return Sample(value: { x = pair.x }); }",
    ] {
        let source = format!(
            "{api}\nfn requireSnapshot<T:Equality+Transferable>(value:T){{}} \
             fn checkSample(value:Sample){{requireSnapshot<Sample>(value);}}"
        );
        let package = Package::new("sdk", "fixture", &[("fixture", &source)]);
        check_package(&package.0).unwrap();
    }
}

#[test]
fn public_structural_aliases_retain_the_checked_canonical_schema_for_artifacts() {
    let package = Package::new(
        "sdk",
        "fixture",
        &[(
            "fixture",
            "native{record Point{x:Int}}type Snapshot={x:Int}; \
             fn snapshot(point:Point)->Snapshot{return {x=point.x};} \
             export Point;export Snapshot;export snapshot;",
        )],
    );
    let (_, artifact) = check_package(&package.0).unwrap();
    let source = artifact.source_interface.unwrap();
    let alias = &source.modules["fixture"]["Snapshot"];
    let rinzu_compiler::Type::Named(target) = &source.type_aliases[alias] else {
        panic!("expected a concrete structural record alias");
    };
    assert_eq!(source.types[target].kind, "record");
    assert_eq!(source.types[target].fields[0].0, "x");
}

#[test]
fn public_type_schemas_have_resolved_aliases_constants_and_original_identity() {
    let package = Package::new(
        "sdk",
        "fixture",
        &[
            (
                "fixture",
                "import implementation; export implementation.Packet; export implementation.Result; export implementation.Point; export implementation.Handle;",
            ),
            (
                "implementation",
                "let width = 2 + 2; class Packet { const data: Bytes(width), const count: CSize } enum Result { success(value: Packet), empty } native { record Point { x: CInt64 } opaque Handle; } export Packet; export Result; export Point; export Handle;",
            ),
        ],
    );
    let (_, artifact) = check_package(&package.0).unwrap();
    assert_eq!(artifact.ir.source_types, artifact.checked.source_types);
    let source = artifact.source_interface.unwrap();
    assert_eq!(
        source.types["implementation::Packet"].fields,
        vec![
            ("data".to_owned(), rinzu_compiler::Type::FixedBytes(4)),
            (
                "count".to_owned(),
                rinzu_compiler::Type::Scalar(rinzu_compiler::AbiScalar::U64)
            )
        ]
    );
    assert_eq!(
        source.types["implementation::Point"].fields[0].1,
        rinzu_compiler::Type::Int
    );
    assert_eq!(source.types["implementation::Handle"].kind, "opaque");
    assert_eq!(
        source.types["implementation::Result"].variants[0].1[0].1,
        rinzu_compiler::Type::Named("implementation::Packet".to_owned())
    );
}

#[test]
fn exported_context_aliases_retain_the_physical_closure_schemas_of_function_fields() {
    let package = Package::new(
        "sdk",
        "context",
        &[(
            "context",
            r#"
type Context={apply:fn(Int)->Int};
native{fn input(context:Context)->Int symbol "context_input";}
fn action(value:Int)->Int{return value;}
fn make()->Context{return {apply=action};}
export Context;export make;
"#,
        )],
    );
    let (_, artifact) = check_package(&package.0).unwrap();
    let source = artifact.source_interface.unwrap();
    let rinzu_compiler::SourceAbiType::UserData(identity) = &source.signatures[0].parameters[0]
    else {
        panic!("context ABI missing");
    };
    let rinzu_compiler::Type::Named(closure) = &source.types[identity].fields[0].1 else {
        panic!("expected physical closure field");
    };
    assert_eq!(source.types[closure].kind, "state");
    for (_, ty) in &source.types[closure].fields {
        if let rinzu_compiler::Type::Named(name) = ty {
            assert!(source.types.contains_key(name));
        }
    }
}
