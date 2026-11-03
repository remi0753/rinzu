use rinzu_compiler::{
    FileId, SourceAbiType, SourceFile, Type, compile_source, parse_source,
    source_native_signatures, verify_module,
};

fn file(text: &str) -> SourceFile {
    SourceFile::new(FileId(0), "context.rinzu", text)
}

#[test]
fn structural_contexts_derive_the_same_abi_from_inline_fields_and_transparent_aliases() {
    let text = r#"
type Context={text:String,count:Int};
type Same={count:Int,text:String};
native {
    fn first(context:Context,callback:fn(Same)->Int)->Int symbol "first";
    fn second(context:{count:Int,text:String})->Int symbol "second";
}
fn value(context:Same)->Int fallback -1 {return context.count;}
fn main(){let result=first({text="hi",count=7},value);let other=second({text="ok",count=8});}
"#;
    let raw = source_native_signatures(&parse_source(&file(text)).unwrap()).unwrap();
    assert_eq!(raw[0].parameters[0], raw[1].parameters[0]);
    let SourceAbiType::UserData(identity) = &raw[0].parameters[0] else {
        panic!("expected context ABI");
    };
    assert!(identity.starts_with("$rinzu_record_"));
    let SourceAbiType::Callback { parameters, .. } = &raw[0].parameters[1] else {
        panic!("callback missing");
    };
    assert_eq!(parameters, &vec![raw[0].parameters[0].clone()]);
    assert!(!raw[0].parameters[1].foreign_callback_eligible());
    let artifact = compile_source(&file(text)).unwrap();
    verify_module(&artifact.ir).unwrap();
    assert_eq!(artifact.ir.source_types[identity].kind, "record");
    assert_eq!(
        artifact.ir.source_types[identity].fields,
        vec![("count".into(), Type::Int), ("text".into(), Type::String)]
    );
}

#[test]
fn context_records_can_contain_classes_enums_native_references_and_functions() {
    compile_source(&file(r#"
class Editor{value:Int}
enum Mode{Ready,Value(value:Int)}
native{opaque Handle;record Packet{value:Int}
    fn make()->Handle symbol "make";
    fn invoke(context:{editor:Editor,mode:Mode,handle:Handle,packet:Packet,apply:fn(Int)->Int})->Int symbol "invoke";
}

fn apply(value:Int)->Int{return value;}
fn main(){try{let result=invoke({editor=Editor(value:1),mode=Mode.Ready(),handle=make(),packet=Packet(value:2),apply=apply});}catch errors{}}
"#)).unwrap();
}

#[test]
fn generic_aliases_and_nested_nominal_values_preserve_checked_context_identity() {
    let text = r#"
class Box<T>{value:T}
type Context<T> = {box:Box<T>,flags:List<Bool>,data:Bytes(3)};
native{fn useContext(context:Context<Int>)->Int symbol "use_context";}
fn main(){let value=useContext({box=Box<Int>(value:1),flags=[true],data:Bytes(3)=b"abc"});}
"#;
    let signatures = source_native_signatures(&parse_source(&file(text)).unwrap()).unwrap();
    let artifact = compile_source(&file(text)).unwrap();
    assert_eq!(
        signatures[0].parameters,
        artifact.checked.source_native_signatures[0].parameters
    );
}

#[test]
fn nominal_classes_enums_and_unsupported_context_positions_require_explicit_wrappers() {
    for signature in [
        "fn f(value:Mutable)->Int",
        "fn f(value:ConstOnly)->Int",
        "fn f(value:Empty)->Int",
        "fn f(value:Mode)->Int",
        "fn f(value:Alias)->Int",
        "fn f(value:fn(ConstOnly)->Int)->Int",
        "fn f(value:fn()->{value:Int})->Int",
        "fn f(value:{x:Int},other:{x:Bool})->Unit",
    ] {
        let text = format!(
            "class Mutable{{value:Int}}class ConstOnly{{const value:Int}}class Empty{{}}enum Mode{{Ready}}type Alias=ConstOnly;native{{{signature} symbol \"f\";}}fn main(){{}}"
        );
        // Different structures are individually legal, unlike direct nominal
        // values. Their ABI identities must remain distinct.
        if signature.contains("other:") {
            let signatures =
                source_native_signatures(&parse_source(&file(&text)).unwrap()).unwrap();
            assert_ne!(signatures[0].parameters[0], signatures[0].parameters[1]);
        } else {
            assert!(
                compile_source(&file(&text)).is_err(),
                "accepted {signature}"
            );
        }
    }
    for record in [
        "{}",
        "{x:Int,x:Bool}",
        "{x:Unknown}",
        "{x:NativeStorage<Packet>}",
    ] {
        let text = format!(
            "native{{record Packet{{value:Int}}fn f(context:{record})->Int symbol \"f\";}}fn main(){{}}"
        );
        assert!(compile_source(&file(&text)).is_err(), "accepted {record}");
        if let Ok(syntax) = parse_source(&file(&text)) {
            assert!(
                source_native_signatures(&syntax).is_err(),
                "raw ABI accepted {record}"
            );
        }
    }
}

#[test]
fn native_context_results_use_the_input_record_type_and_require_error_handling() {
    let text = r#"
type Context={value:Int};
native{fn choose(first:Context,second:{value:Int})->Context symbol "choose";}
fn main(){try{let selected=choose({value=1},{value=2});let value=selected.value;}catch errors{}}
"#;
    let artifact = compile_source(&file(text)).unwrap();
    let signature = &artifact.checked.source_native_signatures[0];
    assert_eq!(signature.result, signature.parameters[0]);
    verify_module(&artifact.ir).unwrap();
    let invalid = text.replace(
        "try{let selected=choose({value=1},{value=2});let value=selected.value;}catch errors{}",
        "let selected=choose({value=1},{value=2});let value=selected.value;",
    );
    assert!(
        compile_source(&file(&invalid))
            .unwrap_err()
            .iter()
            .any(|error| error.code == rinzu_compiler::DiagnosticCode::ErrorEscapesBoundary)
    );
    for result in ["Mutable", "ConstOnly", "Mode"] {
        let text = format!(
            "class Mutable{{value:Int}}class ConstOnly{{const value:Int}}enum Mode{{Ready}}native{{fn f(value:{result})->{result} symbol \"f\";}}fn main(){{}}"
        );
        assert!(
            compile_source(&file(&text)).is_err(),
            "accepted direct result {result}"
        );
    }
}
