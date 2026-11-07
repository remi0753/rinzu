use std::{fs, path::PathBuf, process::Command};

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn control_poll_preserves_events_source_masks_counters_and_worker_atomic_observations() {
    let dir =
        Scratch(std::env::temp_dir().join(format!("rinzu-control-poll-{}", std::process::id())));
    fs::create_dir_all(&dir.0).unwrap();
    let file = dir.0.join("poll.c");
    let binary = dir.0.join("poll");
    fs::write(&file, super::poll_fixture::source()).unwrap();
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
