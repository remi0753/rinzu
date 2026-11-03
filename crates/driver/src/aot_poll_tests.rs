use std::{fs, path::PathBuf, process::Command};

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
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

#[test]
fn control_poll_preserves_events_source_masks_counters_and_worker_atomic_observations() {
    let dir =
        Scratch(std::env::temp_dir().join(format!("rinzu-control-poll-{}", std::process::id())));
    fs::create_dir_all(&dir.0).unwrap();
    let runtime = include_str!("aot_source_async.c");
    let helpers = ["rinzu_aot_async_cancelled", "rinzu_aot_async_root_shutdown"]
        .map(|name| c_function(runtime, name))
        .join("\n");
    let poll_runtime = include_str!("aot_safepoint.c");
    let current = ["rinzu_aot_poll_control", "rinzu_aot_poll_context"]
        .map(|name| c_function(poll_runtime, name))
        .join("\n")
        .replace("rinzu_aot_poll_context", "poll_candidate");
    let source = include_str!("../tests/fixtures/poll_state_matrix.c")
        .replace("/* CURRENT_ASYNC_HELPERS */", &helpers)
        .replace("/* CURRENT_POLL_BODY */", &current);
    let file = dir.0.join("poll.c");
    let binary = dir.0.join("poll");
    fs::write(&file, source).unwrap();
    let output = Command::new("clang")
        .args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror"])
        .arg(&file)
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let output = Command::new(&binary).output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("256000-control-states"));
    for mode in ["old", "candidate"] {
        let output = Command::new(&binary).arg(mode).output().unwrap();
        assert_eq!(output.status.code(), Some(70), "{mode}: {output:?}");
    }
}
