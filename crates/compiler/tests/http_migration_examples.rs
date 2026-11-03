#[test]
fn http_regression_inputs_check_as_real_entries() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let filter = std::env::var("RINZU_MIGRATION_FILTER").unwrap_or_default();
    let mut failures = Vec::new();
    for name in [
        "sdk/socket/bench/client.rinzu",
        "sdk/socket/bench/server.rinzu",
        "tests/fixtures/http-wire/parser-tests.rinzu",
    ] {
        if !name.contains(&filter) {
            continue;
        }
        let started = std::time::Instant::now();
        eprintln!("checking regression input {name}");
        if let Err(errors) = rinzu_compiler::check_input(
            &root.join(name),
            &rinzu_compiler::CompilationOptions {
                language: None,
                target: rinzu_compiler::standard_library::TARGET.into(),
            },
            &rinzu_compiler::NativeCliInputs::default(),
        ) {
            failures.push((
                name,
                errors
                    .into_iter()
                    .map(|error| {
                        format!(
                            "{}:{} {}",
                            error.code.as_str(),
                            error.primary.start_byte,
                            error.message
                        )
                    })
                    .collect::<Vec<_>>(),
            ));
        }
        eprintln!("checked regression input {name} in {:?}", started.elapsed());
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
