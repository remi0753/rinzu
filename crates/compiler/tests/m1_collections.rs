use rinzu_compiler::{FileId, SourceFile, compile_source, format_source};

fn file(source: &str) -> SourceFile {
    SourceFile::new(FileId(1), "collections.rinzu", source)
}

#[test]
fn collection_results_infer_concrete_options_and_verify_ir() {
    let source = file(include_str!(
        "../../../tests/fixtures/programs/collections/src/main.rinzu"
    ));
    let once = format_source(&source).unwrap();
    assert_eq!(once, format_source(&file(&once)).unwrap());
    let compiled = compile_source(&file(&once)).unwrap();
    assert!(
        compiled
            .ir
            .source_types
            .contains_key("Option<List<String>>")
    );
    compile_source(&file("fn main() { let xs=[1,2]; switch listGet(xs,0) { case Option.Some(v): {} case Option.None: {} } }")).unwrap();
}

#[test]
fn data_operations_reject_arity_element_key_and_shadowed_option() {
    for source in [
        "fn main() { let x=listGet([1]); }",
        "fn main() { let x=listGet([1],1.0); }",
        "fn main() { let x=listAppend([1],\"wrong\"); }",
        "fn main() { let x=mapGet({\"key\":1},42); }",
        "enum Option<T> { Other(value: T) } fn main() { let x: Option<Int> = listGet([1],0); }",
    ] {
        assert!(compile_source(&file(source)).is_err(), "{source}");
    }
}
