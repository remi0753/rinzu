use rinzu_compiler::{DataOperation, FileId, OperationKind, SourceFile, compile_source};

fn file(text: &str) -> SourceFile {
    SourceFile::new(FileId(451), "collection-api.rinzu", text)
}

#[test]
fn collection_modules_require_imports_and_preserve_nominal_error_bounds() {
    for text in [
        "fn main() { let count = rinzu.list.length([1]); }",
        "fn main() { let count = rinzu.map.length({1: 2}); }",
        "import rinzu.List; fn main() { let count = rinzu.List.length([1]); }",
        "import rinzu.Map; fn main() { let count = rinzu.Map.length({1: 2}); }",
        "fn main() { let count = listLength([1]); }",
        "import rinzu.core; fn main() { let count = rinzu.core.mapLength({1: 2}); }",
    ] {
        assert!(compile_source(&file(text)).is_err(), "{text}");
    }
    let checked = compile_source(&file(
        r#"
        import rinzu.list;
        import rinzu.map;
        error Callback {}
        fn transform(value: Int) -> Int throws Callback { return value; }
        fn key(value: Int) -> String throws Callback { return f"{value}"; }
        fn converted(values: List<Int>) -> List<Int> {
            return rinzu.list.map(values, transform);
        }
        fn indexed(values: List<Int>) -> Map<String, Int> {
            return rinzu.map.fromList(values, key);
        }
        fn grow(values: List<Int>) -> List<Int> throws rinzu.list.LengthOverflow {
            return rinzu.list.append(values, 1);
        }
        fn insert(values: Map<Int,Int>) -> Map<Int,Int> throws rinzu.map.LengthOverflow {
            return rinzu.map.insert(values, 1, 2);
        }
        fn concat(parts: List<List<Int>>) -> List<Int> throws rinzu.list.LengthOverflow {
            return rinzu.list.concatAll(parts);
        }
        fn main() {}
    "#,
    ))
    .unwrap();
    let summary = |name| {
        checked
            .checked
            .functions
            .iter()
            .find(|function| function.declaration.name == name)
            .unwrap()
            .error_types
            .clone()
    };
    assert_eq!(summary("converted"), ["Callback"]);
    assert_eq!(
        summary("indexed"),
        ["Callback", "rinzu.map::DuplicateKey<String>"]
    );
    assert_eq!(summary("grow"), ["rinzu.list::LengthOverflow"]);
    assert_eq!(summary("insert"), ["rinzu.map::LengthOverflow"]);
    assert_eq!(summary("concat"), ["rinzu.list::LengthOverflow"]);
}

#[test]
fn loops_use_buffers_only_while_the_accumulator_is_private() {
    let checked = compile_source(&file(
        r#"
        import rinzu.list;
        import rinzu.map;
        fn list(values: List<Int>) -> List<Int> {
            var out: List<Int> = [];
            for value in values { out = rinzu.list.append(out, value); }
            return out;
        }
        fn map(count: Int) -> Map<Int, Int> {
            var out: Map<Int, Int> = {};
            var index = 0;
            while index < count {
                out = rinzu.map.insert(out, index, index * 2);
                index = index + 1;
            }
            return out;
        }
        fn alias(values: List<Int>) -> List<Int> {
            var out: List<Int> = [];
            for value in values {
                let before = out;
                out = rinzu.list.append(out, value);
                if rinzu.list.length(before) >= rinzu.list.length(out) { panic("changed alias"); }
            }
            return out;
        }
        fn handler(values: List<Int>) -> List<Int> {
            var out: List<Int> = [];
            for value in values {
                try {
                    out = rinzu.list.append(out, value);
                } catch [rinzu.list.LengthOverflow] { return out; }
            }
            return out;
        }
        async fn pause() {}
        async fn delayed(values: List<Int>) -> List<Int> {
            var out: List<Int> = [];
            for value in values {
                await pause();
                out = rinzu.list.append(out, value);
            }
            return out;
        }
        fn main() {}
    "#,
    ))
    .unwrap();
    let operations = |name| {
        checked
            .ir
            .functions
            .iter()
            .find(|function| function.name == name)
            .unwrap()
            .blocks
            .iter()
            .flat_map(|block| block.operations.iter())
            .filter_map(|operation| match operation.kind {
                OperationKind::DataOperation { kind, .. } => Some(kind),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert!(operations("list").contains(&DataOperation::ListBufferAppend));
    assert!(operations("map").contains(&DataOperation::MapBufferInsert));
    assert!(operations("list").contains(&DataOperation::CollectionFinish));
    for function in ["alias", "handler", "delayed"] {
        assert!(operations(function).contains(&DataOperation::ListAppend));
        assert!(!operations(function).contains(&DataOperation::CollectionFinish));
    }
}

#[test]
fn bulk_operations_keep_future_obligations_on_dropped_and_failing_paths() {
    let prelude = r#"
        import rinzu.list;
        import rinzu.map;
        error Failed {}
        error Stop {}
        async fn fail(value: Int) -> Int { throw Failed(message: f"{value}"); }
    "#;
    for main in [
        r#"
        fn main() {
            let pending = fail(1);
            let kept = rinzu.list.filter([pending], fn (item: Future<Int>) -> Bool { return false; });
        }
        "#,
        r#"
        fn main() {
            let pending = fail(1);
            let input: List<{key: Int, value: Future<Int>}> =
                [{key = 1, value = pending}, {key = 1, value = pending}];
            try { let table = rinzu.map.fromEntries(input); }
            catch [duplicate: rinzu.map.DuplicateKey<Int>] {}
        }
        "#,
        r#"
        async fn main() {
            try {
                let mapped = rinzu.list.map([1, 2], fn (value: Int) -> Future<Int> {
                    if value == 2 { throw Stop(message: "stop"); }
                    return fail(value);
                });
                for task in mapped { try { let value = await task; } catch errors {} }
            } catch [problem: Stop] {}
        }
        "#,
    ] {
        let errors = compile_source(&file(&format!("{prelude}{main}"))).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.code == rinzu_compiler::DiagnosticCode::FutureDiscarded),
            "{errors:#?}"
        );
    }
}
