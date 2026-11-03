use rinzu_compiler::{FileId, SourceFile, compile_source, format_source};
fn file(s: &str) -> SourceFile {
    SourceFile::new(FileId(1), "numeric.rinzu", s)
}
#[test]
fn radix_and_pure_bit_constants_keep_width_and_formatter_spelling() {
    let text = include_str!("../../../tests/fixtures/programs/numeric/src/main.rinzu");
    let formatted = format_source(&file(text)).unwrap();
    assert!(formatted.contains("0b0000_1111"));
    assert!(formatted.contains("0xAA"));
    assert_eq!(formatted, format_source(&file(&formatted)).unwrap());
    compile_source(&file(&formatted)).unwrap();
}
#[test]
fn numeric_type_width_and_radix_rejections_are_frontend_diagnostics() {
    for text in [
        "fn main() {let x=bitAnd(true,false);}",
        "fn main() {let x=bitOr(CInt8(1),CUInt8(1));}",
        "fn main() {let x=shiftLeft(CInt8(1),CSize(0));}",
        "fn main() {let x: CInt8=0xff;}",
        "fn main() {let x=0x_;}",
        "fn main() {let x=0b102;}",
        "fn main() {let x=0x1__2;}",
        "fn main() {let x=0x10000000000000000;}",
        "fn main() {let x=mathSqrt(1);}",
        "let X=checkedAdd(1,2); fn main() {}",
    ] {
        assert!(compile_source(&file(text)).is_err(), "{text}");
    }
}
