#![cfg(unix)]
use std::process::Command;

#[test]
fn root_registration_preserves_domain_uniqueness_and_detaches_in_any_order() {
    let directory =
        std::env::temp_dir().join(format!("rinzu-root-registry-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for sanitized in [false, true] {
        let executable = directory.join(if sanitized { "san" } else { "plain" });
        let mut compiler = Command::new("clang");
        compiler.args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread"]);
        if sanitized {
            compiler.args(["-fsanitize=address,undefined", "-fno-sanitize-recover=all"]);
        }
        let output = compiler
            .arg("-I")
            .arg(manifest.join("../../include"))
            .arg(manifest.join("tests/fixtures/managed_root_registry.c"))
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let output = Command::new(&executable).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        for (mode, code) in [
            ("same-roots", 70),
            ("other-domain", 70),
            ("attached-dispose", 70),
            ("wrong-entry", 70),
            ("broken-link", 70),
            ("count-wrong-domain", 70),
            ("count-underflow", 70),
            ("count-overflow", 71),
        ] {
            let output = Command::new(&executable).arg(mode).output().unwrap();
            assert_eq!(output.status.code(), Some(code), "{mode}: {output:?}");
            if code == 71 {
                assert!(
                    String::from_utf8_lossy(&output.stderr).contains("resource unavailable (71)")
                );
            } else {
                assert!(output.stderr.is_empty(), "{mode}: {output:?}");
            }
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}
