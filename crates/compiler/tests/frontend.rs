use std::fs;
use std::path::Path;

use rinzu_compiler::{
    CompilerOptions, ExternalType, FileId, NativeCallClass, NativeFunction, OperationKind,
    PollReason, SourceFile, StackRoot, StaticHandlerType, Type, check_package, compile_source,
    compile_source_with_options, verify_module,
};

#[test]
fn compiles_core_language_to_verified_ir() {
    let path = Path::new("tests/fixtures/valid/core.rinzu");
    let source = SourceFile::new(
        FileId(1),
        path,
        fs::read_to_string(path).expect("fixture must be readable"),
    );
    let artifact = compile_source(&source).expect("valid fixture must compile");
    assert_eq!(artifact.checked.functions.len(), 5);
    assert!(
        artifact
            .ir
            .functions
            .iter()
            .all(|function| !function.blocks.is_empty())
    );
}

#[test]
fn emits_typed_stack_maps_and_required_poll_sites() {
    let path = Path::new("tests/fixtures/valid/core.rinzu");
    let source = SourceFile::new(
        FileId(1),
        path,
        fs::read_to_string(path).expect("fixture must be readable"),
    );
    let artifact = compile_source(&source).expect("valid fixture must compile");
    let main = artifact
        .ir
        .functions
        .iter()
        .find(|function| function.name == "main")
        .unwrap();
    let safepoints = main
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .filter_map(|operation| match &operation.kind {
            OperationKind::GcSafepoint { reason, stack_map } => Some((*reason, stack_map)),
            _ => None,
        })
        .collect::<Vec<_>>();
    for required in [
        PollReason::FunctionEntry,
        PollReason::LoopBackedge,
        PollReason::ManagedCall,
        PollReason::Allocation,
        PollReason::Await,
        PollReason::Spawn,
    ] {
        assert!(
            safepoints.iter().any(|(reason, _)| *reason == required),
            "missing {required:?} poll"
        );
    }
    assert!(safepoints.iter().all(|(_, stack_map)| {
        stack_map.roots.iter().all(|root| match root {
            StackRoot::Value { ty, .. } | StackRoot::Local { ty, .. } => ty.is_managed_reference(),
        })
    }));
    assert!(safepoints.iter().any(|(reason, stack_map)| {
        *reason == PollReason::ManagedCall
            && stack_map
                .roots
                .iter()
                .any(|root| matches!(root, StackRoot::Local { .. }))
    }));
}

#[test]
fn verifier_rejects_an_incomplete_stack_map() {
    let path = Path::new("tests/fixtures/valid/core.rinzu");
    let source = SourceFile::new(
        FileId(1),
        path,
        fs::read_to_string(path).expect("fixture must be readable"),
    );
    let mut artifact = compile_source(&source).expect("valid fixture must compile");
    let stack_map = artifact
        .ir
        .functions
        .iter_mut()
        .flat_map(|function| &mut function.blocks)
        .flat_map(|block| &mut block.operations)
        .find_map(|operation| match &mut operation.kind {
            OperationKind::GcSafepoint { stack_map, .. }
                if stack_map
                    .roots
                    .iter()
                    .any(|root| matches!(root, StackRoot::Value { .. })) =>
            {
                Some(stack_map)
            }
            _ => None,
        })
        .expect("fixture must have a safepoint with a value root");
    stack_map.roots.clear();
    let diagnostics = verify_module(&artifact.ir).expect_err("incomplete map must be rejected");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("missing from the stack map"))
    );
}

#[test]
fn compile_fail_fixtures_match_golden_diagnostics() {
    let directory = Path::new("tests/fixtures/invalid");
    let mut sources: Vec<_> = fs::read_dir(directory)
        .expect("fixture directory must be readable")
        .map(|entry| entry.expect("fixture entry must be readable").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "rinzu")
        })
        .collect();
    sources.sort();
    assert!(!sources.is_empty());
    for (index, path) in sources.iter().enumerate() {
        let source = SourceFile::new(
            FileId(index as u32 + 1),
            path,
            fs::read_to_string(path).expect("fixture must be readable"),
        );
        let diagnostics = compile_source(&source).expect_err("invalid fixture must fail");
        let actual = diagnostics
            .iter()
            .map(|diagnostic| format!("{}|{}", diagnostic.code.as_str(), diagnostic.message))
            .collect::<Vec<_>>()
            .join("\n");
        let expected_path = path.with_extension("golden");
        let expected = fs::read_to_string(&expected_path)
            .expect("golden file must be readable")
            .trim()
            .to_owned();
        assert_eq!(actual, expected, "fixture {}", path.display());
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| !diagnostic.suggestions.is_empty())
        );
    }
}

#[test]
fn loads_and_links_path_derived_package_modules() {
    let (manifest, artifact) = check_package(Path::new("tests/fixtures/package"))
        .expect("multi-module package must check");
    assert_eq!(manifest.name, "package_fixture");
    assert_eq!(artifact.checked.functions.len(), 2);
    assert!(
        artifact
            .checked
            .functions
            .iter()
            .any(|function| function.declaration.name == "math::add")
    );
}

#[test]
fn package_check_rejects_an_undeclared_sdk_import() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "rinzu-undeclared-sdk-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("rinzu.toml"),
        "[package]\nname = \"undeclared_sdk\"\nlanguage = \"0.2\"\n",
    )
    .unwrap();
    fs::write(
        root.join("src/main.rinzu"),
        "import rinzu.macos;\n\nfn main() {\n}\n",
    )
    .unwrap();
    let diagnostics = check_package(&root).expect_err("undeclared SDK import must fail");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("not a declared dependency")),
        "{diagnostics:?}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn generated_binding_context_rejects_owner_sync_suspension_and_call_only_escape() {
    let source = SourceFile::new(
        FileId(1),
        "binding.rinzu",
        r#"
import rinzu.test;

async fn work() {
}

async fn handler(view: rinzu.test.View) -> rinzu.test.View {
    await work();
    return view;
}

fn main() {
}
"#,
    );
    let mut options = CompilerOptions::default();
    options.owner_sync_handlers.insert("handler".to_owned());
    options.external_types.insert(
        "rinzu.test.View".to_owned(),
        ExternalType { call_only: true },
    );
    let diagnostics =
        compile_source_with_options(&source, &options).expect_err("invalid handler must fail");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code.as_str() == "RZ0401")
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code.as_str() == "RZ0408")
    );
}

#[test]
fn complete_binding_metadata_lowers_to_verified_native_ir() {
    let source = SourceFile::new(
        FileId(1),
        "binding.rinzu",
        r#"
import rinzu.test;

fn handler(view: rinzu.test.View) {
    rinzu.test.consume(view);
}

fn main() {
}
"#,
    );
    let view_type = Type::Native {
        name: "rinzu.test.View".to_owned(),
        call_only: true,
    };
    let mut options = CompilerOptions::default();
    options.owner_sync_handlers.insert("handler".to_owned());
    options.external_types.insert(
        "rinzu.test.View".to_owned(),
        ExternalType { call_only: true },
    );
    options.native_functions.insert(
        "rinzu.test.consume".to_owned(),
        NativeFunction {
            parameters: vec![view_type],
            result: Type::Unit,
            call_class: NativeCallClass::OwnerThreadSync,
            metadata_complete: true,
            cleanup_safe: true,
        },
    );
    let artifact =
        compile_source_with_options(&source, &options).expect("complete binding must compile");
    assert_eq!(artifact.checked.native_calls.len(), 1);
}

#[test]
fn generated_callback_position_accepts_only_a_matching_top_level_handler() {
    let source = SourceFile::new(
        FileId(3),
        "static-handler.rinzu",
        r#"
import rinzu.test;

fn handler(view: rinzu.test.View) {
    try { rinzu.test.consume(view); }
    catch errors {}
}

fn main() {
    try { rinzu.test.register(handler); }
    catch errors {}
}
"#,
    );
    let view_type = Type::Native {
        name: "rinzu.test.View".to_owned(),
        call_only: true,
    };
    let handler_type = Type::StaticHandler(Box::new(StaticHandlerType {
        parameters: vec![view_type.clone()],
        result: Box::new(Type::Unit),
        is_async: false,
        owner_sync: true,
    }));
    let mut options = CompilerOptions::default();
    options.owner_sync_handlers.insert("handler".to_owned());
    options.external_types.insert(
        "rinzu.test.View".to_owned(),
        ExternalType { call_only: true },
    );
    options.native_functions.insert(
        "rinzu.test.consume".to_owned(),
        NativeFunction {
            parameters: vec![view_type],
            result: Type::Unit,
            call_class: NativeCallClass::OwnerThreadSync,
            metadata_complete: true,
            cleanup_safe: true,
        },
    );
    options.native_functions.insert(
        "rinzu.test.register".to_owned(),
        NativeFunction {
            parameters: vec![handler_type],
            result: Type::Unit,
            call_class: NativeCallClass::NativeSafe,
            metadata_complete: true,
            cleanup_safe: false,
        },
    );
    let artifact = compile_source_with_options(&source, &options).unwrap();
    assert!(
        artifact
            .ir
            .functions
            .iter()
            .flat_map(|function| &function.blocks)
            .flat_map(|block| &block.operations)
            .any(|operation| matches!(operation.kind, OperationKind::StaticHandlerReference { ref target } if target == "handler"))
    );

    let stored = SourceFile::new(
        FileId(4),
        "stored-handler.rinzu",
        "fn handler() {}\nfn main() { let stored = handler; }\n",
    );
    let artifact = compile_source(&stored).expect("v0.2 permits local function references");
    assert!(
        artifact
            .ir
            .functions
            .iter()
            .flat_map(|function| &function.blocks)
            .flat_map(|block| &block.operations)
            .any(|operation| matches!(operation.kind, OperationKind::FunctionReference { .. }))
    );
}
