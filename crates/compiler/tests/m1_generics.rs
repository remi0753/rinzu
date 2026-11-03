use rinzu_compiler::{FileId, SourceFile, Type, compile_source, format_source};

fn file(source: &str) -> SourceFile {
    SourceFile::new(FileId(1), "generic.rinzu", source)
}

#[test]
fn concrete_instances_have_distinct_span_keys_and_only_concrete_ir() {
    let artifact = compile_source(&file(
        r#"
class Box<T> { const value: T }
enum Choice<T> { Empty, Value(value: T) }
fn identity<T>(value: T) -> T { return value; }
fn repeat<T>(value: T, count: Int) -> T {
    if count == 0 { return value; }
    return repeat<T>(value, count - 1);
}
fn main() {
    let integer = identity<Int>(42);
    let text = identity<String>("text");
    let nested = Box<Box<Int>>(value: Box<Int>(value: repeat<Int>(integer, 3)));
    let choice = Choice<String>.Value(value: text);
    switch choice {
        case Choice.Value(payload): {}
        case Choice.Empty: {}
    }
}
"#,
    ))
    .unwrap();
    let integers = artifact
        .checked
        .functions
        .iter()
        .find(|f| f.declaration.name == "identity<Int>")
        .unwrap();
    let strings = artifact
        .checked
        .functions
        .iter()
        .find(|f| f.declaration.name == "identity<String>")
        .unwrap();
    assert_eq!(integers.result_type, Type::Int);
    assert_eq!(strings.result_type, Type::String);
    assert_ne!(
        integers.declaration.body.range,
        strings.declaration.body.range
    );
    assert_eq!(
        integers.declaration.body.range.start_byte,
        strings.declaration.body.range.start_byte
    );
    assert_eq!(
        artifact
            .ir
            .functions
            .iter()
            .filter(|f| f.name == "repeat<Int>")
            .count(),
        1
    );
    assert!(artifact.ir.source_types.contains_key("Box<Box<Int>>"));
    assert!(
        artifact
            .checked
            .syntax
            .declarations
            .iter()
            .all(|d| match d {
                rinzu_compiler::ast::Declaration::Function(f) => f.type_parameters.is_empty(),
                rinzu_compiler::ast::Declaration::Record(r) => r.type_parameters.is_empty(),
                rinzu_compiler::ast::Declaration::Enum(e) => e.type_parameters.is_empty(),
                _ => true,
            })
    );
}

#[test]
fn generic_constraints_arity_and_growth_are_diagnosed_before_ir() {
    for source in [
        "class Box<T> { const value: T } fn main() { let b: Box = Box<Int>(value: 1); }",
        "fn id<T>(value: T) -> T { return value; } fn main() { let b = id<Int, String>(1); }",
        "fn id<T, T>(value: T) -> T { return value; } fn main() {}",
        "fn id<T: Safe>(value: T) -> T { return value; } fn main() {}",
        "fn id<T: Key>(value: T) -> T { return value; } fn main() { let b = id<Float>(1.0); }",
        "fn grow<T>(value: T) { grow<List<T>>([value]); } fn main() { grow<Int>(1); }",
        "fn empty<T>() -> Option<T> { return Option<T>.None(); } fn main() { let b = empty(); }",
    ] {
        assert!(compile_source(&file(source)).is_err(), "{source}");
    }
}

#[test]
fn ordinary_less_than_and_generic_formatting_remain_unambiguous() {
    let source = file(
        "class Box<T: Transferable + Equality> { const value: T } fn main() { let x = 1; if x < 2 { let b = Box<Int>(value: x); } }",
    );
    let once = format_source(&source).unwrap();
    let twice = format_source(&file(&once)).unwrap();
    assert_eq!(once, twice);
    compile_source(&file(&once)).unwrap();
}

#[test]
fn scalar_aliases_and_fixed_bytes_constants_share_concrete_identity() {
    let artifact = compile_source(&file(
        r#"
let N = 4;
class Box<T> { const value: T }
fn id<T>(value: T) -> T { return value; }
fn main() {
    let a: Box<Int> = Box<CInt64>(value: id<CInt64>(1));
    let b: Box<Bytes(4)> = Box<Bytes(N)>(value: b"a");
}

"#,
    ))
    .unwrap();
    assert!(artifact.ir.source_types.contains_key("Box<Int>"));
    assert!(artifact.ir.source_types.contains_key("Box<Bytes(4)>"));
    assert_eq!(
        artifact
            .ir
            .functions
            .iter()
            .filter(|f| f.name == "id<Int>")
            .count(),
        1
    );
}

#[test]
fn sealed_constraints_report_nested_concrete_fields_and_variants() {
    for (marker, field, path) in [
        ("Transferable", "Future<Unit>", "Wrapper<Hidden>.value.task"),
        ("Equality", "Future<Unit>", "Wrapper<Hidden>.value.task"),
        (
            "Key",
            "Float",
            "Tagged<Hidden>.Item.value.task -> `Float` lacks Key",
        ),
    ] {
        let aggregate = if marker == "Key" { "Tagged" } else { "Wrapper" };
        let source = format!(
            "class Hidden {{ const task: {field} }} class Wrapper<T> {{ const value: T }} enum Tagged<T> {{ Item(value: T) }} fn require<T: {marker}>(value: T) {{}} fn probe(value: {aggregate}<Hidden>) {{ require<{aggregate}<Hidden>>(value); }} fn main() {{}}"
        );
        let diagnostics = compile_source(&file(&source)).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d.message.contains(path)),
            "{diagnostics:#?}"
        );
    }
    let source = "class Wrapper<T> { const value: T } fn work(value: Wrapper<Future<Unit>>) {} async fn child() {} async fn main() { let pending = child(); let value = Wrapper<Future<Unit>>(value: pending); let result = spawn work(value); await result; }";
    let diagnostics = compile_source(&file(source)).unwrap_err();
    assert!(diagnostics.iter().any(|d| {
        d.message
            .contains("Wrapper<Future<Unit>>.value -> `Future<Unit>` lacks Transferable")
    }));
}

#[test]
fn sealed_properties_are_derived_for_valid_concrete_enum_graphs() {
    compile_source(&file(
        r#"
enum Token<T> { Empty, Value(value: T) }
fn same<T: Transferable + Equality + Key>(a: T, b: T) -> Bool { return a == b; }
fn main() {
    let a = Token<Int>.Value(value: 1);
    let equal = same<Token<Int>>(a, Token<Int>.Value(value: 1));
    let table: Map<Token<Int>, String> = {a: "one"};
}
"#,
    ))
    .unwrap();
}

#[test]
fn generic_byte_lengths_respect_lexical_shadowing_and_keep_full_csize_range() {
    for source in [
        "let size = 4; class Box<T> { const value: T } fn body(size: Int) { let x = Box<Bytes(size)>(value: b\"\"); } fn main() {}",
        "let size = 4; class Box<T> { const value: T } fn main() { var size = 2; let x = Box<Bytes(size)>(value: b\"\"); }",
    ] {
        assert!(compile_source(&file(source)).is_err());
    }
    compile_source(&file("class Box<T> { const value: T } fn main() { let width = 2; let value: Box<Bytes(2)> = Box<Bytes(width)>(value: b\"ab\"); }")).unwrap();
    compile_source(&file("class Box<T> { const value: T } fn body(value: Box<Bytes(18446744073709551615)>) {} fn main() {}")).unwrap();
}
