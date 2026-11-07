#![cfg(unix)]
use std::path::PathBuf;
use std::process::Command;

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn c_value_leaves_keep_canonical_bits_float_equality_and_owner_checks() {
    let directory =
        Scratch(std::env::temp_dir().join(format!("rinzu-value-cpu-{}", std::process::id())));
    std::fs::create_dir_all(&directory.0).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for sanitized in [false, true] {
        let binary = directory.0.join("probe");
        let mut build = Command::new("clang");
        build.args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread"]);
        if sanitized {
            build.args([
                "-DRINZU_VERIFY_ALLOCATIONS=1",
                "-g",
                "-fsanitize=address,undefined",
                "-fno-sanitize-recover=all",
            ]);
        }
        let result = build
            .arg("-I")
            .arg(root.join("include"))
            .arg(root.join("crates/runtime/examples/value_cpu_probe.c"))
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        for (mode, code) in [
            ("verify", 0),
            ("verify-map", 0),
            ("verify-snapshot", 0),
            ("foreign-owner", 70),
            ("foreign-snapshot", 70),
        ] {
            let result = Command::new(&binary)
                .arg(mode)
                .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
                .output()
                .unwrap();
            assert_eq!(result.status.code(), Some(code), "{result:?}");
        }
    }
}

#[test]
fn c_map_indexes_compare_colliding_keys_and_validate_unmatched_tails() {
    let directory = Scratch(
        std::env::temp_dir().join(format!("rinzu-value-collisions-{}", std::process::id())),
    );
    let include = directory.0.join("rinzu");
    std::fs::create_dir_all(&include).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for entry in std::fs::read_dir(root.join("include/rinzu")).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            std::fs::copy(&path, include.join(path.file_name().unwrap())).unwrap();
        }
    }
    let key = include.join("key.h");
    let source = std::fs::read_to_string(&key).unwrap();
    let formula = "(hash->hash ^ bytes[index]) * UINT64_C(0x100000001b3)";
    assert_eq!(source.matches(formula).count(), 1);
    // Test-only headers make every full hash collide, including distinct keys.
    std::fs::write(
        &key,
        source.replace(formula, "hash->hash + (bytes[index] & 0)"),
    )
    .unwrap();
    let binary = directory.0.join("probe");
    let result = Command::new("clang")
        .args([
            "-DRINZU_VERIFY_ALLOCATIONS=1",
            "-std=c11",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pthread",
            "-fsanitize=address,undefined",
            "-fno-sanitize-recover=all",
        ])
        .arg("-I")
        .arg(&directory.0)
        .arg(root.join("crates/runtime/examples/value_cpu_probe.c"))
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    let result = Command::new(&binary)
        .arg("verify-map")
        .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
}
