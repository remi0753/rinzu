use rinzu_compiler::{FileId, SourceFile, Type, compile_source, format_source};
fn file(text: &str) -> SourceFile {
    SourceFile::new(FileId(1), "records.rinzu", text)
}

#[test]
fn transparent_structural_records_infer_types_across_locals_arguments_and_results() {
    let text = r#"
type Point = {x:Int,y:Int};
type Same = {y:Int,x:Int};
type Number = Int;
type T = Int;
type Box<T> = {value:T};
fn read(p:Point)->Number {return p.x;}
fn create()->Same {return {y=2,x=1};}
fn generic<T>(value:T)->Box<T> {return {value=value};}
fn identity<T>(value:T)->T {return value;}
fn idInt(value:Int)->Int {return value;}
class Counter {value:Int}
let origin:Point={x=0,y=0};
let named={name="origin",number:Int=1};
fn main() {
    let initial=origin;
    let label=named.name;
    let a:Point={y=2,x=1};
    let b:Same={x=1,y=2};
    let copy=a;
    let x:Int=1;
    let inferred={x=x,y:Int=2};
    let n=read({x=1,y=2});
    let c=create();
    let equal=a==b;
    let same=a==inferred;
    let inner=generic<Int>(1);
    let other=generic<Bool>(true);
    let number=identity<{x:Int,y:Int}>({x=3,y=4});
    let nested={child={flag=true}};
    let values:Map<{x:Int,y:Int},String> = {a:"point"};
    let counter=Counter(value:0);
    let holder={counter=counter};
    holder.counter.value=1;
    let action={apply:fn(Int)->Int=idInt};
    let result=action.apply(1);
}
"#;
    let formatted = format_source(&file(text)).unwrap();
    assert_eq!(formatted, format_source(&file(&formatted)).unwrap());
    let artifact = compile_source(&file(&formatted)).unwrap();
    let read = artifact
        .checked
        .functions
        .iter()
        .find(|f| f.declaration.name == "read")
        .unwrap();
    let create = artifact
        .checked
        .functions
        .iter()
        .find(|f| f.declaration.name == "create")
        .unwrap();
    assert_eq!(read.parameter_types[0], create.result_type);
    assert_eq!(read.result_type, Type::Int);
    assert!(!artifact.ir.source_types.contains_key("Point"));
    assert!(!artifact.ir.source_types.contains_key("Same"));
}

#[test]
fn invalid_structural_records_are_diagnosed_without_defaults_or_width_subtyping() {
    for (text, message) in [
        ("fn main(){let r={x=1};}", "numeric record field"),
        ("fn main(){let r={x=1.0};}", "numeric record field"),
        ("fn main(){let r={x=-1};}", "numeric record field"),
        (
            "type P={x:Int,y:Int};fn main(){let r:P={x=1};}",
            "missing fields",
        ),
        (
            "type P={x:Int};fn main(){let r:P={x=1,y:Int=2};}",
            "extra field",
        ),
        ("type P={x:Int};fn main(){let r:P={x:Float=1};}", "expected"),
        ("type P={x:Int};fn main(){let r:P={x=true};}", "expected"),
        (
            "fn main(){let r={x:Int=1,x:Int=2};}",
            "duplicate record field",
        ),
        ("type P={x:Int,x:Int};fn main(){}", "duplicate field"),
        ("fn main(){let r:{x:Int}={};}", "missing fields"),
        ("type Empty={};fn main(){}", "record type field"),
        ("type Loop=Loop;fn main(){}", "recursive"),
        ("type A=B;type B=A;fn main(){}", "recursive"),
        (
            "type Box<T> = {value:T};fn main(){let r:Box={value:Int=1};}",
            "explicit type arguments",
        ),
        ("fn main(){let r={x:Int=1};r.x=2;}", "immutable record"),
    ] {
        let errors = compile_source(&file(text)).unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains(message)),
            "{text}: {errors:?}"
        );
    }
    assert!(compile_source(&file("fn main(){let empty={};}")).is_err());
    compile_source(&file(
        "fn main(){let map:Map<Int,String> = {};let n=1;let r={n=n,flag=true,text=\"ok\"};}",
    ))
    .unwrap();
}
