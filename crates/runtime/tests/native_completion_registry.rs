#[test]
fn completed_native_futures_do_not_remain_in_the_active_registry() {
    let directory =
        std::env::temp_dir().join(format!("rinzu-native-registry-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let source = directory.join("registry.c");
    std::fs::write(
        &source,
        include_str!("fixtures/native_completion_registry.c"),
    )
    .unwrap();
    let binary = directory.join("registry");
    let output = std::process::Command::new("clang")
        .args([
            "-std=c11",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pthread",
            "-fsanitize=address,undefined",
        ])
        .arg("-I")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/../../include"))
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    for active in ["0", "1"] {
        let output = std::process::Command::new(&binary)
            .env("RINZU_NATIVE_COMPLETION_DIRECT", "1")
            .env("RINZU_NATIVE_COMPLETION_ACTIVE_ONLY", active)
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .env("UBSAN_OPTIONS", "halt_on_error=1")
            .output()
            .unwrap();
        assert!(output.status.success(), "active={active}: {output:?}");
    }
    std::fs::remove_dir_all(directory).unwrap();
}
