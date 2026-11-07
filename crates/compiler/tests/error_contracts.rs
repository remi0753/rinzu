use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source, format_source};

fn file(source: &str) -> SourceFile {
    SourceFile::new(FileId(701), "error-contracts.rinzu", source)
}
fn ok(source: &str) {
    compile_source(&file(source)).unwrap_or_else(|e| panic!("{source}\n{e:#?}"));
    let formatted = format_source(&file(source)).unwrap();
    assert_eq!(formatted, format_source(&file(&formatted)).unwrap());
    compile_source(&file(&formatted)).unwrap();
}
fn rejects(source: &str, code: DiagnosticCode) {
    let errors = compile_source(&file(source)).expect_err(source);
    assert!(
        errors.iter().any(|e| e.code == code),
        "{source}\n{errors:#?}"
    );
}

#[test]
fn declared_upper_bound_does_not_require_failure_but_callers_must_handle_it() {
    ok(
        "error E{} fn read() throws(E)->String{return \"ok\";} fn forward() throws(E)->String{return read();} fn main(){try{let s=forward();}catch problems{}}",
    );
    rejects(
        "error E{} fn read() throws(E){} fn main(){read();}",
        DiagnosticCode::ErrorEscapesBoundary,
    );
    rejects(
        "error E{} fn read() throws(E){} fn invalid() nothrow{read();}fn main(){}",
        DiagnosticCode::InvalidErrorContract,
    );
    ok(
        "error E{} fn read() throws(E){} fn recover() nothrow{try{read();}catch problems{}}fn main(){recover();}",
    );
}

#[test]
fn body_catch_and_cleanup_are_checked_against_the_upper_bound() {
    rejects(
        "error E{} fn invalid() nothrow{throw E(message:\"bad\");}fn main(){}",
        DiagnosticCode::InvalidErrorContract,
    );
    rejects(
        "error E{} error F{} fn invalid() throws(E){try{}finally{throw F(message:\"bad\");}}fn main(){}",
        DiagnosticCode::InvalidErrorContract,
    );
    ok(
        "error E{} fn recovered() nothrow{try{throw E(message:\"bad\");}catch problems{}}fn main(){recovered();}",
    );
}

#[test]
fn finite_contracts_preserve_error_types_but_do_not_promise_single_failure() {
    ok(
        "error A{}error B{}error C{}fn f()throws(A,B,C){} fn main(){try{f();}catch [a:A,...rest]{}catch [b:B,...rest]{}catch [c:C,...rest]{}}",
    );
    rejects(
        "error A{}fn f()throws(A){} fn main(){try{f();}catch [a:A]{}}",
        DiagnosticCode::NonExhaustiveCatch,
    );
    rejects(
        "error A{}error B{}fn f()throws(A){throw B(message:\"bad\");}fn main(){}",
        DiagnosticCode::InvalidErrorContract,
    );
}

#[test]
fn invalid_redundant_and_non_error_contracts_are_rejected() {
    for source in [
        "fn f()throws(Int){}fn main(){}",
        "error A{}fn f()throws(A,A){}fn main(){}",
        "error A{}fn f()throws(Error,A){}fn main(){}",
        "fn f<T>(value:T)throws(T){}fn main(){}",
        "fn f<T>(value:T){let bad=fn()throws(T){};}fn main(){}",
        "fn f<T>(value:T)throws(Missing){}fn main(){}",
    ] {
        rejects(source, DiagnosticCode::InvalidErrorContract);
    }
    for source in [
        "fn f()throws(){}fn main(){}",
        "fn f()throws({})fn main(){}",
        "fn f()nothrow throws Error{}fn main(){}",
    ] {
        assert!(compile_source(&file(source)).is_err());
    }
}

#[test]
fn async_contracts_remain_latent_and_preserve_future_observation() {
    rejects(
        "error E{}async fn later() throws(E)->Int{return 1;}async fn main(){let pending=later();}",
        DiagnosticCode::FutureDiscarded,
    );
    ok(
        "error E{}async fn later() throws(E)->Int{return 1;}fn begin() nothrow->Future<Int>{return later();}async fn main(){try{let n=await begin();}catch problems{}}",
    );
    rejects(
        "error E{}async fn later() throws(E)->Int{return 1;}async fn invalid() nothrow->Int{return await later();}fn main(){}",
        DiagnosticCode::InvalidErrorContract,
    );
}

#[test]
fn generic_error_types_and_contextual_keywords_round_trip() {
    ok(
        "error Failure<T>{payload:T}fn f<T>(value:T)throws(Failure<T>){throw Failure<T>(message:\"bad\",payload:value);} fn nothrow() nothrow->Int{return 1;} fn throws() nothrow{} fn main(){try{f(1);}catch problems{}let n=nothrow();throws();}",
    );
}

#[test]
fn declared_bounds_follow_function_values_and_argument_evaluation() {
    rejects(
        "error E{}fn apply(action:fn()->Int)->Int{return action();}fn main(){let action=fn() throws(E)->Int{return 42;};let value=apply(action);}",
        DiagnosticCode::ErrorEscapesBoundary,
    );
    ok(
        "error E{}fn apply(action:fn()->Int)->Int{return action();}fn main(){let action=fn() throws(E)->Int{return 42;};try{let value=apply(action);}catch [error:E,...rest]{}}",
    );
    rejects(
        "error E{}fn input() throws(E)->Int{return 42;}async fn ready(value:Int) nothrow->Int{return value;}fn begin() nothrow->Future<Int>{return ready(input());}fn main(){}",
        DiagnosticCode::InvalidErrorContract,
    );
    ok(
        "error A{}error B{}fn input()throws(A,B){}fn recoverB()throws(A){try{input();}catch [a:A,...rest]{throw a;}catch [b:B,...rest]{}}fn main(){try{recoverB();}catch [a:A,...rest]{}}",
    );
}

#[test]
fn contracts_keep_returned_future_responsibility_on_successful_paths() {
    rejects(
        "error E{}async fn fail() throws(E)->Int{throw E(message:\"failed\");}fn begin() throws(E)->Future<Int>{return fail();}fn main(){try{let pending=begin();}catch errors{}}",
        DiagnosticCode::FutureDiscarded,
    );
    ok(
        "error E{}async fn fail() throws(E)->Int{throw E(message:\"failed\");}fn begin() throws(E)->Future<Int>{return fail();}async fn main(){try{let pending=begin();let value=await pending;}catch errors{}}",
    );
}

#[test]
fn upper_bounds_check_every_error_beyond_the_catch_prefix() {
    rejects(
        "error A{}error B{}fn fail()throws(A){try{throw A(message:\"primary\");}finally{throw B(message:\"cleanup\");}}fn main(){}",
        DiagnosticCode::InvalidErrorContract,
    );
    rejects(
        "error A{}error B{}fn fail()throws(A){try all{throw A(message:\"first\");throw A(message:\"second\");throw B(message:\"last\");}finally{}}fn main(){}",
        DiagnosticCode::InvalidErrorContract,
    );
    ok(
        "error A{}fn fail()throws(A){try{throw A(message:\"primary\");}finally{throw A(message:\"cleanup\");}}fn main(){try{fail();}catch errors{}}",
    );
    rejects(
        "error A{}error B{}async fn fail(){try{throw A(message:\"primary\");}finally{throw B(message:\"cleanup\");}}async fn invalid()throws(A){let pending=fail();let value=await pending;}fn main(){}",
        DiagnosticCode::InvalidErrorContract,
    );
    rejects(
        "error A{}error B{}async fn fail(){try{throw A(message:\"primary\");}finally{throw B(message:\"cleanup\");}}async fn forward(value:Future<Unit>){await value;}async fn invalid()throws(A){await forward(fail());}fn main(){}",
        DiagnosticCode::InvalidErrorContract,
    );
    ok(
        "error A{}async fn fail()throws(A){try{throw A(message:\"primary\");}finally{throw A(message:\"cleanup\");}}async fn forward(value:Future<Unit>){await value;}async fn recover()throws(A){await forward(fail());}async fn main(){try{await recover();}catch errors{}}",
    );
}

#[test]
fn many_optional_error_types_keep_finite_summaries_and_complete_api_bounds() {
    let mut source = String::new();
    for i in 0..16 {
        source.push_str(&format!("error E{i}{{}}fn f{i}()throws(E{i}){{}}"));
    }
    source.push_str("fn collect(){try all{");
    for i in 0..16 {
        source.push_str(&format!("f{i}();"));
    }
    source.push_str("}finally{}}fn main(){try{collect();}catch errors{}}");
    let artifact = compile_source(&file(&source)).unwrap();
    let collected = artifact
        .checked
        .functions
        .iter()
        .find(|f| f.declaration.name == "collect")
        .unwrap();
    assert_eq!(collected.error_types.len(), 16);
    assert!(collected.error_types.iter().any(|t| t == "E15"));
}

#[test]
fn callback_contracts_bound_calls_and_widen_only_individual_values() {
    ok(r#"
        error InvalidPort {}
        error ReadFailed {}
        fn pure(value: Int) -> Int { return value; }
        fn validate(value: Int) throws(InvalidPort) -> Int { return value; }
        fn apply(op: fn (Int) throws(InvalidPort) -> Int, value: Int) throws(InvalidPort) -> Int {
            return op(value);
        }
        fn applyPure(op: fn (Int) nothrow -> Int, value: Int) nothrow -> Int { return op(value); }
        fn main() {
            let n = applyPure(pure, 42);
            let callbacks: List<fn (Int) throws(ReadFailed, InvalidPort) -> Int> = [pure, validate];
            let reordered: List<fn (Int) throws(InvalidPort, ReadFailed) -> Int> = callbacks;
            try { let checked = apply(validate, n); }
            catch [invalid: InvalidPort, ...rest] {}
        }
        "#);
    for source in [
        "error E{} fn broad()throws(Error)->Int{return 1;}fn main(){let op:fn()throws(E)->Int=broad;}",
        "error E{} fn allowed(op:fn()throws(E)->Int)nothrow->Int{return op();}fn main(){}",
        "error E{} fn narrow(op:fn()->Int){let typed:fn()throws(E)->Int=op;}fn main(){}",
        "error E{} fn narrow(op:fn()->Int){let typed:fn()nothrow->Int=op;}fn main(){}",
        "error E{} fn failing()->Int{throw E(message:\"bad\");}fn main(){let op:fn()nothrow->Int=failing;}",
        "error E{}fn recursive(n:Int)->Int{let again:fn(Int)nothrow->Int=recursive;if n<0{throw E(message:\"bad\");}return again(n);}fn main(){}",
    ] {
        rejects(source, DiagnosticCode::InvalidErrorContract);
    }
    ok("fn accept(op:fn()->Int){let typed:fn()throws(Error)->Int=op;}fn main(){}");
    rejects(
        "error E{}fn pure()nothrow->Int{return 1;}fn main(){let op:fn()throws(E)->Int=pure;let value=op();}",
        DiagnosticCode::ErrorEscapesBoundary,
    );
    rejects(
        "error E{}fn pure()nothrow->Int{return 1;}fn main(){let xs:List<fn()nothrow->Int> = [pure];let ys:List<fn()throws(E)->Int> = xs;}",
        DiagnosticCode::TypeMismatch,
    );
}

#[test]
fn nested_contracts_survive_returned_closures_fields_and_generic_aliases() {
    ok(r#"
        error Failure<T> { payload: T }
        type Decoder<T> = fn (T) throws(Failure<T>) -> T;
        type Factory = fn (Int) throws(Failure<Int>) -> fn () nothrow -> Int;
        class Holder { const decode: Decoder<Int> }
        fn build(value: Int) throws(Failure<Int>) -> fn () nothrow -> Int {
            return fn () nothrow -> Int { return value; };
        }
        fn apply<T>(operation: Decoder<T>, value: T) throws(Failure<T>) -> T {
            return operation(value);
        }
        fn decode(value: Int) throws(Failure<Int>) -> Int { return value; }
        fn main() {
            let factory: Factory = build;
            let holder = Holder(decode: decode);
            try {
                let read = factory(42);
                let v = holder.decode(read());
                let n = apply(decode, v);
            }
            catch [failed: Failure<Int>, ...rest] {}
        }
        "#);
    rejects(
        "error E{}type Alias=E;type Bad=fn()throws(E,Alias)->Unit;fn main(){}",
        DiagnosticCode::InvalidErrorContract,
    );
    rejects(
        "type Bad<T> = fn()throws(T)->Unit;fn main(){}",
        DiagnosticCode::InvalidErrorContract,
    );
}

#[test]
fn nested_alias_instantiations_have_independent_runtime_signatures() {
    ok(r#"
        type Factory<T> = fn () -> fn (T) nothrow -> T;
        fn ints() -> fn (Int) nothrow -> Int {
            return fn (value: Int) nothrow -> Int { return value; };
        }
        fn strings() -> fn (String) nothrow -> String {
            return fn (value: String) nothrow -> String { return value; };
        }
        fn main() {
            let makeInt: Factory<Int> = ints;
            let makeString: Factory<String> = strings;
            let readInt = makeInt();
            let readString = makeString();
            if readInt(42) != 42 || readString("ok") != "ok" { panic("callback"); }
        }
    "#);
}

#[test]
fn bounded_callback_invocation_does_not_discharge_future_observation() {
    let declarations = r#"
        error E {}
        async fn fail() throws(E) -> Int { throw E(message: "observed"); }
        fn launch(op: fn () nothrow -> Future<Int>) nothrow -> Future<Int> { return op(); }
    "#;
    ok(&format!(
        "{declarations}async fn main(){{try{{let n=await launch(fail);}}catch [e:E,...rest]{{}}}}"
    ));
    rejects(
        &format!("{declarations}fn main(){{let pending=launch(fail);}}"),
        DiagnosticCode::FutureDiscarded,
    );
    rejects(
        &format!(
            "{declarations}async fn invalid()nothrow->Int{{return await launch(fail);}}fn main(){{}}"
        ),
        DiagnosticCode::InvalidErrorContract,
    );
}

#[test]
fn old_contract_positions_and_missing_delimiters_are_syntax_errors() {
    for source in [
        "fn main() -> Unit nothrow {}",
        "fn main() throws Error {}",
        "fn main() -> Unit throws(Error) {}",
        "fn callback(value: CInt) -> CInt fallback 0 nothrow { return value; }",
    ] {
        rejects(source, DiagnosticCode::UnexpectedToken);
    }
    ok("fn callback(value:CInt)nothrow->CInt fallback 0{return value;}fn main(){}");
}
