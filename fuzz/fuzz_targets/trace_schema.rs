#![no_main]

use std::io::Cursor;

use libfuzzer_sys::fuzz_target;
use rinzu_trace::TraceDocument;

fuzz_target!(|data: &[u8]| {
    if let Ok(trace) = TraceDocument::from_reader(Cursor::new(data)) {
        trace.validate().expect("validated trace must remain valid");
        let _semantic = trace.semantic();
    }
});
