use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source};

const BASE: &str = "error Failed{}error Cleanup{}fn risky(flag:Bool)->Int{if flag{throw Failed(message:\"failure\");}return 7;}";
fn check(program: &str, valid: bool) {
    let result = compile_source(&SourceFile::new(
        FileId(277),
        "worker-errors.rinzu",
        format!("{BASE}{program}"),
    ));
    match result {
        Ok(_) => assert!(valid, "an Error escaped a Worker root"),
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
fn a_worker_must_catch_its_own_errors_even_if_the_caller_catches() {
    check(
        "fn work()->Int{return risky(true);}async fn main(){try{let value=await spawn work();}catch errors{}}",
        false,
    );
    check(
        "fn work()->Int{try{return risky(true);}catch[Failed]{return 11;}}async fn main(){let value=await spawn work();}",
        true,
    );
}
#[test]
fn unrelated_throwing_functions_do_not_disable_safe_workers() {
    check(
        "fn work()->Int{return 13;}async fn main(){let value=await spawn work();}",
        true,
    );
    check(
        "fn work()->Int{return 1/0;}async fn main(){let value=await spawn work();}",
        true,
    );
}
#[test]
fn catch_and_finally_bodies_are_part_of_the_worker_boundary() {
    check(
        "fn work()->Int{try{return risky(true);}catch[Failed]{throw Cleanup(message:\"handler\");}}async fn main(){try{let value=await spawn work();}catch errors{}}",
        false,
    );
    check(
        "fn work()->Int{try{return 17;}finally{throw Cleanup(message:\"cleanup\");}}async fn main(){try{let value=await spawn work();}catch errors{}}",
        false,
    );
    check(
        "fn work()->Int{try{try{return risky(true);}finally{throw Cleanup(message:\"cleanup\");}}catch errors{return 19;}}async fn main(){let value=await spawn work();}",
        true,
    );
}
#[test]
fn forwarded_spawn_arguments_preserve_the_error_conditions() {
    check(
        "fn work(flag:Bool)->Int{return risky(flag);}async fn start(flag:Bool){let value=await spawn work(flag);}async fn main(){await start(false);}",
        true,
    );
    check(
        "fn work(flag:Bool)->Int{return risky(flag);}async fn start(flag:Bool){try{let value=await spawn work(flag);}catch errors{}}async fn main(){await start(true);}",
        false,
    );
}
#[test]
fn generic_and_indirect_callees_are_checked_with_their_actual_values() {
    check(
        "fn relay<T>(value:T)->T{throw Failed(message:\"generic\");}fn work()->Int{return relay<Int>(23);}async fn main(){try{let value=await spawn work();}catch errors{}}",
        false,
    );
    check(
        "fn invoke(action:fn(Bool)->Int)->Int{return action(true);}fn work()->Int{return invoke(risky);}async fn main(){try{let value=await spawn work();}catch errors{}}",
        false,
    );
    check(
        "fn work()->Int{let action=risky;try{return action(true);}catch errors{return 29;}}async fn main(){let value=await spawn work();}",
        true,
    );
}
#[test]
fn argument_evaluation_errors_belong_to_the_caller() {
    check(
        "fn work(value:CInt8)->Int{return 31;}async fn main(){try{let value=await spawn work(CInt8(300));}catch errors{}}",
        true,
    );
}
#[test]
fn recursive_and_aggregate_failures_cannot_escape_the_worker() {
    check(
        "fn recurse(depth:Int)->Int{if depth==0{throw Failed(message:\"recursive\");}return recurse(depth-1);}fn work()->Int{return recurse(2);}async fn main(){try{let value=await spawn work();}catch errors{}}",
        false,
    );
    check(
        "fn work(){try all{throw Failed(message:\"first\");throw Cleanup(message:\"second\");}catch[Failed,Cleanup]{}}async fn main(){await spawn work();}",
        true,
    );
}
