pub(super) fn source() -> String {
    let runtime = include_str!("../aot_source_async.c");
    let helpers = ["rinzu_aot_async_cancelled", "rinzu_aot_async_root_shutdown"]
        .map(|name| c_function(runtime, name))
        .join("\n");
    let poll_runtime = include_str!("../aot_safepoint.c");
    let current = ["rinzu_aot_poll_control", "rinzu_aot_poll_context"]
        .map(|name| c_function(poll_runtime, name))
        .join("\n")
        .replace("rinzu_aot_poll_context", "poll_candidate");
    include_str!("../../tests/fixtures/poll_state_matrix.c")
        .replace("/* CURRENT_ASYNC_HELPERS */", &helpers)
        .replace("/* CURRENT_POLL_BODY */", &current)
}

fn c_function<'a>(source: &'a str, symbol: &str) -> &'a str {
    let at = source.find(&format!("{symbol}(")).unwrap();
    let start = source[..at].rfind('\n').map_or(0, |p| p + 1);
    let brace = at + source[at..].find('{').unwrap();
    let mut depth = 1;
    for (offset, byte) in source.as_bytes()[brace + 1..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            return &source[start..brace + offset + 2];
        }
    }
    panic!("unterminated C function: {symbol}");
}
