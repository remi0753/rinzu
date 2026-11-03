use rinzu_compiler::{FileId, SourceFile, compile_source, format_source};

fn file(text: &str) -> SourceFile {
    SourceFile::new(FileId(1), "keys.rinzu", text)
}

#[test]
fn content_key_sample_and_empty_maps_have_sealed_concrete_schemas() {
    let source = file(include_str!(
        "../../codegen/tests/fixtures/m1-keys/src/main.rinzu"
    ));
    compile_source(&source).unwrap();
    let formatted = format_source(&source).unwrap();
    compile_source(&file(&formatted)).unwrap();
    assert_eq!(formatted, format_source(&file(&formatted)).unwrap());
    compile_source(&file("class R {const flag:CBool,const bytes:Bytes(0)} enum E {empty,item(value:R),binary(value:ByteArray)} fn main() {let a:Map<Bool,Int> = {};let b:Map<CBool,Int> = {};let c:Map<Bytes(0),Int> = {};let d:Map<ByteArray,Int> = {};let e:Map<R,Int> = {};let f:Map<E,Int> = {};}")).unwrap();
}

#[test]
fn key_failures_name_record_fields_unused_variants_and_recursive_graphs() {
    for (text, expected) in [
        (
            "class Inner {const items:List<Int>} class Outer<T> {const inner:T} fn require<T:Key>(x:T){} fn bad(x:Outer<Inner>){require<Outer<Inner>>(x);} fn main(){}",
            "inner.items",
        ),
        (
            "class R {const number:Float} enum E {empty,unused(value:R)} fn main(){let x:Map<E,Int> = {};}",
            "unused.value.number",
        ),
        (
            "class R {const self:R} fn main(){let x:Map<R,Int> = {};}",
            "recursive",
        ),
        (
            "class R {const error:Error} fn main(){let x:Map<R,Int> = {};}",
            ".error",
        ),
    ] {
        let errors = compile_source(&file(text)).unwrap_err();
        assert!(
            errors.iter().any(|error| error.message.contains(expected)),
            "{errors:?}"
        );
    }
    for text in [
        "native {record R {number:CInt64}} fn main(){let x:Map<R,Int> = {};}",
        "class R {const values:Map<Int,Int>} fn main(){let x:Map<R,Int> = {};}",
        "fn main(){let x:Map<CDouble,Int> = {};}",
        "fn main(){let x:Map<Unit,Int> = {};}",
        "fn main(){let x:Map<Future<Unit>,Int> = {};}",
        "fn require<T:CustomKey>(x:T){} fn main(){}",
        "fn main(){let x:Map<Bytes(4),Int> = {b\"A\":1};let y=mapGet(x,byteArray(b\"A\"));}",
    ] {
        assert!(compile_source(&file(text)).is_err(), "{text}");
    }
}
