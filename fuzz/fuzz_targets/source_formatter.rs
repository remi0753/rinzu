#![no_main]

use libfuzzer_sys::fuzz_target;
use rinzu_compiler::{FileId, SourceFile, format_source, parse_source};

fuzz_target!(|data: &[u8]| {
    // This target covers the bounded editor/formatter input, not type checking.
    // Keep a finite source size so malformed input cannot dominate a campaign.
    if data.len() > 4096 {
        return;
    }
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let source = SourceFile::new(FileId(0), "fuzz.rinzu", text);
    if parse_source(&source).is_err() {
        return;
    }
    let formatted = format_source(&source).expect("accepted syntax must format");
    let output = SourceFile::new(FileId(0), "fuzz.rinzu", formatted.as_str());
    parse_source(&output).expect("formatting must preserve valid syntax");
    assert_eq!(
        format_source(&output).expect("formatted syntax must format"),
        formatted,
        "formatting must be idempotent"
    );
});
