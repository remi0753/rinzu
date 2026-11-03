use rinzu_compiler::{FileId, SourceFile, compile_source, format_source};
fn file(s: &str) -> SourceFile {
    SourceFile::new(FileId(1), "bytes.rinzu", s)
}
#[test]
fn dynamic_length_reference_and_option_payload_are_concrete() {
    let s = file(include_str!(
        "../../../tests/fixtures/programs/bytes/src/main.rinzu"
    ));
    let formatted = format_source(&s).unwrap();
    assert_eq!(formatted, format_source(&file(&formatted)).unwrap());
    let artifact = compile_source(&file(&formatted)).unwrap();
    assert!(artifact.ir.source_types.contains_key("Option<ByteArray>"));
}
#[test]
fn byte_lengths_indices_and_native_signatures_remain_typed() {
    for s in [
        "fn main() { let x: Bytes = b\"A\"; }",
        "fn main() { let x = byteArray([1]); }",
        "fn main() { let x = bytesSlice(byteArray(b\"A\"), CSize(0), 1); }",
        // Ordinary synchronous results use NativeResult<ByteArray>; callback
        // results still cannot return dynamic bytes through the callback ABI.
        "native { fn f(callback: fn() -> ByteArray) -> Unit symbol \"f\"; } fn main() {}",
        "fn main() { let f: fn(ByteArray) -> Int = byteLength; }",
    ] {
        assert!(compile_source(&file(s)).is_err(), "{s}");
    }
}

#[test]
fn byte_searches_instantiate_their_option_without_an_explicit_constructor() {
    for operation in ["bytesFindAny", "bytesFindNotAny"] {
        let source = format!(
            "fn main() {{ switch {operation}(byteArray(b\"abc\"), byteArray(b\"b\"), 0) {{ \
             case Option.Some(index): {{ let found: Int = index; }} \
             case Option.None: {{ }} }} }}"
        );
        let artifact = compile_source(&file(&source)).unwrap();
        assert!(artifact.ir.source_types.contains_key("Option<Int>"));
    }
}

#[test]
fn byte_searches_reject_a_shadowed_prelude_option() {
    let source = "enum Option<T>{Other(value:T)} fn main(){let found=bytesFindAny(byteArray(b\"a\"),byteArray(b\"a\"),0);}";
    assert!(compile_source(&file(source)).is_err());
}

#[test]
fn boolean_byte_predicates_are_concrete_without_option_and_reject_other_types() {
    for operation in ["bytesContainsAny", "bytesAllIn"] {
        let source = format!(
            "fn main() {{ let b: Bool = {operation}(byteArray(b\"a\"), byteArray(b\"a\")); }}"
        );
        assert!(compile_source(&file(&source)).is_ok(), "{source}");
        for arguments in [
            "byteArray(b\"a\"), 1",
            "b\"a\", byteArray(b\"a\")",
            "byteArray(b\"a\")",
        ] {
            let source = format!("fn main() {{ let b = {operation}({arguments}); }}");
            assert!(compile_source(&file(&source)).is_err(), "{source}");
        }
    }
}

#[test]
fn boolean_byte_predicates_do_not_require_a_prelude_option() {
    let source = "enum Option<T>{Other(value:T)} fn main(){let b:Bool=bytesContainsAny(byteArray(b\"a\"),byteArray(b\"a\"));}";
    assert!(compile_source(&file(source)).is_ok());
}

#[test]
fn trim_returns_bytes_and_requires_two_dynamic_byte_inputs() {
    let source = "fn main(){let b:ByteArray=bytesTrim(byteArray(b\" a \"),byteArray(b\" \"));}";
    assert!(compile_source(&file(source)).is_ok());
    for arguments in [
        r#"b"a", byteArray(b" ")"#,
        r#"byteArray(b"a"), 1"#,
        r#"byteArray(b"a")"#,
    ] {
        let source = format!("fn main(){{let b=bytesTrim({arguments});}}");
        assert!(compile_source(&file(&source)).is_err(), "{source}");
    }
}

#[test]
fn concat_all_accepts_only_one_list_of_dynamic_bytes_without_error_effects() {
    for body in [
        "let pieces:List<ByteArray> = [];let joined:ByteArray=bytesConcatAll(pieces);",
        "let joined:ByteArray=bytesConcatAll([byteArray(b\"A\"),byteArray(b\"B\")]);",
    ] {
        let source = format!("fn main(){{{body}}}");
        let checked = compile_source(&file(&source));
        assert!(checked.is_ok(), "{source}: {:?}", checked.err());
    }
    for arguments in [
        "",
        "[1,2]",
        "[b\"A\"]",
        "byteArray(b\"A\")",
        "[byteArray(b\"A\")],1",
    ] {
        let source = format!("fn main(){{let joined=bytesConcatAll({arguments});}}");
        assert!(compile_source(&file(&source)).is_err(), "{source}");
    }
}
