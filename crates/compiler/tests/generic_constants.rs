use rinzu_compiler::{FileId, SourceFile, compile_source};

fn file(text: &str) -> SourceFile {
    SourceFile::new(FileId(1), "generic-constants.rinzu", text)
}

#[test]
fn generic_byte_lengths_preserve_annotated_integer_widths() {
    for (ty, maximum) in [
        ("CUInt8", "255"),
        ("CUInt16", "65535"),
        ("CUInt32", "4294967295"),
        ("CUInt64", "18446744073709551615"),
    ] {
        for (initializer, length) in [
            ("limit", "wrappingAdd(size, 1)"),
            ("wrappingAdd(limit, 1)", "size"),
        ] {
            let source = format!(
                "fn id<T>(value: T) -> T {{ return value; }}
                 fn main() {{ let limit: {ty} = {maximum}; let size: {ty} = {initializer};
                   let bytes: Bytes(0) = id<Bytes({length})>(b\"\"); }}"
            );
            let artifact = compile_source(&file(&source)).unwrap();
            assert!(
                artifact
                    .ir
                    .functions
                    .iter()
                    .any(|function| function.name == "id<Bytes(0)>")
            );
        }
    }
    for (ty, minimum) in [
        ("CInt8", "-128"),
        ("CInt16", "-32768"),
        ("CInt32", "-2147483648"),
    ] {
        let source = format!(
            "fn id<T>(value: T) -> T {{ return value; }}
             fn main() {{ let limit: {ty} = {minimum}; let size: {ty} = wrappingAdd(limit, limit);
               let bytes: Bytes(0) = id<Bytes(size)>(b\"\"); }}"
        );
        compile_source(&file(&source)).unwrap();
    }
}

#[test]
fn noninteger_annotations_cannot_supply_generic_integer_constants() {
    for ty in ["Float", "CFloat"] {
        let source = format!(
            "fn id<T>(value: T) -> T {{ return value; }}
             fn main() {{ let size: {ty} = 4; let bytes = id<Bytes(size)>(b\"\"); }}"
        );
        assert!(compile_source(&file(&source)).is_err(), "{source}");
    }
}

#[test]
fn specialization_and_aliases_preserve_local_constant_annotations() {
    let artifact = compile_source(&file(
        "type Small = CUInt8;
         fn id<T>(value: T) -> T { return value; }
         fn sized<T>() { let limit: T = 255; let size: T = wrappingAdd(limit, 1); let bytes: Bytes(0) = id<Bytes(size)>(b\"\"); }
         fn main() { let size: Small = 255; let bytes: Bytes(0) = id<Bytes(wrappingAdd(size, 1))>(b\"\"); sized<Small>(); }",
    )).unwrap();
    assert_eq!(
        artifact
            .ir
            .functions
            .iter()
            .filter(|function| function.name == "id<Bytes(0)>")
            .count(),
        1
    );
}
