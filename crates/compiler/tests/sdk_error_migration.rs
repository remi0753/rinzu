use rinzu_compiler::check_package;
#[test]
fn sdk_apis_check_with_direct_returns_and_error_payloads() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut failures = Vec::new();
    for name in [
        "io",
        "process",
        "stream",
        "testing",
        "shutdown",
        "http",
        "socket",
        "codec",
        "log",
        "sqlite",
        "macos",
        "shim-example",
    ] {
        match check_package(&root.join("sdk").join(name)) {
            Err(errors) => failures.push((name, errors)),
            Ok((_, artifact)) => {
                assert!(
                    !artifact
                        .ir
                        .source_types
                        .keys()
                        .any(|ty| ty.starts_with("Result<")),
                    "{name}"
                );
                if name == "http" || name == "socket" {
                    let error_name = if name == "http" {
                        "HttpFailure"
                    } else {
                        "WriteFailure"
                    };
                    let (_, schema) = artifact
                        .ir
                        .source_types
                        .iter()
                        .find(|(name, _)| name.ends_with(error_name))
                        .unwrap();
                    assert_eq!(schema.kind, "error");
                    assert_eq!(schema.fields[4].0, "payload");
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
