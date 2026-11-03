use std::fs;
use std::process::Command;

use rinzu_bindgen::{generate_binding, validate_metadata};

const HEADER: &str = r#"
#include <stdint.h>
typedef int32_t (*fixture_handler_t)(void *context, int32_t event);
int32_t fixture_register_handler(void *context, fixture_handler_t handler);
void fixture_unregister(void *context);
"#;

const METADATA: &str = r#"
{
  "schema": "rinzu.binding-metadata",
  "version": 1,
  "library": "fixture",
  "header": "fixture.h",
  "functions": [{
    "name": "register_handler",
    "symbol": "fixture_register_handler",
    "parameters": [
      { "name": "context", "cType": "void *" },
      { "name": "handler", "cType": "fixture_handler_t" }
    ],
    "resultCType": "int32_t",
    "thread": "process-main",
    "blocking": "non-blocking",
    "synchronousReentry": "may-callback",
    "exceptionBoundary": true,
    "callbacks": [{
      "parameter": "handler",
      "contextParameter": "context",
      "resultCType": "int32_t",
      "class": "owner-sync",
      "arguments": [{
        "name": "event",
        "cType": "int32_t",
        "lifetime": "value-copy",
        "retainReleaseThreadSafe": null
      }],
      "delivery": "inline",
      "nestedDelivery": "guaranteed",
      "unregister": { "kind": "function", "symbol": "fixture_unregister", "asynchronous": false },
      "shutdown": { "kind": "unregister" },
      "failureReturn": "-1"
    }]
  }]
}
"#;

#[test]
fn generator_emits_raw_safe_trampoline_and_conformance_artifacts() {
    let metadata = validate_metadata(METADATA).unwrap();
    let generated = generate_binding(HEADER, &metadata).unwrap();
    assert!(
        generated
            .raw_header
            .contains("rinzu_raw_fixture_register_handler")
    );
    assert!(
        generated
            .safe_header
            .contains("rinzu_safe_fixture_register_handler")
    );
    assert!(
        generated
            .trampoline_source
            .contains("rinzu_generated_callback_dispatch_v1")
    );
    assert!(
        generated
            .conformance_source
            .contains("rinzu_fixture_binding_conformance")
    );
    assert!(
        generated
            .descriptor_json
            .contains("rinzu.fixture.register_handler")
    );
}

#[test]
fn generator_rejects_header_drift_and_variadics() {
    let metadata = validate_metadata(METADATA).unwrap();
    let errors = generate_binding("int32_t other(void);", &metadata).unwrap_err();
    assert!(errors[0].message.contains("not found"));

    let variadic = HEADER.replace(
        "void *context, fixture_handler_t handler",
        "void *context, fixture_handler_t handler, ...",
    );
    let errors = generate_binding(&variadic, &metadata).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("variadic"))
    );
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn generated_c_artifacts_pass_the_target_c_compiler() {
    let metadata = validate_metadata(METADATA).unwrap();
    let generated = generate_binding(HEADER, &metadata).unwrap();
    let directory = std::env::temp_dir().join(format!("rinzu-bindgen-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("fixture.h"), HEADER).unwrap();
    fs::write(directory.join("fixture_raw.h"), generated.raw_header).unwrap();
    fs::write(directory.join("fixture_safe.h"), generated.safe_header).unwrap();
    fs::write(
        directory.join("fixture_trampoline.c"),
        generated.trampoline_source,
    )
    .unwrap();
    fs::write(
        directory.join("fixture_conformance.c"),
        generated.conformance_source,
    )
    .unwrap();

    for source in ["fixture_trampoline.c", "fixture_conformance.c"] {
        let output = Command::new("clang")
            .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-fsyntax-only"])
            .arg(directory.join(source))
            .arg("-I")
            .arg(&directory)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fs::remove_dir_all(directory).unwrap();
}
