use rinzu_compiler::check_package;

#[test]
fn sdk_usage_fixtures_check_as_real_entries() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut failures = Vec::new();
    let filter = std::env::var("RINZU_MIGRATION_FILTER").unwrap_or_default();
    for name in [
        "sdk/process/tests/app/src/main.rinzu",
        "sdk/socket/tests/app/src/main.rinzu",
        "sdk/socket/tests/client/src/main.rinzu",
        "sdk/socket/tests/server/src/main.rinzu",
        "sdk/io/tests/dynamic.rinzu",
        "sdk/io/tests/cancel.rinzu",
        "sdk/io/tests/main.rinzu",
        "sdk/log/tests/main.rinzu",
        "sdk/http/tests/main.rinzu",
        "sdk/http/tests/stress.rinzu",
        "sdk/stream/tests/basic.rinzu",
        "sdk/stream/tests/stress.rinzu",
        "sdk/stream/tests/parallel.rinzu",
        "sdk/stream/tests/admission.rinzu",
        "sdk/sqlite/tests/owned.rinzu",
        "sdk/sqlite/tests/owned-cancel.rinzu",
        "sdk/sqlite/tests/native-results.rinzu",
        "sdk/sqlite/tests/native-observer.rinzu",
        "sdk/sqlite/tests/rows/src/main.rinzu",
        "sdk/macos/tests/graphics/src/main.rinzu",
        "sdk/sdl2/tests/native-records/src/main.rinzu",
    ] {
        if !name.contains(&filter) {
            continue;
        }
        let started = std::time::Instant::now();
        eprintln!("checking fixture {name}");
        let temporary =
            std::env::temp_dir().join(format!("rinzu-migration-{}", std::process::id()));
        let result = if name.starts_with("sdk/macos/") || name.starts_with("sdk/sdl2/") {
            check_package(root.join(name).parent().unwrap().parent().unwrap()).map(|_| ())
        } else if name.starts_with("sdk/sqlite/") || name.starts_with("sdk/log/") {
            let sdk = if name.starts_with("sdk/sqlite/") {
                "sqlite"
            } else {
                "log"
            };
            std::fs::create_dir_all(temporary.join("src")).unwrap();
            std::fs::copy(root.join(name), temporary.join("src/main.rinzu")).unwrap();
            copy_sdk(&root.join("sdk").join(sdk), &temporary.join("sdk"));
            if name.ends_with("owned-cancel.rinzu") {
                // This fixture is run with the same private receipt gate in
                // the resource regression runner, not a production SDK API.
                let api = temporary.join("sdk/src/sqlite.rinzu");
                let source=std::fs::read_to_string(&api).unwrap().replacen("native {",r#"async fn gateCompleted(receipt: GateReceipt) -> Unit { await nativeCompleted(receipt); }
native {
                    opaque GateReceipt;
                    fn rawGateReceipt()->GateReceipt symbol "rinzu_owned_gate_receipt";

                    fn gateRelease()->Unit symbol "rinzu_owned_gate_release";
                "#,1);
                std::fs::write(api,format!("{source}\nasync fn gateStarted(){{await gateCompleted(rawGateReceipt());}}\nexport gateStarted;export gateRelease;\n")).unwrap();
                let header = temporary.join("sdk/native/include/rinzu_sqlite.h");
                let source = std::fs::read_to_string(&header).unwrap();
                std::fs::write(header,format!("{source}\nvoid *rinzu_owned_gate_receipt(void);\nvoid rinzu_owned_gate_release(void);\n")).unwrap();
            }
            std::fs::write(temporary.join("rinzu.toml"), format!(
                "[package]\nname=\"migration_fixture\"\nlanguage=\"0.5\"\n[dependencies]\n\"rinzu.{sdk}\"={{path=\"sdk\"}}\n"
            )).unwrap();
            let result = check_package(&temporary).map(|_| ());
            std::fs::remove_dir_all(&temporary).unwrap();
            result
        } else {
            rinzu_compiler::check_input(
                &root.join(name),
                &rinzu_compiler::CompilationOptions {
                    language: None,
                    target: rinzu_compiler::standard_library::TARGET.into(),
                },
                &rinzu_compiler::NativeCliInputs::default(),
            )
            .map(|_| ())
        };
        eprintln!("checked fixture {name} in {:?}", started.elapsed());
        match result {
            Ok(_) => {}
            Err(errors) => failures.push((
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
            )),
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn regression_programs_check_as_real_entries() {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/programs");
    let mut failures = Vec::new();
    let filter = std::env::var("RINZU_MIGRATION_FILTER").unwrap_or_default();
    for name in [
        "file-copy",
        "editor",
        "macos-charts",
        "macos-data-browser",
        "macos-image-viewer",
        "macos-metal",
        "macos-notepad",
        "macos-task-list",
        "macos-widgets",
        "source-callback",
        "async-spawn",
        "async-services",
        "formatted-strings",
        "multi-file",
        "sdl2",
        "aggregation",
        "byte-chunks",
        "bytes",
        "collections",
        "constants",
        "errors",
        "generics",
        "numeric",
        "raw-text",
        "results",
        "text",
        "traversal",
        "classes",
        "lambdas",
        "records",
        "try-all",
        "try-all-control",
    ] {
        if !name.contains(&filter) {
            continue;
        }
        let started = std::time::Instant::now();
        eprintln!("checking regression program {name}");
        if let Err(errors) = check_package(&root.join(name)) {
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
        eprintln!(
            "checked regression program {name} in {:?}",
            started.elapsed()
        );
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

fn copy_sdk(source: &std::path::Path, destination: &std::path::Path) {
    fn directory(source: &std::path::Path, destination: &std::path::Path) {
        std::fs::create_dir_all(destination).unwrap();
        for entry in std::fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let to = destination.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                directory(&entry.path(), &to);
            } else {
                std::fs::copy(entry.path(), to).unwrap();
            }
        }
    }
    std::fs::create_dir_all(destination).unwrap();
    std::fs::copy(source.join("rinzu.toml"), destination.join("rinzu.toml")).unwrap();
    for name in ["src", "native"] {
        if source.join(name).is_dir() {
            directory(&source.join(name), &destination.join(name));
        }
    }
}
