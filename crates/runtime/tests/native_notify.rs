#![cfg(unix)]
use std::path::Path;
use std::process::Command;

#[test]
fn notified_native_carriers_preserve_registration_races_and_target_lifetime() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let directory =
        std::env::temp_dir().join(format!("rinzu-native-notify-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    for fixture in ["native_notify", "native_transfer"] {
        for sanitized in [false, true] {
            let binary = directory.join(format!(
                "{fixture}-{}",
                if sanitized { "sanitized" } else { "plain" }
            ));
            let mut build = Command::new("clang");
            build.args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread"]);
            if sanitized {
                build.args(["-fsanitize=address,undefined", "-fno-sanitize-recover=all"]);
            }
            let output = build
                .arg("-I")
                .arg(root.join("include"))
                .arg(root.join(format!("crates/runtime/tests/fixtures/{fixture}.c")))
                .arg("-o")
                .arg(&binary)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            for notify in ["0", "1"] {
                for direct in ["0", "1"] {
                    for active in ["0", "1"] {
                        let output = Command::new(&binary)
                            .env("RINZU_NATIVE_COMPLETION_NOTIFY", notify)
                            .env("RINZU_NATIVE_COMPLETION_DIRECT", direct)
                            .env("RINZU_NATIVE_COMPLETION_ACTIVE_ONLY", active)
                            .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
                            .env("UBSAN_OPTIONS", "halt_on_error=1")
                            .output()
                            .unwrap();
                        assert!(
                            output.status.success(),
                            "sanitized={sanitized} notify={notify} direct={direct} active={active}: {output:?}"
                        );
                    }
                }
            }
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}
