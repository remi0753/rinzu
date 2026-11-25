#![cfg(feature = "test-native-lifetime-trace")]
#[path = "../../../tests/support/native.rs"]
mod native;
#[path = "../../../tests/support/process.rs"]
#[allow(dead_code)]
mod process;
use rinzu_runtime_model::native_lifetime::replay_joined;
use std::path::{Path, PathBuf};
struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn check(count: usize, events: &[String]) {
    let replay = |e: &[String], limit| replay_joined(count, 7183, e, limit);
    let r = replay(events, 100_000).unwrap_or_else(|e| panic!("{e}\n{}", events.join("\n")));
    let mut mutations = 0;
    for prefix in [
        "aux_admit,",
        "aux_bind,",
        "aux_publish,",
        "aux_copied,",
        "aux_claim,",
        "aux_physical,",
        "aux_settle,",
        "aux_release,",
        "aux_retire,",
        "helper_wake_begin,",
        "helper_wake_end,",
        "helper_quiescent,",
        "helper_join_wait,",
        "helper_joined,",
        "completion_claim,",
        "producer_released,",
    ] {
        let mut e = events.to_vec();
        e.remove(e.iter().position(|e| e.starts_with(prefix)).unwrap());
        assert!(replay(&e, 100_000).is_err(), "accepted missing {prefix}");
        mutations += 1;
    }
    for prefix in [
        "helper_wake_begin,",
        "helper_joined,",
        "aux_physical,",
        "completion_claim,",
    ] {
        let mut e = events.to_vec();
        let i = e.iter().position(|e| e.starts_with(prefix)).unwrap();
        e.insert(i, e[i].clone());
        assert!(replay(&e, 100_000).is_err());
        mutations += 1;
    }
    let mut e = events.to_vec();
    let from = e
        .iter()
        .position(|e| e.starts_with("helper_joined,"))
        .unwrap();
    let row = e.remove(from);
    let to = e
        .iter()
        .position(|e| e.starts_with("helper_wake_end,"))
        .unwrap();
    e.insert(to, row);
    assert!(replay(&e, 100_000).is_err());
    mutations += 1;
    let mut e = events.to_vec();
    let from = e.iter().position(|e| e.starts_with("aux_retire,")).unwrap();
    let row = e.remove(from);
    let to = e
        .iter()
        .position(|e| e.starts_with("aux_physical,"))
        .unwrap();
    e.insert(to, row);
    assert!(replay(&e, 100_000).is_err());
    mutations += 1;
    let mut e = events.to_vec();
    let from = e.iter().position(|e| e.starts_with("env_free,")).unwrap();
    let row = e.remove(from);
    let to = e
        .iter()
        .position(|e| e.starts_with("helper_wake_end,"))
        .unwrap();
    e.insert(to, row);
    assert!(replay(&e, 100_000).is_err());
    mutations += 1;
    let mut e = events.to_vec();
    e.insert(1, "invented,0,0,0".into());
    assert!(replay(&e, 100_000).is_err());
    mutations += 1;
    assert!(replay(&events[..events.len() - 1], 100_000).is_err());
    mutations += 1;
    assert!(replay(events, 1).is_err());
    mutations += 1;
    let mut wrong_claim = events.to_vec();
    let row = wrong_claim
        .iter_mut()
        .find(|e| e.starts_with("completion_claim,"))
        .unwrap();
    let f: Vec<_> = row.split(',').collect();
    *row = format!(
        "completion_claim,{},{},0",
        f[1],
        if f[2] == "1" { 0 } else { 1 }
    );
    assert!(replay(&wrong_claim, 100_000).is_err());
    mutations += 1;
    println!(
        "JOINED TRACE waiters={count} events={} frontier={} mutations={mutations}",
        r.events, r.maximum_frontier
    );
}
#[test]
fn c_polling_helper_join_and_forced_claim_window_refine_the_model_with_sanitizers() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let tmp =
        Temporary(std::env::temp_dir().join(format!("rinzu-joined-trace-{}", std::process::id())));
    std::fs::create_dir_all(&tmp.0).unwrap();
    for flags in [
        vec!["-O2"],
        vec![
            "-O1",
            "-g",
            "-fsanitize=address,undefined",
            "-fno-sanitize-recover=all",
        ],
    ] {
        let binary = tmp.0.join("trace");
        let build = process::bounded_tool(
            native::clang()
                .args(["-std=c11", "-pthread", "-Wall", "-Wextra", "-Werror"])
                .args(&flags)
                .arg("-I")
                .arg(root.join("include"))
                .arg(root.join("crates/runtime/tests/fixtures/joined_lifetime_trace.c"))
                .arg("-o")
                .arg(&binary),
        );
        assert!(build.status.success(), "{build:?}");
        let result = process::bounded_tool(
            std::process::Command::new(&binary)
                .env("RINZU_NATIVE_COMPLETION_DIRECT", "0")
                .env("RINZU_NATIVE_COMPLETION_NOTIFY", "1")
                .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
                .env("UBSAN_OPTIONS", "halt_on_error=1"),
        );
        assert!(result.status.success(), "{result:?}");
        let mut count = None;
        let mut events = Vec::new();
        let mut cases = 0;
        for line in String::from_utf8(result.stdout).unwrap().lines() {
            if let Some(start) = line.strip_prefix("BEGIN ") {
                assert!(count.is_none());
                count = Some(start.split_whitespace().next().unwrap().parse().unwrap());
            } else if line == "END" {
                check(count.take().unwrap(), &events);
                events.clear();
                cases += 1;
            } else {
                events.push(line.to_owned());
            }
        }
        assert!(count.is_none() && events.is_empty());
        assert_eq!(cases, 10);
    }
}
