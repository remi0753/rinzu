use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source};
const BASE: &str = r#"
error Failed{payload:Int}
error Cleanup{}
native {fn invoke(handler:fn(Int)->Int)->Int symbol "invoke";}
fn risky(value:Int)->Int{throw Failed(message:"failure",payload:value);}
"#;
fn check(program: &str, valid: bool) {
    let result = compile_source(&SourceFile::new(
        FileId(278),
        "native-errors.rinzu",
        format!("{BASE}{program}"),
    ));
    match result {
        Ok(_) => assert!(valid, "an unchecked native entry was accepted"),
        Err(errors) => {
            assert!(!valid, "{errors:#?}");
            assert!(
                errors
                    .iter()
                    .any(|error| error.code == DiagnosticCode::ErrorEscapesBoundary),
                "{errors:#?}"
            );
        }
    }
}
#[test]
fn caller_catch_does_not_replace_native_entry_error_handling() {
    check(
        "fn callback(value:Int)->Int{return risky(value);}fn main(){try{let result=invoke(callback);}catch errors{}}",
        false,
    );
    check(
        "fn callback(value:Int)->Int{try{return risky(value);}catch[error:Failed]{return error.payload;}}fn main(){let result=invoke(callback);}",
        true,
    );
}
#[test]
fn explicit_fallback_covers_error_propagation_and_cleanup() {
    check(
        "fn callback(value:Int)->Int fallback -1{return risky(value);}fn main(){let result=invoke(callback);}",
        true,
    );
    check(
        "fn callback(value:Int)->Int fallback -1{try{return risky(value);}finally{throw Cleanup(message:\"cleanup\");}}fn main(){let result=invoke(callback);}",
        true,
    );
    check(
        "fn callback(value:Int)->Int{try{return risky(value);}catch errors{throw Cleanup(message:\"handler\");}}fn main(){let result=invoke(callback);}",
        false,
    );
}
#[test]
fn input_conversion_needs_fallback_before_the_body_can_catch() {
    let text = "native{fn input(handler:fn(String)->Int)->Int symbol \"input\";}fn callback(text:String)->Int{try{return 1;}catch errors{return -1;}}fn main(){let result=input(callback);}";
    let errors =
        compile_source(&SourceFile::new(FileId(278), "native-input.rinzu", text)).unwrap_err();
    let diagnostic = errors
        .iter()
        .find(|d| d.code == DiagnosticCode::ErrorEscapesBoundary)
        .unwrap();
    assert!(
        diagnostic
            .message
            .contains("input conversion can fail before the handler runs")
    );
    assert!(
        diagnostic.suggestions[0]
            .message
            .contains("ABI-compatible callback fallback")
    );
    assert!(
        diagnostic
            .secondary
            .iter()
            .all(|(at, _)| *at != diagnostic.primary)
    );
    check(
        "native{fn input(handler:fn(String)->Int)->Int symbol \"input\";}fn callback(text:String)->Int fallback -1{return 1;}fn main(){let result=input(callback);}",
        true,
    );
    check(
        "native{fn input(handler:fn(Bool)->Int)->Int symbol \"input\";}fn callback(value:Bool)->Int{return 1;}fn main(){let result=input(callback);}",
        false,
    );
}
#[test]
fn private_error_capture_stays_managed_and_must_not_leak() {
    check(
        "fn main(){let error=Failed(message:\"capture\",payload:7);let result=invoke(fn(value:Int)->Int{return error.payload+value;});}",
        true,
    );
    check(
        "fn main(){let error=Failed(message:\"capture\",payload:7);try{let result=invoke(fn(value:Int)->Int{throw error;});}catch errors{}}",
        false,
    );
    check(
        "fn main(){let error=Failed(message:\"capture\",payload:7);let result=invoke(fn(value:Int)->Int fallback -1{throw error;});}",
        true,
    );
}
#[test]
fn unused_closure_variants_do_not_pollute_the_native_entry() {
    check(
        "fn main(){let unused=fn(value:Int)->Int{return risky(value);};let result=invoke(fn(value:Int)->Int{return value;});}",
        true,
    );
}
#[test]
fn callback_constraints_follow_factories_and_forwarding() {
    check(
        "fn install(handler:fn(Int)->Int)->Int{return invoke(handler);}fn main(){let result=install(fn(value:Int)->Int{return risky(value);});}",
        false,
    );
    check(
        "fn install(handler:fn(Int)->Int)->Int{return invoke(handler);}fn main(){let result=install(fn(value:Int)->Int{try{return risky(value);}catch errors{return 11;}});}",
        true,
    );
}
#[test]
fn generic_and_recursive_callback_callees_cannot_hide_errors() {
    check(
        "fn fail<T>(value:T)->T{throw Cleanup(message:\"generic\");}fn callback(value:Int)->Int{return fail<Int>(value);}fn main(){let result=invoke(callback);}",
        false,
    );
    check(
        "fn recur(value:Int)->Int{if value==0{throw Cleanup(message:\"recursive\");}return recur(value-1);}fn main(){let result=invoke(recur);}",
        false,
    );
}
#[test]
fn native_result_preparation_is_not_assumed_to_be_infallible() {
    check(
        "native{fn get()->String symbol \"get\";}fn callback(value:Int)->Int{let text=get();return value;}fn main(){let result=invoke(callback);}",
        false,
    );
    check(
        "native{fn get()->String symbol \"get\";}fn callback(value:Int)->Int{try{let text=get();return value;}catch errors{return 13;}}fn main(){let result=invoke(callback);}",
        true,
    );
}
#[test]
fn core_callbacks_use_the_same_input_conversion_rule() {
    check(
        "native{fn input(handler:fn(String)->CSize)->CSize symbol \"input\";}fn main(){let result=input(utf8ByteLength);}",
        false,
    );
    check(
        "native{fn input(handler:fn(String)->CSize)->CSize symbol \"input\";}fn callback(text:String)->CSize fallback 0{return utf8ByteLength(text);}fn main(){let result=input(callback);}",
        true,
    );
    check(
        "native{fn input(handler:fn(Bytes(4))->CSize)->CSize symbol \"input\";}fn main(){let result=input(byteLength);}",
        true,
    );
}
#[test]
fn capture_free_forwarding_keeps_target_conditions() {
    check(
        "fn good(value:Int)->Int{return value;}fn install(handler:fn(Int)->Int)->Int{return invoke(handler);}fn main(){let result=install(good);}",
        true,
    );
    check(
        "fn install(handler:fn(Int)->Int)->Int{return invoke(handler);}fn main(){let result=install(risky);}",
        false,
    );
}
#[test]
fn generated_static_handlers_have_the_same_body_boundary() {
    use rinzu_compiler::{
        CompilerOptions, NativeCallClass, NativeFunction, StaticHandlerType, Type,
        compile_source_with_options,
    };
    let mut options = CompilerOptions::default();
    options.native_functions.insert(
        "generated".into(),
        NativeFunction {
            parameters: vec![Type::StaticHandler(Box::new(StaticHandlerType {
                parameters: vec![Type::Int],
                result: Box::new(Type::Int),
                is_async: false,
                owner_sync: false,
            }))],
            result: Type::Int,
            call_class: NativeCallClass::NativeSafe,
            metadata_complete: true,
            cleanup_safe: true,
        },
    );
    for (body, valid) in [
        ("return risky(value);", false),
        ("try{return risky(value);}catch errors{return 17;}", true),
    ] {
        let text = format!(
            "{BASE}fn callback(value:Int)->Int{{{body}}}fn main(){{try{{let result=generated(callback);}}catch errors{{}}}}"
        );
        let result = compile_source_with_options(
            &SourceFile::new(FileId(278), "generated-errors.rinzu", text),
            &options,
        );
        if valid {
            result.unwrap();
        } else {
            let errors = result.unwrap_err();
            assert!(
                errors
                    .iter()
                    .any(|error| error.code == DiagnosticCode::ErrorEscapesBoundary
                        && error.message.contains("native callback")),
                "{errors:#?}"
            );
        }
    }
}
#[test]
fn backend_fixture_has_checked_error_entries_and_private_captures() {
    let api = include_str!("../../driver/tests/fixtures/error_native_sdk/api.rinzu")
        .replace("export apply;", "")
        .replace("export input;", "");
    let (imports, app) = include_str!("../../driver/tests/fixtures/error_native_sdk/main.rinzu")
        .split_once("\n\n")
        .unwrap();
    let imports = imports.strip_prefix("import handlers;\n").unwrap();
    let app = app.replace("handlers.", "");
    compile_source(&SourceFile::new(
        FileId(278),
        "native-error-fixture.rinzu",
        format!("{imports}\n{api}\n{app}"),
    ))
    .unwrap();
}
#[test]
fn physical_native_function_pointers_do_not_add_a_managed_adapter() {
    check(
        "native{fn raw(value:String)->Int symbol \"raw\";fn input(handler:fn(String)->Int)->Int symbol \"input\";}fn main(){let result=input(raw);}",
        true,
    );
}
