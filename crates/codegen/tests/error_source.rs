#[path = "../test-support/library.rs"]
mod test_library;
use rinzu_codegen::{SourceAsyncOptions, run_source_async_report};
use rinzu_compiler::{FileId, SourceFile};
use test_library::compile_library;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn invalid_source_ranges_are_rejected_before_source_constants_are_emitted() {
    let file = SourceFile::new(FileId(219), "invalid-range.rinzu", "fn main(){let n=1;}");
    let mut artifact = rinzu_compiler::compile_source(&file).unwrap();
    let operation = artifact.ir.functions[0]
        .blocks
        .iter_mut()
        .flat_map(|block| &mut block.operations)
        .next()
        .unwrap();
    operation.range.start_byte = u32::MAX;
    operation.range.end_byte = 0;
    let error = rinzu_codegen::emit_baseline_aot_object(&artifact.ir, "main").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("invalid generated source position")
    );
}

fn check(text: &str, expected: &[u8]) {
    let file = SourceFile::new(FileId(219), "absent/位置.rinzu", text);
    let artifact = compile_library(&file).unwrap();
    let asynchronous = (!artifact
        .ir
        .functions
        .iter()
        .any(|function| function.is_async))
    .then(|| {
        compile_library(&SourceFile::new(
            FileId(219),
            "absent/位置.rinzu",
            text.replace("fn main()", "async fn main()"),
        ))
        .unwrap()
    });
    let async_ir = asynchronous
        .as_ref()
        .map_or(&artifact.ir, |artifact| &artifact.ir);
    for stress in [false, true] {
        let run = run_source_async_report(
            async_ir,
            "main",
            SourceAsyncOptions {
                capture_stdout: true,
                gc_stress: stress,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(run.failure_details.is_none(), "{run:#?}");
        assert_eq!(run.stdout, expected);
        assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if !artifact
            .ir
            .functions
            .iter()
            .any(|function| function.is_async)
        {
            let jit = rinzu_codegen::run_baseline_jit_with_options(
                &artifact.ir,
                "main",
                rinzu_codegen::BaselineOptions {
                    capture_stdout: true,
                    collect_every_allocation: stress,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(jit.result, Ok(0), "{jit:#?}");
            assert_eq!(jit.stdout, expected);
            assert_eq!(jit.heap_objects, 0);
        }
    }
}

const INSPECT: &str = r#"
fn inspect(problem:Error){
    switch problem.source {
    case Option.None:{rinzu.stdio.writeLine("none");}
    case Option.Some(at):{
        rinzu.stdio.writeLine(at.file);
        rinzu.stdio.writeLine(at.line);
        rinzu.stdio.writeLine(at.column);
    }
    }
}
"#;

fn expected_at(text: &str, token: &str) -> String {
    let file = SourceFile::new(FileId(219), "absent/位置.rinzu", text);
    let (line, column) = file.line_column(text.find(token).unwrap());
    format!("absent/位置.rinzu\n{line}\n{column}\n")
}

#[test]
fn first_throw_preserves_source_through_rethrow_alias_cause_and_gc_without_source_files() {
    let text = format!(
        "import rinzu.stdio;\r\nerror A{{payload:Int}}\r\nerror Wrapped{{}}\r\n{INSPECT}\r\n{}",
        r#"
fn raise(problem:A){let label="界🙂";   throw problem;}
fn relay(problem:A){try{raise(problem);}catch[error:A]{inspect(error);throw error;}}
fn main(){
    let problem=A(message:"original",payload:37);
    inspect(problem);
    try{relay(problem);}catch errors{inspect(problem);}
    try{throw problem;}catch[error:A]{inspect(error);rinzu.stdio.writeLine(error.payload);}
    let wrapped=Wrapped(message:"outer",cause:problem);
    switch wrapped.cause {case Option.None:{}case Option.Some(original):{inspect(original);}}
    try{throw wrapped;}catch[error:Wrapped]{inspect(error);}
}
"#
    );
    let at = expected_at(&text, "throw problem");
    let outer = expected_at(&text, "throw wrapped");
    let expected = format!("none\n{at}{at}{at}37\n{at}{outer}");
    check(&text, expected.as_bytes());
}

#[test]
fn operation_and_legacy_errors_keep_their_first_position_when_thrown_again() {
    let text = format!(
        "import rinzu.stdio;\n{INSPECT}\n{}",
        r#"
fn operation()->CInt8{return CInt8(300);}
fn relay(){try{let value=operation();}catch[first:Error,...rest]{throw first;}}
fn main(){
    try{relay();}catch[first:Error,...rest]{inspect(first);}
    let problem=error(7,"legacy");
    inspect(problem);
    try{throw problem;}catch[first:Error,...rest]{inspect(first);}
}
"#
    );
    let expected = format!(
        "{}none\n{}",
        expected_at(&text, "CInt8(300)"),
        expected_at(&text, "throw problem")
    );
    check(&text, expected.as_bytes());
}

#[test]
fn failed_future_repeated_await_and_finally_keep_each_original_position() {
    let text = format!(
        "import rinzu.stdio;\nerror A{{}}error B{{}}\n{INSPECT}\n{}",
        r#"
async fn work()->Int{throw A(message:"future");}
async fn main(){
    let pending=work();
    try{let one=await pending;}catch[first:A]{inspect(first);}
    try{try{let two=await pending;}finally{throw B(message:"cleanup");}}
    catch[first:A,second:B]{inspect(first);inspect(second);}
}
"#
    );
    let at = expected_at(&text, "throw A");
    let expected = format!("{at}{at}{}", expected_at(&text, "throw B"));
    check(&text, expected.as_bytes());
}

#[test]
fn concrete_error_source_fields_use_the_same_lazy_location_as_common_error_fields() {
    let text = r#"
import rinzu.stdio;
error A{}
fn raise(problem:A){throw problem;}
fn main(){
    let original=A(message:"typed");
    let before=original.source;
    switch before {case Option.None:{rinzu.stdio.writeLine("none");}case Option.Some(at):{rinzu.stdio.writeLine("unexpected");}}
    try{raise(original);}
    catch[problem:A]{switch problem.source {
    case Option.None:{rinzu.stdio.writeLine("missing");}
    case Option.Some(at):{rinzu.stdio.writeLine(at.file);rinzu.stdio.writeLine(at.line);rinzu.stdio.writeLine(at.column);}
    }}
    switch before {case Option.None:{rinzu.stdio.writeLine("still none");}case Option.Some(at):{rinzu.stdio.writeLine("changed");}}
}
"#;
    check(
        text,
        format!("none\n{}still none\n", expected_at(text, "throw problem")).as_bytes(),
    );
}
