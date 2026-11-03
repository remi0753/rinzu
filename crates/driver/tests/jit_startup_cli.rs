#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "rinzu-jit-cache-{}-{}/with spaces",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        for directory in ["sdk/src", "sdk/preferred", "sdk/headers", "app/src"] {
            std::fs::create_dir_all(root.join(directory)).unwrap();
        }
        std::fs::write(root.join("sdk/rinzu.toml"), "[package]\nname=\"model\"\nlanguage=\"0.5\"\nkind=\"sdk\"\nentry=\"src/model.rinzu\"\n[native]\nsources=[\"api.c\"]\nheaders=[\"headers/api.h\"]\ninclude-dirs=[\"preferred\",\"headers\"]\n").unwrap();
        std::fs::write(
            root.join("sdk/src/model.rinzu"),
            "native { fn answer() -> Int symbol \"answer\"; } export answer;",
        )
        .unwrap();
        std::fs::write(
            root.join("sdk/headers/api.h"),
            "#include <stdint.h>\nint64_t answer(void);\n",
        )
        .unwrap();
        std::fs::write(root.join("sdk/headers/value.h"), "#define ANSWER 42\n").unwrap();
        std::fs::write(
            root.join("sdk/api.c"),
            "#include \"value.h\"\nint64_t answer(void) {return ANSWER;}\n",
        )
        .unwrap();
        std::fs::write(root.join("app/rinzu.toml"), "[package]\nname=\"consumer\"\nlanguage=\"0.5\"\n[dependencies]\nmodel={path=\"../sdk\"}\n").unwrap();
        std::fs::write(root.join("app/src/main.rinzu"), "import model;import rinzu.stdio;fn main(){try{rinzu.stdio.writeLine(model.answer());}catch errors{panic(\"native call failed\");}}").unwrap();
        Self(root)
    }

    fn run(&self, arguments: &[&str], timings: bool) -> Output {
        self.run_jobs(arguments, timings, 2)
    }

    fn run_jobs(&self, arguments: &[&str], timings: bool, jobs: usize) -> Output {
        let mut command = Command::new(frozen_cli::path());
        command
            .arg("run")
            .arg(self.0.join("app"))
            .args(arguments)
            .env("RINZU_CACHE_DIR", self.0.join("cache"))
            .env("RINZU_NATIVE_JOBS", jobs.to_string())
            .env_remove("RINZU_TOOLCHAIN_ROOT")
            .env_remove("RINZU_COMPILE_TIMINGS");
        if timings {
            command.env("RINZU_COMPILE_TIMINGS", "1");
        }
        command.output().unwrap()
    }

    fn corrupt_artifacts(&self) {
        for entry in std::fs::read_dir(self.0.join("cache/native-compile-v1")).unwrap() {
            std::fs::write(entry.unwrap().path().join("artifact"), b"corrupt object").unwrap();
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
    }
}

fn success(output: &Output, expected: &[u8]) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, expected);
}

fn hit(output: &Output, action: &str) -> bool {
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .filter_map(|line| line.strip_prefix("RINZU_COMPILE_TIMING="))
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .any(|record| record["phase"] == "native.compile.hit" && record["detail"] == action)
}

#[test]
fn native_object_cache_rechecks_transitive_headers_search_precedence_and_corruption() {
    let fixture = Fixture::new();
    let cold = fixture.run(&[], true);
    success(&cold, b"42\n");
    assert!(!hit(&cold, "compile source SDK object"));
    let warm = fixture.run(&[], true);
    success(&warm, b"42\n");
    assert!(hit(&warm, "compile source SDK object"));
    std::fs::write(fixture.0.join("sdk/headers/value.h"), "#define ANSWER 43\n").unwrap();
    let changed = fixture.run(&[], true);
    success(&changed, b"43\n");
    assert!(!hit(&changed, "compile source SDK object"));
    // The compiler evaluates this builtin after preprocessing. Line-only
    // changes in an included file must invalidate the object as well.
    let location_header = "static inline int64_t answer_value(void) {return __builtin_LINE();}\n#define ANSWER answer_value()\n";
    std::fs::write(fixture.0.join("sdk/headers/value.h"), location_header).unwrap();
    success(&fixture.run(&[], true), b"1\n");
    std::fs::write(
        fixture.0.join("sdk/headers/value.h"),
        format!("\n{location_header}"),
    )
    .unwrap();
    let moved = fixture.run(&[], true);
    success(&moved, b"2\n");
    assert!(!hit(&moved, "compile source SDK object"));
    // This file was absent from the previous dependency list. Fresh
    // preprocessing must notice that it now wins the include search.
    std::fs::write(
        fixture.0.join("sdk/preferred/value.h"),
        "#define ANSWER 44\n",
    )
    .unwrap();
    let shadowed = fixture.run(&[], true);
    success(&shadowed, b"44\n");
    assert!(!hit(&shadowed, "compile source SDK object"));
    fixture.corrupt_artifacts();
    let recovered = fixture.run(&[], true);
    success(&recovered, b"44\n");
    assert!(!hit(&recovered, "compile source SDK object"));
    std::fs::write(
        fixture.0.join("sdk/headers/api.h"),
        "#include <stdint.h>\nint32_t answer(void);\n",
    )
    .unwrap();
    let invalid = fixture.run(&[], true);
    assert!(!invalid.status.success());
    assert!(
        String::from_utf8_lossy(&invalid.stderr).contains("native scalar ABI mismatch"),
        "{}",
        String::from_utf8_lossy(&invalid.stderr)
    );
}

#[test]
fn volatile_preprocessor_macros_keep_the_original_compilation_behavior() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.0.join("sdk/api.c"),
        "int64_t answer(void) {return __TIME__[0] ? 42 : 0;}\n",
    )
    .unwrap();
    for _ in 0..2 {
        let output = fixture.run(&[], true);
        success(&output, b"42\n");
        assert!(!hit(&output, "compile source SDK object"));
    }
}

#[test]
fn parallel_sdk_dependencies_preserve_descriptor_bytes_and_execution_order() {
    let fixture = Fixture::new();
    for directory in ["second/src", "bridge/src"] {
        std::fs::create_dir_all(fixture.0.join(directory)).unwrap();
    }
    std::fs::write(fixture.0.join("second/rinzu.toml"), "[package]\nname=\"second\"\nlanguage=\"0.5\"\nkind=\"sdk\"\nentry=\"src/second.rinzu\"\n[native]\nsources=[\"api.c\"]\nheaders=[\"api.h\"]\n").unwrap();
    std::fs::write(
        fixture.0.join("second/src/second.rinzu"),
        "native { fn answer() -> Int symbol \"second_answer\"; } export answer;",
    )
    .unwrap();
    std::fs::write(
        fixture.0.join("second/api.h"),
        "#include <stdint.h>\nint64_t second_answer(void);\n",
    )
    .unwrap();
    std::fs::write(
        fixture.0.join("second/api.c"),
        "int64_t second_answer(void) {return 7;}\n",
    )
    .unwrap();
    std::fs::write(fixture.0.join("bridge/rinzu.toml"), "[package]\nname=\"bridge\"\nlanguage=\"0.5\"\nkind=\"sdk\"\nentry=\"src/bridge.rinzu\"\n[dependencies]\nmodel={path=\"../sdk\"}\nsecond={path=\"../second\"}\n").unwrap();
    std::fs::write(fixture.0.join("bridge/src/bridge.rinzu"), "import model;import second;fn answer()->Int{return model.answer()+second.answer();}export answer;").unwrap();
    std::fs::write(fixture.0.join("app/rinzu.toml"), "[package]\nname=\"consumer\"\nlanguage=\"0.5\"\n[dependencies]\nbridge={path=\"../bridge\"}\n").unwrap();
    std::fs::write(fixture.0.join("app/src/main.rinzu"), "import bridge;import rinzu.stdio;fn main(){try{rinzu.stdio.writeLine(bridge.answer());}catch errors{panic(\"dependency failed\");}}").unwrap();
    let descriptors = || {
        let directory = fixture.0.join("app/.rinzu/generated/source-packages-v2");
        std::fs::read_dir(directory)
            .unwrap()
            .filter_map(|entry| {
                let entry = entry.unwrap();
                let path = entry.path().join("source-package-v2.json");
                path.is_file()
                    .then(|| (entry.file_name(), std::fs::read(path).unwrap()))
            })
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    success(&fixture.run_jobs(&[], true, 1), b"49\n");
    let serial = descriptors();
    assert_eq!(serial.len(), 3);
    success(&fixture.run_jobs(&[], true, 2), b"49\n");
    assert_eq!(descriptors(), serial);
}

#[test]
fn compiled_runtime_cache_preserves_observation_managed_edits_and_resource_reports() {
    let fixture = Fixture::new();
    let source = fixture.0.join("app/src/main.rinzu");
    std::fs::write(&source, "import model;import rinzu.stdio;import rinzu.timer;async fn main(){try{await rinzu.timer.sleep(1);rinzu.stdio.writeLine(model.answer());}catch errors{panic(\"native call failed\");}}").unwrap();
    let cold = fixture.run(&[], true);
    success(&cold, b"42\n");
    assert!(!hit(&cold, "compile async runtime source"));
    let warm = fixture.run(&[], true);
    success(&warm, b"42\n");
    assert!(hit(&warm, "compile async runtime source"));
    let traced = fixture.run(&["--trace"], true);
    success(&traced, b"42\n");
    assert!(!hit(&traced, "compile async runtime source"));
    std::fs::write(
        &source,
        std::fs::read_to_string(&source)
            .unwrap()
            .replace("model.answer()", "model.answer() + 1"),
    )
    .unwrap();
    success(&fixture.run(&[], true), b"43\n");
    let report = fixture.0.join("resources.jsonl");
    let arguments = [
        "--resource-report",
        report.to_str().unwrap(),
        "--resource-interval-ms",
        "0",
    ];
    success(&fixture.run(&arguments, true), b"43\n");
    let reported = fixture.run(&arguments, true);
    success(&reported, b"43\n");
    assert!(hit(&reported, "compile async runtime source"));
    assert!(Path::new(&report).is_file());
    fixture.corrupt_artifacts();
    success(&fixture.run(&[], true), b"43\n");
    let normal = fixture.run(&[], false);
    success(&normal, b"43\n");
    assert!(!String::from_utf8_lossy(&normal.stderr).contains("RINZU_COMPILE_TIMING="));
    let equivalence = Command::new(frozen_cli::path())
        .arg("verify-aot")
        .arg(fixture.0.join("app"))
        .env("RINZU_CACHE_DIR", fixture.0.join("cache"))
        .env("RINZU_COMPILE_TIMINGS", "1")
        .env_remove("RINZU_TOOLCHAIN_ROOT")
        .output()
        .unwrap();
    assert!(
        equivalence.status.success(),
        "{}",
        String::from_utf8_lossy(&equivalence.stderr)
    );
    assert!(String::from_utf8_lossy(&equivalence.stdout).contains("JIT/AOT equivalent"));
}
#[test]
fn native_image_query_hints_batch_current_uuids_and_recover_after_corruption() {
    let fixture = Fixture::new();
    let manifest = fixture.0.join("sdk/rinzu.toml");
    let mut text = std::fs::read_to_string(&manifest).unwrap();
    text.push_str("libraries=[\"System\"]\n");
    std::fs::write(&manifest, text).unwrap();
    let records = |output: &Output| -> Vec<serde_json::Value> {
        String::from_utf8_lossy(&output.stderr)
            .lines()
            .filter_map(|line| line.strip_prefix("RINZU_COMPILE_TIMING="))
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    };
    let uuid_commands = |output: &Output| {
        records(output)
            .iter()
            .filter(|record| {
                record["phase"] == "native.command"
                    && record["detail"]
                        .as_str()
                        .is_some_and(|detail| detail.starts_with("inspect native UUID/target"))
            })
            .count()
    };
    let hint_hit = |output: &Output| {
        records(output)
            .iter()
            .any(|record| record["phase"] == "native.image.hints.hit")
    };
    let cold = fixture.run(&[], true);
    success(&cold, b"42\n");
    assert!(!hint_hit(&cold));
    let warm = fixture.run(&[], true);
    success(&warm, b"42\n");
    assert!(hint_hit(&warm));
    assert!(uuid_commands(&warm) < uuid_commands(&cold));
    let mut corrupted = 0;
    for entry in std::fs::read_dir(fixture.0.join("cache/source-v4")).unwrap() {
        let hint = entry.unwrap().path().join("image-paths.json");
        if hint.is_file() {
            std::fs::write(hint, b"corrupt hints").unwrap();
            corrupted += 1;
        }
    }
    assert!(corrupted > 0);
    let recovered = fixture.run(&[], true);
    success(&recovered, b"42\n");
    assert!(!hint_hit(&recovered));
    let warm_again = fixture.run(&[], true);
    success(&warm_again, b"42\n");
    assert!(hint_hit(&warm_again));
    // A valid old System hint must not make its symbols available after the
    // explicit library is removed from the owner's current native scope.
    let text = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(&manifest, text.replace("libraries=[\"System\"]\n", "")).unwrap();
    std::fs::write(
        fixture.0.join("sdk/headers/api.h"),
        "#include <stdint.h>\n#include <unistd.h>\n",
    )
    .unwrap();
    std::fs::write(
        fixture.0.join("sdk/src/model.rinzu"),
        "native { fn answer() -> CInt symbol \"getpid\"; } export answer;",
    )
    .unwrap();
    std::fs::write(fixture.0.join("app/src/main.rinzu"), "import model;import rinzu.stdio;fn main(){try{rinzu.stdio.writeLine(Int(model.answer()));}catch errors{panic(\"native call failed\");}}").unwrap();
    let unscoped = fixture.run(&[], true);
    assert!(hint_hit(&unscoped));
    assert!(!unscoped.status.success());
    assert!(
        String::from_utf8_lossy(&unscoped.stderr).contains("unresolved native symbol `getpid`"),
        "{}",
        String::from_utf8_lossy(&unscoped.stderr)
    );
}
