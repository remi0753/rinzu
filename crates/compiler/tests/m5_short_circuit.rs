use rinzu_compiler::{FileId, SourceFile, compile_source};

#[test]
fn nested_boolean_edges_keep_enclosing_operands_and_typed_roots_valid() {
    for text in [
        r#"
class Pair { const data: ByteArray, flag: Bool }
fn yes() -> Bool { return true; }
fn keep(data: ByteArray, flag: Bool) -> ByteArray { return data; }
fn main() {
    let apply = keep;
    let data = apply(utf8Encode("日"), false || yes());
    let pair = Pair(data: data, flag: true && yes());
    pair.flag = false || yes();
    let flags = [false && yes(), true || yes(), (false || yes()) && (true || yes())];
    let formatted = f"{data}{false || yes()}";
}
"#,
        r#"
async fn yes() -> Bool { return true; }
fn keep(data: ByteArray, flag: Bool) -> ByteArray { return data; }
async fn main() {
    let consumed = await yes();
    let data = keep(utf8Encode("日"), false || await yes());
    let selected = (true && await yes()) || await yes();
}
"#,
    ] {
        compile_source(&SourceFile::new(FileId(1), "short-circuit", text)).unwrap();
    }
}
