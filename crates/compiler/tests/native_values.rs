use rinzu_compiler::{FileId, SourceFile, compile_source};

#[test]
fn native_record_snapshots_have_content_equality_keys_and_transfer_semantics() {
    let source = r#"
        native { record Pair { x: Int, y: Bool } }
        type Snapshot = { x: Int, y: Bool };
        fn worker(pair: Snapshot) -> Snapshot { return { x=pair.x + 1, y=pair.y }; }
        async fn main() {try {
            let pair = Pair(x: 1, y: true);
            let snapshot: Snapshot = { x=pair.x, y=pair.y };
            let result = await spawn worker(snapshot);
            let equal = snapshot == result;
            let list = [snapshot, result];
            let map: Map<Snapshot, Int> = { snapshot: 1 };
        } catch errors {}}
    "#;
    let artifact = compile_source(&SourceFile::new(FileId(0), "native-values.rz", source)).unwrap();
    assert!(
        artifact
            .checked
            .functions
            .iter()
            .find(|f| f.declaration.name == "worker")
            .unwrap()
            .facts
            .worker_safe
    );
}

#[test]
fn native_references_do_not_derive_markers_through_containers_or_unused_variants() {
    let prefix = "native {record Pair{x:Int,y:Bool}opaque Handle;} \
                  class Box<T>{const value:T} \
                  enum Choice<T>{empty,value(item:T)} \
                  type Context<T> = {reference:T};";
    for reference in ["Pair", "Handle"] {
        for owner in [
            reference.to_owned(),
            format!("Box<{reference}>"),
            format!("Choice<{reference}>"),
            format!("Context<{reference}>"),
            format!("List<{reference}>"),
            format!("Map<Int,{reference}>"),
        ] {
            for marker in ["Equality", "Key", "Transferable"] {
                let source = format!(
                    "{prefix} fn require<T:{marker}>(value:T){{}} \
                     fn probe(value:{owner}){{require<{owner}>(value);}} fn main(){{}}"
                );
                let diagnostics =
                    compile_source(&SourceFile::new(FileId(0), "native-markers.rinzu", source))
                        .unwrap_err();
                assert!(
                    diagnostics.iter().any(|diagnostic| {
                        diagnostic.message.contains(&format!("sealed `{marker}`"))
                    }),
                    "{owner} unexpectedly satisfied {marker}: {diagnostics:?}"
                );
            }
        }
    }
}

#[test]
fn native_record_comparison_and_cross_domain_arguments_and_results_are_rejected() {
    for body in [
        "fn probe(value:Pair){let equal=value==value;}",
        "fn probe(value:Pair){let different=value!=value;}",
        "fn probe(value:Pair){let map={value:1};}",
        "fn worker(value:Pair){} async fn probe(value:Pair){await spawn worker(value);}",
        "fn worker()->Pair{return Pair(x:1,y:true);} async fn probe(){let result=await spawn worker();}",
        "type Alias=Pair;fn probe(value:Alias){let equal=value==value;}",
    ] {
        let source = format!("native{{record Pair{{x:Int,y:Bool}}}}{body}fn main(){{}}");
        assert!(
            compile_source(&SourceFile::new(FileId(0), "native-reject.rinzu", source)).is_err(),
            "accepted {body}"
        );
    }
}

#[test]
fn native_record_references_remain_storable_and_capturable_in_the_same_domain() {
    let source = r#"
        native {record Pair{x:Int,y:Bool}}
        class Box {const value:Pair}
        type Context={value:Pair};
        fn hold(value:Pair)->Pair{return value;}
        fn main(){try{
            let pair=Pair(x:1,y:true);
            let alias=hold(pair);
            let boxed=Box(value:alias);
            let context:Context={value=boxed.value};
            let list=[context.value,alias];
            let map:Map<Int,Pair> = {1:pair};
            let capture=fn()->Pair{return boxed.value;};
            let captured=capture();
            let x=captured.x;
        }catch errors{}}
    "#;
    compile_source(&SourceFile::new(FileId(0), "native-sharing.rinzu", source)).unwrap();
}

#[test]
fn workers_can_create_and_read_local_native_records_without_transferring_references() {
    let source = "native{record Pair{x:Int,y:Bool}} \
                  fn worker()->Int{try{let local=Pair(x:7,y:true);return local.x;}catch errors{return -1;}} \
                  async fn main(){try{let value=await spawn worker();}catch errors{}}";
    compile_source(&SourceFile::new(FileId(0), "worker-local.rinzu", source)).unwrap();
}

#[test]
fn opaque_references_are_nominal_without_equality_or_transfer() {
    let prefix = r#"
        native {
            opaque Handle;
            opaque Other;
            fn acquire() -> Handle symbol "acquire";
            fn consume(h: Handle) -> Unit symbol "consume";
        }
        class Box { const value: Handle }
        fn worker(h: Handle) {}
    "#;
    let valid = format!(
        "{prefix} fn main() {{ try {{ let h = acquire(); let alias = h; consume(alias); }} catch errors {{}} }}"
    );
    compile_source(&SourceFile::new(FileId(0), "native-values.rz", valid)).unwrap();
    for body in [
        "let h = acquire(); let equal = h == h;",
        "let h = acquire(); let other: Other = h;",
        "let h = acquire(); await spawn worker(h);",
        "let h = acquire(); let map = {h: 1};",
        "let h = acquire(); let a = Box(value: h); let equal = a == a;",
    ] {
        let source = format!("{prefix} async fn main() {{ {body} }}");
        assert!(
            compile_source(&SourceFile::new(FileId(0), "native-values.rz", source)).is_err(),
            "accepted {body}"
        );
    }
}

#[test]
fn ordinary_context_records_preserve_nested_class_values_and_parameters() {
    let source = r#"
        class Editor { const value: Int }
        type Context = {editor:Editor};
        native {
            fn invoke(context: Context, callback: fn(Context) -> Int) -> Int symbol "invoke";
        }
        fn callback(context: Context) -> Int fallback -1 { return context.editor.value; }
        fn main() { let context = {editor=Editor(value:42)}; let result = invoke(context, callback); }
    "#;
    let artifact = compile_source(&SourceFile::new(FileId(0), "native-values.rz", source)).unwrap();
    assert_eq!(
        artifact.checked.source_native_signatures[0]
            .parameters
            .len(),
        2
    );
}
