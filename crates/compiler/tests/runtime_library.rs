use rinzu_compiler::{
    FileId, OperationKind, RuntimeFunction, SourceAbiType, SourceFile, Type, compile_source,
    format_source, parse_source,
};

fn compile(text: &str) -> rinzu_compiler::FrontendArtifact {
    compile_source(&SourceFile::new(FileId(0), "runtime.rinzu", text)).unwrap()
}

#[test]
fn timer_is_an_imported_library_function_without_native_declarations() {
    let artifact = compile(
        "import rinzu.timer; async fn main() { try { await rinzu.timer.sleep(1); } catch errors { panic(\"timer failed\"); } }",
    );
    assert!(artifact.syntax.native_blocks.is_empty());
    assert!(artifact.ir.functions.iter().flat_map(|f| &f.blocks).flat_map(|b| &b.operations).any(|op| {
        matches!(&op.kind, OperationKind::SourceNativeCall { signature, .. }
            if signature.binding == rinzu_compiler::ast::NativeBinding::Runtime(RuntimeFunction::Sleep))
            && op.result.as_ref().is_some_and(|value| value.ty == Type::Future(Box::new(Type::Unit)))
    }));
    assert!(!artifact.checked.functions[0].facts.worker_safe);
}

#[test]
fn timer_requires_its_import_and_int_milliseconds() {
    for text in [
        "async fn main() { try { await rinzu.timer.sleep(1); } catch errors {} }",
        "import rinzu.timer; async fn main() { try { await rinzu.timer.sleep(true); } catch errors {} }",
        "import rinzu.timer; async fn main() { try { await rinzu.timer.sleep(); } catch errors {} }",
        "import rinzu.timer; async fn main() { try { await rinzu.timer.sleep(1, 2); } catch errors {} }",
        "import rinzu.timer; async fn main() { try { await rinzu.timer.sleep(milliseconds: 1); } catch errors {} }",
        "import rinzu.io; async fn main() { try { await rinzu.io.timer(1); } catch errors {} }",
    ] {
        assert!(
            compile_source(&SourceFile::new(FileId(0), "bad.rinzu", text)).is_err(),
            "accepted {text}"
        );
    }
}

#[test]
fn service_bindings_are_rejected_and_service_is_an_ordinary_identifier() {
    for text in [
        r#"native { async fn delay(ms: Int) -> Unit service "reactor.timer"; }"#,
        r#"native { fn delay(ms: Int) -> Unit service "reactor.timer"; }"#,
        r#"native { async fn wait(value: Int) service "reactor.native-completion"; }"#,
    ] {
        let file = SourceFile::new(FileId(0), "removed.rinzu", text);
        assert!(parse_source(&file).is_err(), "accepted {text}");
        assert!(format_source(&file).is_err());
    }
    compile("fn service(value: Int) -> Int { return value; } fn main() { let x = service(1); }");
}

#[test]
fn completion_library_functions_preserve_each_opaque_handle_type() {
    let artifact = compile(
        "native { opaque First; opaque Second; } async fn wait(first: First, second: Second) -> Int { await nativeCompleted(first); await nativeCompleted(second); return await nativeTransferred(second); } async fn main() {}",
    );
    let signatures = &artifact.checked.source_native_signatures;
    assert_eq!(signatures.len(), 3);
    assert_eq!(
        signatures
            .iter()
            .filter(|s| s.parameters == [SourceAbiType::Opaque("Second".into())])
            .count(),
        2
    );
    for parameter in ["Int", "String", "ByteArray", "Record"] {
        for function in ["nativeCompleted", "nativeTransferred"] {
            let text = format!(
                "class Record {{}} async fn wait(value: {parameter}) {{ let result = {function}(value); await result; }} async fn main() {{}}"
            );
            assert!(
                compile_source(&SourceFile::new(FileId(0), "bad.rinzu", &text)).is_err(),
                "accepted {text}"
            );
        }
    }
}
