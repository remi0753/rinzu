use rinzu_bindgen::validate_metadata;

const VALID: &str = r#"
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
        "lifetime": "call-only",
        "retainReleaseThreadSafe": null
      }],
      "delivery": "inline",
      "nestedDelivery": "guaranteed",
      "unregister": { "kind": "function", "symbol": "fixture_unregister", "asynchronous": false },
      "shutdown": { "kind": "unregister" },
      "failureReturn": "FIXTURE_HANDLER_FAILED"
    }]
  }]
}
"#;

#[test]
fn complete_closed_metadata_is_safe() {
    let metadata = validate_metadata(VALID).expect("complete metadata must be safe");
    assert_eq!(metadata.get().library, "fixture");
}

#[test]
fn missing_or_unknown_contract_values_are_raw_only() {
    for invalid in [
        VALID.replace("\"blocking\": \"non-blocking\",", ""),
        VALID.replace("\"owner-sync\"", "\"surprise-callback\""),
        VALID.replace("\"call-only\"", "\"borrowed-until-later\""),
        VALID.replace(
            "\"nestedDelivery\": \"guaranteed\"",
            "\"nestedDelivery\": \"unknown\"",
        ),
        VALID.replace(
            "\"shutdown\": { \"kind\": \"unregister\" }",
            "\"shutdown\": { \"kind\": \"maybe\" }",
        ),
        VALID.replace(
            "\"synchronousReentry\": \"may-callback\"",
            "\"synchronousReentry\": \"unknown\"",
        ),
    ] {
        let errors = validate_metadata(&invalid).expect_err("unknown contracts must be RawOnly");
        assert_eq!(errors[0].code, "RZB0001");
    }
}

#[test]
fn unsafe_cross_field_combinations_are_raw_only() {
    let call_only_async = VALID
        .replace("\"owner-sync\"", "\"owner-async\"")
        .replace("\"inline\"", "\"serial-bounded\"")
        .replace(
            "\"failureReturn\": \"FIXTURE_HANDLER_FAILED\"",
            "\"failureReturn\": null",
        );
    let errors = validate_metadata(&call_only_async).expect_err("CallOnly cannot escape");
    assert!(errors.iter().any(|error| error.code == "RZB0020"));

    let foreign_sync = VALID.replace("\"owner-sync\"", "\"foreign-sync\"");
    let errors = validate_metadata(&foreign_sync).expect_err("ForeignSync is unsafe in v1");
    assert!(errors.iter().any(|error| error.code == "RZB0011"));

    let missing_exception_boundary = VALID.replace(
        "\"exceptionBoundary\": true",
        "\"exceptionBoundary\": false",
    );
    let errors = validate_metadata(&missing_exception_boundary)
        .expect_err("callbacks require exception containment");
    assert!(errors.iter().any(|error| error.code == "RZB0008"));
}

#[test]
fn schema_is_shipped_and_declares_every_required_contract_axis() {
    let schema = include_str!("../schema/binding-metadata-v1.schema.json");
    let value: serde_json::Value = serde_json::from_str(schema).unwrap();
    assert_eq!(
        value["properties"]["schema"]["const"],
        "rinzu.binding-metadata"
    );
    for axis in [
        "class",
        "lifetime",
        "synchronousReentry",
        "blocking",
        "nestedDelivery",
        "unregister",
        "shutdown",
    ] {
        assert!(schema.contains(axis), "schema is missing {axis}");
    }
}
