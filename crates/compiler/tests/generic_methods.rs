use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source, format_source};
fn file(text: &str) -> SourceFile {
    SourceFile::new(FileId(309), "generic-methods.rinzu", text)
}
pub const BOX: &str = r#"
class Box<T> {
    private const item:T
    fn create(value:T)->Box<T>{return Box<T>(item:value);}
    fn get(self:Box<T>)->T{return self.item;}
    private fn raw(self:Box<T>)->T{return self.item;}
    fn map<U>(self:Box<T>,transform:fn(T)->U)->Box<U>{return Box<U>.create(transform(self.raw()));}
    async fn later(self:Box<T>)->T{return self.item;}
}
enum Value<T> {
    Some(value:T), None
    fn getOr(self:Value<T>,otherwise:T)->T {switch self {case Value.Some(value):{return value;}case Value.None:{return otherwise;}}}
    fn create(value:T)->Value<T>{return Value<T>.Some(value:value);}
}
"#;
#[test]
fn owner_and_method_arguments_infer_and_explicit_unbound_references_work() {
    let text = format!(
        r#"{BOX}
fn identity<T>(value:T)->T{{return value;}}
async fn main(){{
    let boxed=Box<Int>.create(20);
    let inferred=Box.create(21);
    let result=boxed.get();
    let next=boxed.map<String>(fn(value:Int)->String{{return f"{{value}}";}});
    let mapped=inferred.map(fn(value:Int)->String{{return f"{{value}}";}});
    let unbound=Box<Int>.map<String>;
    let viaReference=unbound(boxed,fn(value:Int)->String{{return f"{{value}}";}});
    let get=Box<Int>.get;
    let a=get(boxed);
    let id=identity<Int>;
    let b=id(42);
    let later=await boxed.later();
    let value=Value<Int>.create(9);
    let c=value.getOr(3);
}}
"#
    );
    let formatted = format_source(&file(&text)).unwrap();
    assert_eq!(formatted, format_source(&file(&formatted)).unwrap());
    compile_source(&file(&formatted)).unwrap();
    compile_source(&file("class Plain{value:Int fn copy<T>(self:Plain,value:T)->T{return value;}}fn main(){let p=Plain(value:0);let a=p.copy<Int>(1);let b=p.copy(2);let copy=Plain.copy<Int>;let c=copy(p,3);}")).unwrap();
}

#[test]
fn spawn_uses_an_unbound_generic_method_target() {
    let program = |call: &str| {
        format!(
            r#"
class Box<T> {{
    const item: T
    fn get(self: Box<T>) -> T {{ return self.item; }}
    fn pick<U>(self: Box<T>, other: U) -> T {{ return self.item; }}
}}
async fn main() {{
    let box = Box<Int>(item: 42);
    let work = spawn {call};
    let value = await work;
}}
"#
        )
    };
    for call in ["Box<Int>.get(box)", "Box<Int>.pick<Int>(box, 0)"] {
        compile_source(&file(&program(call))).unwrap();
    }
    for call in ["box.get()", "box.pick(0)", "box.pick<Int>(0)"] {
        let errors = compile_source(&file(&program(call))).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.code == DiagnosticCode::InvalidSpawn)
        );
    }
}

#[test]
fn privacy_markers_and_parameter_groups_are_not_bypassed() {
    for body in [
        "let c=Box<Int>(item:1);",
        "let c=Box.create(1);let raw=c.raw();",
        "let c=Box.create(1);let bound=c.map<String>;",
        "let c=Box.create(1);let invalid=c.map<String,Int>(fn(v:Int)->String{return \"x\";});",
    ] {
        let text = format!("{BOX} fn main(){{{body}}}");
        assert!(compile_source(&file(&text)).is_err(), "{text}");
    }
    assert!(
        compile_source(&file(
            "class C<T>{fn f<T>(value:T)->T{return value;}}fn main(){}"
        ))
        .is_err()
    );
    assert!(compile_source(&file("class C<T:Key>{fn make(value:T)->Int{return 1;}}fn main(){let n=C<List<Int>>.make([1]);}")).is_err());
}
