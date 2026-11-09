use super::*;
use crate::test_library::compile_library as compile_source;
use rinzu_compiler::{FileId, SourceFile};

#[test]
fn legacy_native_callback_keeps_its_callers_owner_for_list_catches() {
    let module = Arc::new(
        compile_source(&SourceFile::new(
            FileId(179),
            "callback-errors.rinzu",
            r#"import rinzu.list;

        error A{} error B{}
        fn handler()->Int {
            try {try {throw A(message:"a");} finally {throw B(message:"b");}}
            catch errors {return 40+rinzu.list.length(errors);}
            return 0;
        }
        async fn main(){}
    "#,
        ))
        .unwrap()
        .ir,
    );
    let owner = OwnerValues::new(&module).unwrap();
    let mut callback = SourceCallbackContext {
        scope: 0,
        module: Arc::clone(&module),
        shared: Arc::new(Mutex::new(SharedState::default())),
        owner: Rc::clone(&owner),
        source_native: Rc::new(NativeEngine::new(Arc::new(NativeBindings::default())).unwrap()),
    };
    let handler = function_index(&module, "handler").unwrap() as u64;
    // SAFETY: the synchronous invocation uses this live stack context and
    // cannot retain it after return; its Owner and module remain alive.
    let result =
        unsafe { invoke_source_handler(std::ptr::from_mut(&mut callback).cast(), handler) };
    assert_eq!(result, 42);
    let observed = &callback.shared.lock().unwrap().caught_errors;
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].cleanup.len(), 1);
    assert_eq!(owner.borrow_mut().finish().unwrap().objects, 0);
}

#[test]
fn typed_sync_failure_routes_match_jit_calls_catches_and_nested_cleanup() {
    for body in [
        "try { let bad = fail(); } catch [error: Error, ...errorRest] { if error.message != \"numeric conversion is out of range or loses required integer precision\" { panic(\"wrong_message\"); } }",
        "var order = 0; try { try { let bad = fail(); } finally { order = order * 10 + 1; } } catch error { order = order * 10 + 2; } finally { order = order * 10 + 3; } if order != 123 { panic(\"wrong_order\"); }",
        "try { let bad = CInt8(200); } finally { let other = CUInt8(-1); }",
        "try { try { let bad = CInt8(200); } finally { let trap = 1 / 0; } } catch error { let should_not_catch = CInt8(300); }",
        "try { let trap = 1 / 0; } finally { try { let bad = CInt8(200); } catch [error: Error, ...errorRest] { let observed = error.message; } }",
        "try { let bad = CInt8(200); } finally { if true { try { let other = CUInt8(-1); } catch [error: Error, ...errorRest] { let observed = error.message; } } }",
        "try { try { let bad = CInt8(200); } finally { try { let second = CInt8(300); } finally { let third = CInt8(400); } } } catch [error: Error, ...errorRest] { let observed = error.message; }",
    ] {
        let source = SourceFile::new(
            FileId(23),
            "sync-failure.rinzu",
            format!("fn fail() -> CInt8 {{ return CInt8(200); }} fn main() {{ {body} }}"),
        );
        let artifact = compile_source(&source).unwrap();
        let jit = crate::run_baseline_jit(&artifact.ir, "main").unwrap();
        let mut observed = Vec::new();
        let owner = OwnerValues::new(&artifact.ir).unwrap();
        let native = NativeEngine::new(Arc::new(NativeBindings::default())).unwrap();
        let result = evaluate_sync_observed(
            &artifact.ir,
            "main",
            Vec::new(),
            Some(OwnerEvaluation {
                domain: None,
                callback: false,
                owner: &owner,
                source_native: &native,
            }),
            &mut observed,
        );
        let failure = result.as_ref().err().map(|error| {
            error
                .downcast_ref::<ManagedFailure>()
                .unwrap_or_else(|| panic!("unexpected failure in {body}: {error:#}"))
                .0
                .clone()
        });
        assert_eq!(failure, jit.failure_details, "{body}");
        assert_eq!(observed, jit.caught_errors, "{body}");
        assert_eq!(result.is_ok(), jit.result.is_ok(), "{body}");
        assert_eq!(owner.borrow_mut().finish().unwrap().objects, 0);
    }
}
