use rinzu_compiler::{FileId, SourceFile, compile_source};
fn compile(
    text: &str,
) -> Result<rinzu_compiler::FrontendArtifact, Vec<rinzu_compiler::Diagnostic>> {
    compile_source(&SourceFile::new(
        FileId(304),
        "type-argument-inference.rinzu",
        text,
    ))
}
const BASE: &str = "fn identity<T>(value:T)->T{return value;} fn same<T>(first:T,second:T)->T{return first;} class Box<T>{const value:T}";

#[test]
fn arguments_expected_results_constructors_and_typed_callbacks_infer() {
    let text = format!(
        r#"{BASE}
fn transform<T,U>(value:T,callback:fn(T)->U)->U{{return callback(value);}}
fn nested()->Option<String>{{return Option.None();}}
fn take(value:Option<Int>){{}}
fn main(){{
    let value=identity(42);
    let narrow=same(7,CInt8(3));
    let nestedNarrow=same(identity(7),CInt8(3));
    let listNarrow=same([7],[CInt8(3)]);
    let boxed=Box(value:"hello");
    let some=Option.Some(value:boxed);
    let none:Option<String> = Option.None();
    let result=transform(5,fn(value:Int)->String{{return f"{{value}}";}});
    let explicit=identity<Int>(42);
    take(Option.None());
    let options:List<Option<Int>> = [Option.Some(value:1)];
    let appended=listAppend(options,Option.None());
    let replaced=listSet(appended,0,Option.None());
    let mapped:Map<String,Option<Int>> = {{"first":Option.Some(value:1)}};
    let inserted=mapInsert(mapped,"second",Option.None());
    let accept:fn(Option<Int>)->Int=fn(value:Option<Int>)->Int{{return 1;}};
    let accepted=accept(Option.None());
    let a=identity({{x:Int=1,y="yes"}});
    let b=identity({{y="yes",x:Int=2}});
    let equal=a==b;
}}
"#
    );
    compile(&text).unwrap();
}

#[test]
fn no_backwards_inference_or_implicit_conversion_resolves_ambiguity() {
    for body in [
        "let none=Option.None();",
        "let value=same(identity<Int>(1),CInt8(2));",
        "let value=same(1,\"bad\");",
        "let none=Option.None();take(none);",
    ] {
        let text = format!("{BASE} fn take(value:Option<Int>){{}} fn main(){{{body}}}");
        assert!(compile(&text).is_err(), "{text}");
    }
}

#[test]
fn local_callables_can_shadow_core_operations_without_changing_inference() {
    compile("fn main(){let listAppend:fn(Option<Int>,Int)->Int=fn(value:Option<Int>,other:Int)->Int{return other;};let result=listAppend(Option.None(),7);}").unwrap();
}

#[test]
fn explicit_and_inferred_arguments_reuse_the_same_specialization() {
    let artifact = compile(&format!(
        "{BASE} fn main(){{let a=identity(1);let b=identity<Int>(2);}}"
    ))
    .unwrap();
    assert_eq!(
        artifact
            .ir
            .functions
            .iter()
            .filter(|f| f.name == "identity<Int>")
            .count(),
        1
    );
}
