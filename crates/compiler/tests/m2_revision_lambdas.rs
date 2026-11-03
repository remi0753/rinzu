use rinzu_compiler::{FileId, SourceFile, compile_source, format_source};
fn file(text: &str) -> SourceFile {
    SourceFile::new(FileId(1), "lambdas.rinzu", text)
}

#[test]
fn lambdas_use_full_arguments_explicit_returns_and_lexical_value_captures() {
    let text = r#"
fn add(a:Int,b:Int)->Int{return a+b;}
fn make(base:Int)->fn(Int)->Int {return fn(n:Int)->Int{return add(base,n);};}
class Counter {value:Int}
let double=fn(n:Int)->Int{return n+n;};
fn main() {
    let doubled=double(21);
    var base:Int=40;
    let addBase=fn(n:Int)->Int{return add(base,n);};
    base=41;
    let result=addBase(2);
    let plain=fn(n:Int)->Int{return n;};
    let generated=make(40);
    let counter=Counter(value:1);
    let change=fn(n:Int){counter.value=counter.value+n;};
    let nested=fn(n:Int)->fn(Int)->Int {return fn(m:Int)->Int{return base+n+m;};};
    let inner=nested(1);
    let answer=inner(1);
    let holder={apply=addBase};
    let field=holder.apply(2);
    let otherHolder={apply=fn(n:Int)->String{return "other";}};
    let text=otherHolder.apply(2);
    let shadow=fn(base:Int)->Int{return base;};
    let scope=fn()->Int {let old=base;let base:Int=3;return old+base;};
    let updated=scope();
    let n:Int=3;
    let sized=fn()->Bytes(3) {let data:Bytes(n)=b"abc";return data;};
    let bytes=sized();
}
"#;
    let formatted = format_source(&file(text)).unwrap();
    assert_eq!(formatted, format_source(&file(&formatted)).unwrap());
    let artifact = compile_source(&file(&formatted)).unwrap();
    assert!(
        artifact
            .ir
            .functions
            .iter()
            .any(|f| f.name.contains("$rinzu_lambda_"))
    );
    assert!(
        artifact
            .ir
            .source_types
            .keys()
            .any(|n| n.starts_with("$rinzu_closure_"))
    );
}

#[test]
fn lambdas_capture_only_free_variables_and_preserve_existing_rejections() {
    for (text, message) in [
        (
            "fn main(){let f=fn(n:Int)->Int{let x=n;};}",
            "without returning",
        ),
        ("fn main(){var n:Int=1;let f=fn(){n=2;};}", "immutable"),
        (
            "fn main(){let f=fn(a:Int,b:Int)->Int{return a+b;};let n=f(1);}",
            "argument count",
        ),
        (
            "fn main(){let f=fn(a:Int,a:Int)->Int{return a;};}",
            "duplicate function parameter",
        ),
        (
            "fn use(f:fn()->Int){} async fn main(){let f=fn()->Int{return 1;};await spawn use(f);}",
            "Transferable",
        ),
        (
            "fn main(){let f=fn()->Int{return 1;};let same=f==f;}",
            "operator",
        ),
    ] {
        let errors = compile_source(&file(text)).unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains(message)),
            "{text}: {errors:?}"
        );
    }
    compile_source(&file("async fn child()->Int{return 1;} async fn main(){let task=child();let f=fn(n:Int)->Int{return n;};let n=f(2);let ready=await task;}")).unwrap();
    compile_source(&file("async fn child()->Int{return 1;} async fn main(){let task=child();let f=fn()->Future<Int>{return task;};let ready=await task;}")).unwrap();
}

#[test]
fn async_lambdas_preserve_eager_call_effects_and_native_capture_plans() {
    compile_source(&file(include_str!(
        "../../../tests/fixtures/programs/lambdas/src/main.rinzu"
    )))
    .unwrap();
    let artifact=compile_source(&file("native {fn invoke(handler:fn(Int)->Int)->Int symbol \"invoke\";} class Editor {value:Int} fn main(){let editor=Editor(value:39);let result=invoke(fn(n:Int)->Int fallback -7 {editor.value=editor.value+n;return editor.value;});}")).unwrap();
    assert!(artifact.ir.functions.iter().flat_map(|f|&f.blocks).flat_map(|b|&b.operations).any(|op| matches!(&op.kind,rinzu_compiler::OperationKind::PrepareNativeCall{callbacks,..} if callbacks.iter().any(|c|c.capture.is_some()))));
    for text in [
        "native {fn invoke(handler:fn(Int)->Int)->Int symbol \"invoke\";} fn work()->Int {let prefix:Int=39;return invoke(fn(n:Int)->Int{return prefix+n;});} async fn main(){let n=await spawn work();}",
        "async fn child()->Int{return 1;} fn use(f:fn()->Future<Int>){} async fn main(){let f=async fn()->Int{return await child();};await spawn use(f);}",
    ] {
        assert!(compile_source(&file(text)).is_err(), "{text}");
    }
}
