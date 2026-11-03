use rinzu_compiler::{FileId, SourceFile, compile_source, format_source};
fn file(s: &str) -> SourceFile {
    SourceFile::new(FileId(1), "text.rinzu", s)
}
#[test]
fn text_operations_return_direct_values_and_format_idempotently() {
    let text = include_str!("../../../tests/fixtures/programs/text/src/main.rinzu");
    let formatted = format_source(&file(text)).unwrap();
    assert_eq!(formatted, format_source(&file(&formatted)).unwrap());
    let artifact = compile_source(&file(&formatted)).unwrap();
    assert!(
        !artifact
            .ir
            .source_types
            .keys()
            .any(|ty| ty.starts_with("Result<"))
    );
    assert!(artifact.ir.source_types.contains_key("Option<String>"));
}
#[test]
fn string_byte_indices_and_argument_types_are_checked() {
    for text in [
        "fn main() { let x = stringGet(\"x\",CSize(0)); }",
        "fn main() { let x = utf8Decode(\"text\"); }",
        "fn main() { let x = stringFind(\"x\",1,0); }",
        "fn main() { let x = formatInt(CInt8(1)); }",
    ] {
        assert!(compile_source(&file(text)).is_err(), "{text}");
    }
    compile_source(&file(
        "enum Result<T,E>{Other(value:T)} fn main(){try{let value=parseInt(\"1\");}catch errors{}}",
    ))
    .unwrap();
}
