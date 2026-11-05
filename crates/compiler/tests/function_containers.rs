use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source};
fn compile(
    source: &str,
) -> Result<rinzu_compiler::FrontendArtifact, Vec<rinzu_compiler::Diagnostic>> {
    compile_source(&SourceFile::new(
        FileId(310),
        "function-containers.rinzu",
        source,
    ))
}
#[test]
fn list_map_values_future_and_nested_function_types_are_supported() {
    compile(
        r#"import rinzu.Map;

fn identity<T>(value:T)->T{return value;}
async fn make()->fn(Int)->Int{return fn(value:Int)->Int{return value+2;};}
async fn main(){
    let handlers:List<fn(Int)->Int> = [identity,fn(value:Int)->Int{return value+1;}];
    for handle in handlers {let value=handle(1);}
    let names:Map<String,fn(Int)->Int> = {"next":identity<Int>};
    switch rinzu.Map.get(names,"next") {case Option.Some(handle):{let value=handle(2);}case Option.None:{}}
    let handle=await make();let value=handle(3);
}
"#,
    )
    .unwrap();
}
const TASKS: &str = "error Failed{}async fn fail()->Int{throw Failed(message:\"failed\");}async fn safe()->Int{return 7;}";
#[test]
fn captured_future_obligations_survive_container_and_outer_future() {
    compile(&format!("{TASKS} async fn main(){{let pending=fail();let handlers:List<fn()->Future<Int>> = [fn()->Future<Int>{{return pending;}}];for get in handlers{{let receiver=async fn(){{try{{let result=await get();}}catch errors{{}}}};await receiver();}}}}")).unwrap();
    compile(&format!("{TASKS} async fn observe(handlers:List<fn()->Future<Int>>){{for get in handlers{{try{{let result=await get();}}catch errors{{}}}}}}async fn main(){{let pending=fail();let handlers:List<fn()->Future<Int>> = [safe,fn()->Future<Int>{{return pending;}}];await observe(handlers);}}")).unwrap();
    for body in [
        "let pending=fail();let handlers:List<fn()->Future<Int>> = [fn()->Future<Int>{return pending;}];",
        "let handlers=await make();",
    ] {
        let source = format!(
            "{TASKS} async fn make()->List<fn()->Future<Int>>{{let pending=fail();return [fn()->Future<Int>{{return pending;}}];}}async fn main(){{{body}}}"
        );
        let errors = compile(&source).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.code == DiagnosticCode::FutureDiscarded),
            "{errors:?}"
        );
    }
    for skip in [
        "break;",
        "let pending=get();if futureReady(pending){continue;}try{let result=await pending;}catch errors{}",
    ] {
        let source = format!(
            "{TASKS} async fn observe(handlers:List<fn()->Future<Int>>){{for get in handlers{{{skip}}}}}async fn main(){{let pending=fail();let handlers:List<fn()->Future<Int>> = [safe,fn()->Future<Int>{{return pending;}}];await observe(handlers);}}"
        );
        let errors = compile(&source).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.code == DiagnosticCode::FutureDiscarded),
            "{errors:?}"
        );
    }
}
#[test]
fn function_keys_worker_transfer_and_future_capturing_error_payloads_stay_invalid() {
    for source in [
        "fn f()->Int{return 1;}fn main(){let invalid:Map<fn()->Int,Int> = {f:1};}",
        "fn f()->Int{return 1;}fn worker(values:List<fn()->Int>)->Int{return 1;}async fn main(){let values:List<fn()->Int> = [f];let n=await spawn worker(values);}",
        "native {fn invalid(values:List<fn()->Int>) symbol \"invalid\";}fn main(){}",
    ] {
        assert!(compile(source).is_err(), "{source}");
    }
    compile(&format!("{TASKS} error Stored{{payload:List<fn()->Future<Int>>}}fn main(){{let e=Stored(message:\"failure\",payload:[safe]);}}")).unwrap();
    let source = format!(
        "{TASKS} error Stored{{payload:List<fn()->Future<Int>>}}fn main(){{let pending=safe();let e=Stored(message:\"failure\",payload:[fn()->Future<Int>{{return pending;}}]);}}"
    );
    assert!(compile(&source).is_err());
}
