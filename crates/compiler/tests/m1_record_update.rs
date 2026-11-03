use rinzu_compiler::{FileId, OperationKind, SourceFile, compile_source, format_source};
fn file(s: &str) -> SourceFile {
    SourceFile::new(FileId(1), "record-update.rinzu", s)
}
#[test]
fn field_enumeration_lowers_to_existing_field_construct_ir_and_formats_idempotently() {
    let text = include_str!("../../codegen/tests/fixtures/m1-record-update/src/main.rinzu");
    let formatted = format_source(&file(text)).unwrap();
    assert_eq!(formatted, format_source(&file(&formatted)).unwrap());
    let artifact = compile_source(&file(&formatted)).unwrap();
    assert!(
        artifact
            .ir
            .functions
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.operations)
            .any(|o| matches!(o.kind, OperationKind::Field { .. }))
    );
}
#[test]
fn removed_with_is_rejected_for_all_base_types() {
    for text in [
        "class R {const x:Int} fn main() {let r=R(x:1) with {x:2,x:3};}",
        "class R {const x:Int} fn main() {let r=R(x:1) with {y:2};}",
        "class R {const x:Int} fn main() {let r=R(x:1) with {x:\"bad\"};}",
        "fn main() {let r=1 with {};}",
        "enum E {One} fn main() {let r=E.One() with {};}",
    ] {
        assert!(compile_source(&file(text)).is_err(), "{text}");
    }
}
