use rinzu_compiler::{FileId, SourceFile, compile_source, format_source};
fn file(s: &str) -> SourceFile {
    SourceFile::new(FileId(1), "constants.rinzu", s)
}
#[test]
fn pure_constants_typecheck_unused_initializers_and_preserve_spelling() {
    let text = include_str!("../../../tests/fixtures/programs/constants/src/main.rinzu");
    let formatted = format_source(&file(text)).unwrap();
    assert_eq!(formatted, format_source(&file(&formatted)).unwrap());
    assert!(formatted.contains("0b1111"));
    assert!(formatted.contains("r#\"日本語 {name}\"#"));
    compile_source(&file(&formatted)).unwrap();
    for text in [
        "let UNUSED:String=1; fn main() {}",
        "let A:String=B; let B:String=A; fn main() {}",
        "let A:List<Int>=[]; fn main() {}", // >= remains one token without whitespace
        "let A:List<Int> = [\"bad\"]; fn main() {}",
        "class R {const x:Int} let A:R=R(x:1/0); fn main() {}",
        "fn io()->String {return \"runtime\";} let A:String=io(); fn main() {}",
        "let A:String=utf8Decode(byteArray(b\"A\")); fn main() {}",
        "let A=spawn work(); fn work() {} fn main() {}",
        "let A=[1,2]; fn main() {}",
        "let A=f\"value={1}\"; fn main() {}",
        "let A:Float=0.5; fn main() {}",
    ] {
        assert!(compile_source(&file(text)).is_err(), "{text}");
    }
}
#[test]
fn declaration_order_aliases_and_local_shadowing_remain_lexical() {
    compile_source(&file("let A=B; let B=true; let TEXT=\"module\"; fn use(TEXT:String)->String {return TEXT;} fn main() {let before=A;let A=false;let after=A;let value=use(TEXT);}" )).unwrap();
}
