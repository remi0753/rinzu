use rinzu_runtime::resource_accounting::{Attribution, Category};

pub(crate) fn c_string(text: &str) -> String {
    let mut result = String::from("\"");
    for byte in text.bytes() {
        match byte {
            b'"' => result.push_str("\\\""),
            b'\\' => result.push_str("\\\\"),
            32..=126 => result.push(byte as char),
            _ => result.push_str(&format!("\\{byte:03o}")),
        }
    }
    result.push('"');
    result
}

pub(crate) fn native_end(evidence: rinzu_codegen::SourceNativeCallEvidence) -> Vec<Category> {
    let row = |id, unit, live, allocated, released, subset| Category {
        id,
        scope: "run",
        unit,
        status: "measured",
        live,
        allocated,
        released,
        peak: None,
        retention_reason: "Completed source native call/foreign ledger for this run; no SDK allocation inference",
        subset_of: subset,
        attribution: Attribution {
            precision: "run",
            ..Attribution::unknown()
        },
    };
    vec![
        row(
            "native_calls_run",
            "count",
            Some(evidence.live_calls as u64),
            Some(evidence.prepared as u64),
            Some((evidence.released + evidence.abandoned) as u64),
            Some("native_calls"),
        ),
        row(
            "native_copy_run",
            "bytes",
            None,
            Some(evidence.copied_bytes as u64),
            None,
            Some("native_copy"),
        ),
        row(
            "foreign_queue",
            "bytes",
            Some(evidence.foreign.bytes as u64),
            None,
            None,
            None,
        ),
        row(
            "foreign_records",
            "count",
            Some(evidence.foreign.live as u64),
            Some(evidence.foreign.created),
            Some(evidence.foreign.retired),
            None,
        ),
    ]
}
