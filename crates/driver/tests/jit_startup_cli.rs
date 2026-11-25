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
        self.command(arguments, timings, jobs).output().unwrap()
    }

    fn command(&self, arguments: &[&str], timings: bool, jobs: usize) -> Command {
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
        command
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

fn timing_records(output: &Output) -> impl Iterator<Item = serde_json::Value> + '_ {
    output
        .stderr
        .split(|byte| *byte == b'\n')
        .filter_map(|line| line.strip_prefix(b"RINZU_COMPILE_TIMING="))
        .map(|record| serde_json::from_slice(record).unwrap())
}

fn hit(output: &Output, action: &str) -> bool {
    timing_records(output)
        .any(|record| record["phase"] == "native.compile.hit" && record["detail"] == action)
}

fn phase(output: &Output, name: &str) -> bool {
    timing_records(output).any(|event| event["phase"] == name)
}

fn machine_code_counts(output: &Output, phase: &str) -> (u64, u64) {
    let event = timing_records(output)
        .find(|event| event["phase"] == phase)
        .unwrap_or_else(|| {
            panic!(
                "missing {phase}: {}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
    let counters = &event["counters"];
    (
        counters["generated"].as_u64().unwrap(),
        counters["reused"].as_u64().unwrap(),
    )
}

fn runtime_links(output: &Output) -> usize {
    timing_records(output)
        .filter(|record| record["phase"] == "jit.runtime.link")
        .count()
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
    let source = fixture.0.join("app/src/main.rinzu");
    let edited = std::fs::read_to_string(&source)
        .unwrap()
        .replace("model.answer()", "model.answer() + 1");
    std::fs::write(source, edited).unwrap();
    let invalid = fixture.run(&[], true);
    assert!(!invalid.status.success());
    assert!(
        timing_records(&invalid).any(|event| {
            event["phase"] == "frontend.sdk-cache.hit" && event["detail"] == "model"
        })
    );
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
    let run = |arguments: &[&str], timings| {
        // Cargo's test runner adds this override for the Rust toolchain.
        fixture
            .command(arguments, timings, 2)
            .env_remove("DYLD_FALLBACK_LIBRARY_PATH")
            .output()
            .unwrap()
    };
    let source = fixture.0.join("app/src/main.rinzu");
    std::fs::write(&source, "import model;import rinzu.stdio;import rinzu.timer;fn adjustment()->Int{return 0;}async fn main(){try{await rinzu.timer.sleep(1);rinzu.stdio.writeLine(model.answer()+adjustment());}catch errors{panic(\"native call failed\");}}").unwrap();
    let cold = run(&[], true);
    success(&cold, b"42\n");
    assert!(!hit(&cold, "compile async runtime source"));
    assert_eq!(runtime_links(&cold), 1);
    assert!(phase(&cold, "jit.frontend.cache.store"));
    assert!(phase(&cold, "jit.runtime.exact.store"));
    assert!(phase(&cold, "jit.managed.cache.store"));
    assert!(phase(&cold, "jit.managed.function-cache.store"));
    let (cold_functions, reused) = machine_code_counts(&cold, "jit.managed.lower");
    assert!(cold_functions > 0);
    assert_eq!(reused, 0);
    let (cold_stubs, reused) = machine_code_counts(&cold, "jit.managed.analysis");
    assert!(cold_stubs > 0);
    assert_eq!(reused, 0);
    assert!(timing_records(&cold).any(|event| {
        event["phase"] == "jit.managed.analysis"
            && event["counters"]["native_stubs"].as_u64().unwrap_or(0) > 0
    }));
    let warm = run(&[], true);
    success(&warm, b"42\n");
    assert!(phase(&warm, "jit.frontend.cache.hit"));
    assert!(phase(&warm, "jit.runtime.exact.hit"));
    assert!(phase(&warm, "jit.runtime.cache.codesign"));
    assert!(phase(&warm, "jit.managed.cache.hit"));
    assert!(!phase(&warm, "jit.managed.function-cache.lookup"));
    assert!(!phase(&warm, "jit.managed.lower"));
    assert!(!phase(&warm, "jit.managed.analysis"));
    let publication = timing_records(&warm)
        .find(|event| event["phase"] == "jit.managed.publish")
        .unwrap();
    let audit = &publication["counters"];
    assert!(audit["executable_mappings"].as_u64().unwrap() > 0);
    assert_eq!(audit["executable_mappings"], audit["rx_transitions"]);
    assert_eq!(audit["executable_mappings"], audit["icache_flushes"]);
    assert_eq!(audit["rwx_mappings"], 0);
    assert!(!phase(&warm, "frontend"));
    assert!(!phase(&warm, "jit.runtime.generate"));
    assert!(!hit(&warm, "compile async runtime source"));
    assert_eq!(
        runtime_links(&warm),
        0,
        "{}",
        String::from_utf8_lossy(&warm.stderr)
    );
    let generation = std::fs::read_dir(fixture.0.join("cache/jit-code-v1"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let machine = generation.join("code.bin");
    std::fs::write(&machine, b"partial machine code").unwrap();
    let repaired_code = run(&[], true);
    success(&repaired_code, b"42\n");
    assert!(phase(&repaired_code, "jit.runtime.exact.hit"));
    assert!(phase(&repaired_code, "jit.managed.cache.miss"));
    assert_eq!(
        machine_code_counts(&repaired_code, "jit.managed.lower"),
        (0, cold_functions)
    );
    assert_eq!(
        machine_code_counts(&repaired_code, "jit.managed.analysis"),
        (0, cold_stubs)
    );
    assert!(phase(&repaired_code, "jit.managed.cache.store"));
    let copy = generation.join("saved-code.bin");
    std::fs::rename(&machine, &copy).unwrap();
    std::os::unix::fs::symlink(&copy, &machine).unwrap();
    let rejected_symlink = run(&[], true);
    success(&rejected_symlink, b"42\n");
    assert!(phase(&rejected_symlink, "jit.managed.cache.miss"));
    assert!(phase(&rejected_symlink, "jit.managed.cache.store"));
    assert!(std::fs::symlink_metadata(&machine).unwrap().is_file());
    let traced = run(&["--trace"], true);
    success(&traced, b"42\n");
    assert!(phase(&traced, "jit.frontend.cache.hit"));
    assert!(phase(&traced, "jit.managed.cache.hit"));
    assert!(!phase(&traced, "jit.runtime.exact.hit"));
    assert!(hit(&traced, "compile async runtime source"));
    assert_eq!(runtime_links(&traced), 1);
    std::fs::write(
        &source,
        std::fs::read_to_string(&source)
            .unwrap()
            .replace("return 0;", "return 1;"),
    )
    .unwrap();
    let edited = run(&[], true);
    success(&edited, b"43\n");
    assert_eq!(runtime_links(&edited), 0);
    assert!(phase(&edited, "frontend"));
    assert!(!phase(&edited, "jit.frontend.cache.hit"));
    assert!(!phase(&edited, "jit.managed.cache.hit"));
    let (generated, reused) = machine_code_counts(&edited, "jit.managed.lower");
    assert!(generated > 0 && reused > 0);
    assert_eq!(generated + reused, cold_functions);
    assert_eq!(
        machine_code_counts(&edited, "jit.managed.analysis"),
        (0, cold_stubs)
    );
    let report = fixture.0.join("resources.jsonl");
    let arguments = [
        "--resource-report",
        report.to_str().unwrap(),
        "--resource-interval-ms",
        "0",
    ];
    let first_report = run(&arguments, true);
    success(&first_report, b"43\n");
    assert_eq!(runtime_links(&first_report), 1);
    let reported = run(&arguments, true);
    success(&reported, b"43\n");
    assert!(phase(&reported, "jit.frontend.cache.hit"));
    assert!(phase(&reported, "jit.runtime.exact.hit"));
    assert!(phase(&reported, "jit.managed.cache.hit"));
    assert_eq!(runtime_links(&reported), 0);
    assert!(Path::new(&report).is_file());
    for entry in std::fs::read_dir(fixture.0.join("cache/jit-runtime-v1")).unwrap() {
        std::fs::write(
            entry.unwrap().path().join("compiled-async.dylib"),
            b"bad runtime image",
        )
        .unwrap();
    }
    let rebuilt = run(&arguments, true);
    success(&rebuilt, b"43\n");
    assert_eq!(runtime_links(&rebuilt), 1);
    let recovered = run(&arguments, true);
    success(&recovered, b"43\n");
    assert_eq!(runtime_links(&recovered), 0);
    assert!(phase(&recovered, "jit.runtime.exact.hit"));

    let extra_source = fixture.0.join("sdk/src/extra.rinzu");
    std::fs::write(&extra_source, "fn").unwrap();
    let invalid = run(&[], true);
    assert!(!invalid.status.success());
    assert!(phase(&invalid, "frontend"));
    assert!(!phase(&invalid, "jit.frontend.cache.hit"));
    std::fs::remove_file(extra_source).unwrap();

    for entry in std::fs::read_dir(fixture.0.join("cache/jit-frontend-v1")).unwrap() {
        std::fs::write(entry.unwrap().path().join("checked.msgpack"), b"broken IR").unwrap();
    }
    let refreshed = run(&[], true);
    success(&refreshed, b"43\n");
    assert!(phase(&refreshed, "frontend"));
    assert!(phase(&refreshed, "jit.frontend.cache.store"));
    assert!(!phase(&refreshed, "jit.frontend.cache.hit"));
    fixture.corrupt_artifacts();
    success(&run(&[], true), b"43\n");
    let normal = run(&[], false);
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

    let pack = std::fs::read_dir(fixture.0.join("cache/jit-functions-v1"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("functions.bin");
    std::fs::write(&pack, b"partial function stencils").unwrap();
    let program = std::fs::read_to_string(&source).unwrap();
    std::fs::write(&source, program.replace("return 1;", "return 2;")).unwrap();
    let rebuilt_pack = run(&[], true);
    success(&rebuilt_pack, b"44\n");
    assert_eq!(
        machine_code_counts(&rebuilt_pack, "jit.managed.lower"),
        (cold_functions, 0)
    );
    assert_eq!(
        machine_code_counts(&rebuilt_pack, "jit.managed.analysis"),
        (cold_stubs, 0)
    );
    assert!(phase(&rebuilt_pack, "jit.managed.function-cache.store"));
    std::fs::write(&source, program.replace("return 1;", "return 3;")).unwrap();
    let reused_pack = run(&[], true);
    success(&reused_pack, b"45\n");
    assert!(machine_code_counts(&reused_pack, "jit.managed.lower").1 > 0);
    assert_eq!(
        machine_code_counts(&reused_pack, "jit.managed.analysis"),
        (0, cold_stubs)
    );

    std::fs::write(&source, program.replace("return 1;", "return 4;")).unwrap();
    std::fs::write(
        fixture.0.join("sdk/headers/api.h"),
        "#include <stdint.h>\nint32_t answer(void);\n",
    )
    .unwrap();
    let invalid_abi = run(&[], true);
    assert!(!invalid_abi.status.success());
    let error = String::from_utf8_lossy(&invalid_abi.stderr);
    assert!(error.contains("native scalar ABI mismatch"), "{error}");
    assert_eq!(
        machine_code_counts(&invalid_abi, "jit.managed.analysis"),
        (0, cold_stubs)
    );
    assert!(!phase(&invalid_abi, "jit.managed.publish"));
    assert!(!phase(&invalid_abi, "jit.managed.function-cache.store"));
    for name in [
        "jit.frontend.cache.store",
        "jit.managed.cache.store",
        "jit.runtime.cache.store",
        "jit.runtime.exact.store",
        "native.package.early.store",
    ] {
        assert!(
            !phase(&invalid_abi, name),
            "failed ABI validation published {name}"
        );
    }
}

struct Running {
    child: std::process::Child,
    reader: Option<std::thread::JoinHandle<Vec<u8>>>,
    log: PathBuf,
}

impl Running {
    fn ready(fixture: &Fixture, attempt: usize) -> Self {
        use std::io::BufRead;
        let log = fixture.0.join(format!("long-run-{attempt}.stderr"));
        let mut child = fixture
            .command(&[], true, 2)
            .env_remove("DYLD_FALLBACK_LIBRARY_PATH")
            .stdout(std::process::Stdio::piped())
            .stderr(std::fs::File::create(&log).unwrap())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        let reader = std::thread::spawn(move || {
            let mut output = Vec::new();
            let mut input = std::io::BufReader::new(stdout);
            loop {
                let mut line = Vec::new();
                if input.read_until(b'\n', &mut line).unwrap() == 0 {
                    break;
                }
                if line == b"READY\n" {
                    let _ = send.send(());
                }
                output.extend(line);
            }
            output
        });
        let mut running = Self {
            child,
            reader: Some(reader),
            log,
        };
        assert!(
            receive
                .recv_timeout(std::time::Duration::from_secs(60))
                .is_ok(),
            "Main never became ready: {}",
            std::fs::read_to_string(&running.log).unwrap()
        );
        assert!(
            running.child.try_wait().unwrap().is_none(),
            "Main must still be waiting"
        );
        running
    }

    fn records(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(&self.log)
            .unwrap()
            .lines()
            .filter_map(|line| line.strip_prefix("RINZU_COMPILE_TIMING="))
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn terminate(mut self) {
        use std::os::unix::process::ExitStatusExt;
        // SAFETY: this is our live child process and SIGTERM has no pointer arguments.
        let signalled = unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM) };
        assert_eq!(signalled, 0);
        assert_eq!(self.child.wait().unwrap().signal(), Some(libc::SIGTERM));
        assert_eq!(self.reader.take().unwrap().join().unwrap(), b"42\nREADY\n");
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

#[test]
fn long_running_main_commits_before_waiting_and_hits_after_sigterm_restart() {
    let fixture = Fixture::new();
    std::fs::write(fixture.0.join("app/src/main.rinzu"), "import model;import rinzu.stdio;import rinzu.timer;async fn main(){try{rinzu.stdio.writeLine(model.answer());rinzu.stdio.writeLine(\"READY\");await rinzu.timer.sleep(60000);}catch errors{panic(\"unexpected failure\");}}").unwrap();
    let cold = Running::ready(&fixture, 0);
    let records = cold.records();
    for phase in [
        "jit.frontend.cache.store",
        "jit.managed.cache.store",
        "jit.managed.function-cache.store",
        "jit.runtime.cache.store",
        "jit.runtime.exact.store",
    ] {
        assert!(
            records
                .iter()
                .any(|event| event["phase"] == phase && event["counters"]["stored"] == 1),
            "{phase} was not committed before Main waited: {records:#?}"
        );
    }
    for namespace in [
        "jit-frontend-v1",
        "jit-code-v1",
        "jit-functions-v1",
        "jit-runtime-v1",
        "jit-run-runtime-v2",
    ] {
        assert_eq!(
            std::fs::read_dir(fixture.0.join("cache").join(namespace))
                .unwrap()
                .filter(|entry| entry
                    .as_ref()
                    .unwrap()
                    .path()
                    .join("receipt.json")
                    .is_file())
                .count(),
            1,
            "{namespace}"
        );
    }
    cold.terminate();
    let warm = Running::ready(&fixture, 1);
    let records = warm.records();
    for phase in [
        "jit.frontend.cache.hit",
        "jit.managed.cache.hit",
        "jit.runtime.exact.hit",
    ] {
        assert!(
            records.iter().any(|event| event["phase"] == phase),
            "missing {phase} after SIGTERM: {records:#?}"
        );
    }
    for phase in [
        "frontend",
        "jit.managed.lower",
        "jit.managed.analysis",
        "jit.runtime.generate",
        "jit.runtime.link",
    ] {
        assert!(
            !records.iter().any(|event| event["phase"] == phase),
            "warm restart unnecessarily repeated {phase}"
        );
    }
    let publication = records
        .iter()
        .find(|event| event["phase"] == "jit.managed.publish")
        .unwrap();
    assert_eq!(publication["counters"]["rwx_mappings"], 0);
    assert_eq!(
        publication["counters"]["rx_transitions"],
        publication["counters"]["executable_mappings"]
    );
    warm.terminate();
}

#[test]
fn synchronous_main_commits_frontend_before_waiting_and_hits_after_restart() {
    let fixture = Fixture::new();
    std::fs::write(fixture.0.join("app/src/main.rinzu"), "import model;import rinzu.stdio;fn main(){try{rinzu.stdio.writeLine(model.answer());rinzu.stdio.writeLine(\"READY\");}catch errors{panic(\"unexpected failure\");}while true{}}").unwrap();
    let cold = Running::ready(&fixture, 0);
    assert!(
        cold.records()
            .iter()
            .any(|event| event["phase"] == "jit.frontend.cache.store"
                && event["counters"]["stored"] == 1)
    );
    cold.terminate();
    let warm = Running::ready(&fixture, 1);
    assert!(
        warm.records()
            .iter()
            .any(|event| event["phase"] == "jit.frontend.cache.hit")
    );
    warm.terminate();
}

#[test]
fn corrupt_function_frontend_packs_recompute_and_publish_a_valid_replacement() {
    let fixture = Fixture::new();
    let source = fixture.0.join("app/src/main.rinzu");
    let original = std::fs::read_to_string(&source).unwrap();
    let seed = fixture.run(&[], true);
    success(&seed, b"42\n");
    assert!(phase(&seed, "frontend.function-cache.store"));
    let directory = std::fs::read_dir(fixture.0.join("cache/jit-function-frontend-v1"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let pack = directory.join("functions.msgpack");
    for (attempt, reseal) in [false, true].into_iter().enumerate() {
        let broken = b"invalid MessagePack function cache";
        std::fs::write(&pack, broken).unwrap();
        if reseal {
            let receipt = directory.join("receipt.json");
            let mut contents: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&receipt).unwrap()).unwrap();
            contents["files"]["functions.msgpack"] =
                rinzu_bindgen::source_package::sha256(broken).into();
            std::fs::write(receipt, serde_json::to_vec(&contents).unwrap()).unwrap();
        }
        std::fs::write(
            &source,
            original.replace(
                "model.answer()",
                &format!("model.answer() + {}", attempt + 1),
            ),
        )
        .unwrap();
        let recovered = fixture.run(&[], true);
        success(&recovered, format!("{}\n", 43 + attempt).as_bytes());
        assert!(phase(&recovered, "frontend"));
        assert!(!phase(&recovered, "jit.frontend.cache.hit"));
        assert_eq!(phase(&recovered, "frontend.function-cache.decode"), reseal);
        assert!(phase(&recovered, "frontend.function-cache.store"));
        let _: rinzu_compiler::FunctionFrontendCache =
            rmp_serde::from_slice(&std::fs::read(&pack).unwrap()).unwrap();
        success(
            &fixture.run(&[], true),
            format!("{}\n", 43 + attempt).as_bytes(),
        );
    }
}

#[test]
fn function_insertion_relinks_dispatch_and_diagnostic_positions() {
    let fixture = Fixture::new();
    let source = fixture.0.join("app/src/main.rinzu");
    let original = r#"
import model;
import rinzu.process;
import rinzu.stdio;

fn leaf(value: Int) -> Int { return model.answer() + value; }
async fn worker(value: Int) -> Int { return await spawn leaf(value); }
fn divide() -> Int {
    return 1 / 0;
}
async fn main() {
    let reference = worker;
    var answer = 0;
    try {
        answer = await reference(7);
        if rinzu.process.setting("RINZU_JIT_TEST_FAIL", "") == "yes" {
            answer = divide();
        }
    }
    catch errors { panic("unexpected test input error"); }
    finally { answer = answer + 1; }
    try { rinzu.stdio.writeLine(answer); }
    catch errors { panic("write failed"); }
}
"#;
    let run = |fail: bool| {
        fixture
            .command(if fail { &["--trace"] } else { &[] }, true, 2)
            .env("RINZU_JIT_TEST_FAIL", if fail { "yes" } else { "no" })
            .output()
            .unwrap()
    };
    std::fs::write(&source, original).unwrap();
    let seed = run(false);
    success(&seed, b"50\n");
    let (ordinary, 0) = machine_code_counts(&seed, "jit.managed.lower") else {
        panic!("expected fresh machine code");
    };
    let (stubs, 0) = machine_code_counts(&seed, "jit.managed.analysis") else {
        panic!("expected fresh ABI stubs");
    };

    let added = original
        .replace("fn leaf(", "fn extra() -> Int { return 2; }\n\nfn leaf(")
        .replace("reference(7)", "reference(extra())");
    let division = "1 / 0";
    let start = added.find(division).unwrap();
    let range = format!(":{start}:{}", start + division.len());
    std::fs::write(&source, added).unwrap();
    let failure = run(true);
    assert!(!failure.status.success());
    let error = String::from_utf8_lossy(&failure.stderr);
    assert!(error.contains("RINZU_FAILURE=arithmetic"), "{error}");
    assert!(
        error
            .lines()
            .filter_map(|line| line.strip_prefix("RINZU_DIAGNOSTICS="))
            .any(|diagnostic| diagnostic.ends_with(&range)),
        "{error}"
    );
    assert_eq!(
        machine_code_counts(&failure, "jit.managed.lower"),
        (2, ordinary - 1)
    );
    // Main failed after a valid native startup and executable publication.
    // Its immutable build products must be available on the next start.
    assert!(phase(&failure, "jit.managed.function-cache.store"));
    assert!(phase(&failure, "jit.managed.cache.store"));
    assert!(phase(&failure, "jit.frontend.cache.store"));

    let inserted = run(false);
    success(&inserted, b"45\n");
    assert!(hit(&inserted, "compile async runtime source"));
    assert!(phase(&inserted, "jit.frontend.cache.hit"));
    assert!(phase(&inserted, "jit.managed.cache.hit"));
    assert!(!phase(&inserted, "jit.managed.lower"));
    assert!(!phase(&inserted, "jit.managed.analysis"));

    std::fs::write(&source, original.replace("reference(7)", "reference(8)")).unwrap();
    let deleted = run(false);
    success(&deleted, b"51\n");
    assert_eq!(
        machine_code_counts(&deleted, "jit.managed.lower"),
        (1, ordinary - 1)
    );
    assert_eq!(
        machine_code_counts(&deleted, "jit.managed.analysis"),
        (0, stubs)
    );
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

#[test]
fn early_sdk_build_waits_for_checked_application_before_publishing() {
    let fixture = Fixture::new();
    let source = fixture.0.join("app/src/main.rinzu");
    let valid = std::fs::read(&source).unwrap();
    std::fs::write(
        fixture.0.join("sdk/headers/api.h"),
        "#include <stdint.h>\nint32_t answer(void);\n",
    )
    .unwrap();
    std::fs::write(&source, "import model; fn main() -> Int { return true; }").unwrap();
    let invalid_application = fixture.run(&[], true);
    assert!(!invalid_application.status.success());
    let error = String::from_utf8_lossy(&invalid_application.stderr);
    assert!(error.contains("input failed to check"), "{error}");
    assert!(!error.contains("native scalar ABI mismatch"), "{error}");
    let published = fixture
        .0
        .join("app/.rinzu/generated/source-packages-v2/manifest.json");
    assert!(!published.exists());

    std::fs::write(&source, valid).unwrap();
    let invalid_sdk = fixture.run(&[], true);
    assert!(!invalid_sdk.status.success());
    let error = String::from_utf8_lossy(&invalid_sdk.stderr);
    assert!(error.contains("native scalar ABI mismatch"), "{error}");
    assert!(!published.exists());
}
