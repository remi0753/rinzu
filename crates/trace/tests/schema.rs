use std::fs::File;
use std::path::PathBuf;

use rinzu_trace::{TraceDocument, TraceError};

fn workspace_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

#[test]
fn valid_fixture_parses_and_normalization_drops_observations() {
    let path = workspace_path("tests/conformance/trace/valid/basic.json");
    let trace = TraceDocument::from_reader(File::open(path).unwrap()).unwrap();
    let mut with_other_observations = trace.clone();
    for event in &mut with_other_observations.events {
        if let Some(observation) = &mut event.observation {
            observation.monotonic_ns = Some(999_999);
            observation.os_thread_id = Some("different-thread".into());
            observation.address = Some("0xfeed".into());
        }
    }
    assert_eq!(trace.semantic(), with_other_observations.semantic());
}

#[test]
fn invalid_fixture_fails_semantic_validation() {
    let path = workspace_path("tests/conformance/trace/invalid/noncontiguous.json");
    let failure = TraceDocument::from_reader(File::open(path).unwrap()).unwrap_err();
    assert!(matches!(
        failure,
        TraceError::NonContiguousSequence {
            expected: 1,
            actual: 2
        }
    ));
}
