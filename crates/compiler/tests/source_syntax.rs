use rinzu_compiler::ast::{Declaration, NativeMember, StatementKind, TypeRefKind};
use rinzu_compiler::{
    DiagnosticCode, FileId, SourceFile, compile_source, format_source, parse_source,
};

const SOURCE: &str = r#"// source package
let blockSize: CSize = 4;
native {
    opaque Handle;
    record Event { code: CInt, bytes: Bytes(blockSize) }
    fn poll(output: Event) -> CBool symbol "poll_event";
    supplant fn run(handler: fn(Handle, Event) -> Unit) -> CInt symbol "run_loop";
}
class State { const active: Bool }
export Handle;
export Event;
export run;
fn callback(event: Event) fallback () {
    try { poll(event); } catch error { return; } finally { return; }
}
export blockSize;
fn main() -> Int { return 0; }
"#;

#[test]
fn parses_native_members_shared_constants_function_types_and_error_surface() {
    let source = SourceFile::new(FileId(7), "sdk.rinzu", SOURCE);
    let syntax = parse_source(&source).unwrap();
    assert_eq!(syntax.exports.len(), 4);
    assert!(matches!(syntax.declarations[0], Declaration::Constant(_)));
    let native = &syntax.native_blocks[0];
    assert_eq!(native.members.len(), 4);
    assert!(matches!(native.members[0], NativeMember::Opaque { .. }));
    let NativeMember::Function(run) = &native.members[3] else {
        panic!()
    };
    assert!(run.is_supplant);
    assert!(matches!(
        run.parameters[0].ty.kind,
        TypeRefKind::Function {
            supplant: false,
            ..
        }
    ));
    let Declaration::Function(callback) = &syntax.declarations[2] else {
        panic!()
    };
    assert!(callback.fallback.is_some());
    assert!(matches!(
        callback.body.statements[0].kind,
        StatementKind::TryCatch {
            cleanup: Some(_),
            ..
        }
    ));
    // Syntax-only acceptance must not silently expose untyped native calls in IR.
    assert!(
        compile_source(&source)
            .unwrap_err()
            .iter()
            .any(|d| d.code == DiagnosticCode::UnsupportedSafeFfi)
    );
}

#[test]
fn preserves_original_byte_spans_with_bom_crlf_and_unicode() {
    let text =
        "native { fn call(text: String) symbol \"entry\"; }\r\nfn cb() fallback \"日本語\" {}";
    let mut bytes = vec![0xef, 0xbb, 0xbf];
    bytes.extend(text.as_bytes());
    let source = SourceFile::from_utf8(FileId(9), "unicode.rinzu", bytes).unwrap();
    let syntax = parse_source(&source).unwrap();
    assert_eq!(syntax.native_blocks[0].range.start_byte, 3);
    let NativeMember::Function(function) = &syntax.native_blocks[0].members[0] else {
        panic!()
    };
    let span = function.binding_range;
    assert_eq!(
        &text[(span.start_byte - 3) as usize..(span.end_byte - 3) as usize],
        "\"entry\""
    );
    let Declaration::Function(callback) = &syntax.declarations[0] else {
        panic!()
    };
    assert_eq!(callback.fallback.as_ref().unwrap().range.file_id, FileId(9));
}

#[test]
fn formatting_preserves_tokens_comments_literals_and_is_idempotent() {
    let source = SourceFile::new(FileId(1), "sdk.rinzu", SOURCE);
    let formatted = format_source(&source).unwrap();
    let output = SourceFile::new(FileId(1), "formatted.rinzu", formatted.clone());
    assert_eq!(format_source(&output).unwrap(), formatted);
    assert!(formatted.contains("// source package"));
    let syntax = parse_source(&output).unwrap();
    assert_eq!(syntax.native_blocks[0].members.len(), 4);
    let comments = SourceFile::new(
        FileId(1),
        "comments.rinzu",
        "fn main() { /* before */ let value = /* value */ 2; // trailing\n} // end\n",
    );
    let formatted = format_source(&comments).unwrap();
    assert!(formatted.contains("/* value */") && formatted.contains("// end"));
    compile_source(&SourceFile::new(FileId(1), "formatted.rinzu", &formatted)).unwrap();
    assert_eq!(
        format_source(&SourceFile::new(FileId(1), "formatted.rinzu", &formatted)).unwrap(),
        formatted
    );
}

#[test]
fn rejects_broken_or_unapproved_surface_at_syntax_boundary() {
    for text in [
        "supplant fn run() {}",
        "native { supplant async fn run() service \"x\"; }",
        "native { async fn call() symbol \"x\"; }",
        "native { fn call() service \"x\"; }",
        "native { let x = 1; }",
        "native { type X }",
        "fn cb() { try {} }",
        "fn cb() { try {} catch (e) {} }",
    ] {
        assert!(
            parse_source(&SourceFile::new(FileId(1), "invalid.rinzu", text)).is_err(),
            "{text}"
        );
    }
    parse_source(&SourceFile::new(
        FileId(1),
        "types.rinzu",
        "fn call(handler: supplant fn(Bytes(4)) -> CInt) {}",
    ))
    .unwrap();
}
