#[path = "fixtures/checked_entry.rs"]
mod checked_entry;
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[allow(dead_code)]
#[path = "support/scratch.rs"]
mod scratch;

use std::process::Command;

fn assert_static_rejection(root: &std::path::Path, code: &str) {
    for command in ["check", "run", "build", "verify-aot"] {
        let output = Command::new(frozen_cli::path())
            .arg(command)
            .arg(root)
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "{command} accepted an invalid boundary"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(code),
            "{command}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn synchronous_entry_drains_async_work_and_can_set_the_process_status() {
    let root = std::env::temp_dir().join(format!("rinzu-sync-main-task-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    for (index, text, expected, status) in [
        (
            0,
            include_str!("../../codegen/tests/fixtures/future-sync-lifetime.rinzu"),
            b"sync main complete\nretained after sync main\n".as_slice(),
            0,
        ),
        (
            1,
            r#"import rinzu.stdio;async fn work(){try{rinzu.stdio.writeLine("after main");}catch errors{}}fn main(){work();try{rinzu.stdio.writeLine("main");}catch errors{}}"#,
            b"main\nafter main\n".as_slice(),
            0,
        ),
        (
            2,
            "fn main(){let accepted=setProcessExitStatus(7);}",
            b"".as_slice(),
            7,
        ),
    ] {
        let file = root.join(format!("case{index}.rinzu"));
        std::fs::write(&file, text).unwrap();
        let output = Command::new(frozen_cli::path())
            .arg("run")
            .arg(&file)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(status),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, expected, "case {index}");
        if index == 2 {
            let executable = root.join("status");
            let built = Command::new(frozen_cli::path())
                .arg("build")
                .arg(&file)
                .arg("--output")
                .arg(&executable)
                .output()
                .unwrap();
            assert!(
                built.status.success(),
                "{}",
                String::from_utf8_lossy(&built.stderr)
            );
            let aot = Command::new(executable).output().unwrap();
            assert_eq!(
                aot.status.code(),
                Some(status),
                "{}",
                String::from_utf8_lossy(&aot.stderr)
            );
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[path = "fixtures/native_length_sdk/setup.rs"]
mod native_length_fixture;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "support/source_platform.rs"]
#[allow(dead_code)]
mod source_platform;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn source_native_length_sdk_runs_main_workers_and_rejects_a_closed_entry() {
    let root = std::env::temp_dir().join(format!("rinzu-cli-native-length-{}", std::process::id()));
    native_length_fixture::create(&root);
    let binary = frozen_cli::path();
    let run = Command::new(binary)
        .arg("run")
        .arg(root.join("app"))
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(String::from_utf8_lossy(&run.stderr).contains("compiled JIT continuation runtime"));
    assert!(
        root.join("app/.rinzu/generated/source-packages-v2/manifest.json")
            .is_file()
    );
    let build = Command::new(binary)
        .arg("build")
        .arg(root.join("app"))
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let aot = Command::new(root.join("app/build/length_app"))
        .output()
        .unwrap();
    assert!(
        aot.status.success(),
        "{}",
        String::from_utf8_lossy(&aot.stderr)
    );
    let aot_stdout = String::from_utf8_lossy(&aot.stderr);
    assert!(
        aot_stdout.contains("RINZU_NATIVE_CALLS=46,46,46,46,46,46,864,3744,0,0,0"),
        "{aot_stdout}"
    );
    let verified = Command::new(binary)
        .arg("verify-aot")
        .arg(root.join("app"))
        .output()
        .unwrap();
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    std::fs::write(root.join("app/src/main.rinzu"), "import measure; fn main() { if measure.measure(b\"A\\xff\", \"日本語\\u{0}🙂\") != 13 || measure.calls() != 1 { panic(\"failure\"); } }").unwrap();
    let compiled = Command::new(binary)
        .arg("run")
        .arg(root.join("app"))
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    assert!(String::from_utf8_lossy(&compiled.stderr).contains("baseline JIT"));
    std::fs::write(
        root.join("app/src/main.rinzu"),
        "import measure; async fn main() { let late = measure.late(); }",
    )
    .unwrap();
    let closed = Command::new(binary)
        .arg("run")
        .arg(root.join("app"))
        .output()
        .unwrap();
    assert_eq!(
        closed.status.code(),
        Some(86),
        "{}",
        String::from_utf8_lossy(&closed.stderr)
    );
    assert!(!String::from_utf8_lossy(&closed.stderr).contains("ran length_app"));
    let built = Command::new(binary)
        .arg("build")
        .arg(root.join("app"))
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let aot_closed = Command::new(root.join("app/build/length_app"))
        .output()
        .unwrap();
    assert_eq!(aot_closed.status.code(), Some(86));
    assert!(!String::from_utf8_lossy(&aot_closed.stderr).contains("RINZU_RESULT="));
    for kind in 1..=4 {
        std::fs::write(
            root.join("app/src/main.rinzu"),
            format!("import measure; async fn main() {{ let fatal = measure.fatal({kind}); }}"),
        )
        .unwrap();
        let fatal = Command::new(binary)
            .arg("run")
            .arg(root.join("app"))
            .output()
            .unwrap();
        // Pointer/foreign-thread violations remain fatal. Invalid String input
        // uses the required explicit callback fallback on both backends.
        let expected_code = if kind < 3 { 86 } else { 0 };
        assert_eq!(
            fatal.status.code(),
            Some(expected_code),
            "kind={kind}: {}",
            String::from_utf8_lossy(&fatal.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&fatal.stderr).contains("ran length_app"),
            kind >= 3
        );
        let built = Command::new(binary)
            .arg("build")
            .arg(root.join("app"))
            .output()
            .unwrap();
        assert!(
            built.status.success(),
            "kind={kind}: {}",
            String::from_utf8_lossy(&built.stderr)
        );
        let aot_fatal = Command::new(root.join("app/build/length_app"))
            .output()
            .unwrap();
        assert_eq!(aot_fatal.status.code(), Some(expected_code));
        assert_eq!(
            String::from_utf8_lossy(&aot_fatal.stderr).contains("RINZU_RESULT="),
            kind >= 3
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn version_and_doctor_are_available() {
    let binary = frozen_cli::path();
    let version = Command::new(binary).arg("--version").output().unwrap();
    assert!(version.status.success());
    assert!(
        String::from_utf8(version.stdout)
            .unwrap()
            .starts_with("rinzu ")
    );

    let doctor = Command::new(binary).arg("doctor").output().unwrap();
    assert!(doctor.status.success());
    assert!(
        String::from_utf8(doctor.stdout)
            .unwrap()
            .contains("v1 runtime:")
    );
}

#[test]
fn application_author_sample_hides_runtime_topology() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/programs/source-callback/src/main.rinzu"),
    )
    .unwrap();
    let source_lower = source.to_ascii_lowercase();
    for forbidden in ["executor", "thread", "domain", "worker", "scheduler"] {
        assert!(
            !source_lower.contains(forbidden),
            "source exposes {forbidden}"
        );
    }
    assert!(source.contains("async fn main"));
    assert!(source.contains("await spawn"));
    assert!(source.contains("sample.platform.run(app, onEvent)"));
}

#[test]
fn check_compiles_a_package_to_verified_ir() {
    let package =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../compiler/tests/fixtures/package");
    let output = Command::new(frozen_cli::path())
        .arg("check")
        .arg(package)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("checked package_fixture")
    );
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn explicit_native_cli_inputs_are_checked_linked_and_saved() {
    let root = std::env::temp_dir().join(format!("rinzu-cli-native-inputs-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("native")).unwrap();
    std::fs::write(
        root.join("rinzu.toml"),
        "[package]\nname = \"cli_fixture\"\nlanguage = \"0.2\"\n",
    )
    .unwrap();
    std::fs::write(root.join("src/main.rinzu"), "fn main() { let value = 7; }").unwrap();
    std::fs::write(
        root.join("native/shim.c"),
        "int explicit_native_input(void) { return 1; }\n",
    )
    .unwrap();
    assert!(
        Command::new("clang")
            .args(["-target", "arm64-apple-macos11"])
            .arg("-c")
            .arg(root.join("native/shim.c"))
            .arg("-o")
            .arg(root.join("native/shim with space.o"))
            .status()
            .unwrap()
            .success()
    );
    for command in ["check", "run", "test", "build"] {
        let output = Command::new(frozen_cli::path())
            .arg(command)
            .arg(&root)
            .args([
                "-L",
                "native",
                "-lSystem",
                "--native-object",
                "native/shim with space.o",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{command}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let metadata: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.join("build/cli_fixture.metadata.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        metadata["nativeInputs"]["objects"][0],
        root.join("native/shim with space.o")
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
    );
    assert_eq!(metadata["nativeInputs"]["libraries"][0], "System");
    for args in [
        vec!["-L", "rinzu.toml"],
        vec!["-Wl,-rpath,/tmp"],
        vec!["--native-object", "src/main.rinzu"],
        vec!["-l", "@response"],
        vec!["@response"],
    ] {
        assert!(
            !Command::new(frozen_cli::path())
                .arg("check")
                .arg(&root)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn check_validates_supplied_native_header_and_never_falls_back() {
    let root =
        std::env::temp_dir().join(format!("rinzu-cli-native-contract-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("native")).unwrap();
    std::fs::write(root.join("rinzu.toml"), "[package]\nname = \"contract_fixture\"\nlanguage = \"0.2\"\nkind = \"sdk\"\nentry = \"src/fixture.rinzu\"\n[native]\nheaders = [\"native/api.h\"]\nsources = [\"native/api.c\"]\n").unwrap();
    std::fs::write(
        root.join("src/fixture.rinzu"),
        "native { fn value(input: CSize) -> CInt symbol \"value\"; } export value;",
    )
    .unwrap();
    std::fs::write(
        root.join("native/api.c"),
        "#include <stddef.h>\nint value(size_t input) { return (int)input; }\n",
    )
    .unwrap();
    for (header, succeeds) in [
        ("#include <stddef.h>\nint value(size_t input);", true),
        ("int value(unsigned int input);", false),
        ("#error bad supplied header", false),
    ] {
        std::fs::write(root.join("native/api.h"), header).unwrap();
        let output = Command::new(frozen_cli::path())
            .arg("check")
            .arg(&root)
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            succeeds,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_async_services_are_driven_by_source_declarations() {
    let package = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/programs/async-services");
    let output = Command::new(frozen_cli::path())
        .arg("verify-async")
        .arg(package)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stderr).unwrap();
    assert!(stdout.contains("last-awaited=40"));
    assert!(stdout.contains("resources=baseline"));
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn source_callback_sdk_package_runs_from_an_ordinary_main() {
    let work = scratch::Scratch::new("rinzu-source-callback-program");
    let package =
        source_platform::stage_fixture(work.path(), "tests/fixtures/programs/source-callback");
    let output = source_platform::bounded_tool(
        Command::new(frozen_cli::path())
            .arg("run")
            .arg(package)
            .arg("--trace"),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = std::str::from_utf8(&output.stderr).unwrap().to_owned();
    assert!(stdout.contains("ran source_callback with compiled JIT continuation runtime"));
    assert!(stdout.contains("\"producer\": \"rinzu-jit\""));
    assert!(
        stdout.contains("\"operation\": \"file-read-size\"")
            && stdout.contains("\"operation\": \"spawn\"")
    );
    assert!(stdout.contains("\"resources\": \"baseline\""));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("implemented in both"));
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn sdl2_sdk_package_uses_the_same_native_package_abi() {
    let work = scratch::Scratch::new("rinzu-sdl2-program");
    source_platform::stage_fixture(work.path(), "sdk/sdl2");
    let package = source_platform::stage_fixture(work.path(), "tests/fixtures/programs/sdl2");
    let output = Command::new(frozen_cli::path())
        .arg("run")
        .arg(package)
        .arg("--trace")
        .env("SDL_VIDEODRIVER", "dummy")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = std::str::from_utf8(&output.stderr).unwrap().to_owned();
    assert!(stdout.contains("ran v0_2_sdl2 with compiled JIT continuation runtime"));
    assert!(stdout.contains("\"operation\": \"timer\""));
    assert!(stdout.contains("\"operation\": \"file-read-size\""));
    assert!(stdout.contains("\"operation\": \"spawn\""));
    assert!(stdout.contains("\"resources\": \"baseline\""));
}

#[test]
fn compiler_and_codegen_do_not_name_framework_packages() {
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut directories = vec![
        workspace.join("crates/compiler/src"),
        workspace.join("crates/codegen/src"),
    ];
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                directories.push(path);
                continue;
            }
            if path.extension().is_none_or(|extension| extension != "rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap().to_ascii_lowercase();
            assert!(
                !source.contains("appkit"),
                "{} names AppKit",
                path.display()
            );
            // v0.2 specifies one closed config tool; framework APIs remain SDK-owned.
            let without_config_tool = source.replace("sdl2-config", "");
            assert!(
                !without_config_tool.contains("sdl2"),
                "{} names SDL2",
                path.display()
            );
        }
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn native_sdk_packages_have_equivalent_jit_and_aot_execution() {
    let work = scratch::Scratch::new("rinzu-native-sdk-equivalence");
    source_platform::stage_fixture(work.path(), "sdk/sdl2");
    for sample in [
        "tests/fixtures/programs/source-callback",
        "tests/fixtures/programs/sdl2",
    ] {
        let package = source_platform::stage_fixture(work.path(), sample);
        let output = source_platform::bounded_tool(
            Command::new(frozen_cli::path())
                .arg("verify-aot")
                .arg(package)
                .env("SDL_VIDEODRIVER", "dummy"),
        );
        assert!(
            output.status.success(),
            "{sample}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains("JIT/AOT equivalent: result=0, outcome=success"));
        assert!(stdout.contains("resume-domain=main"));
        assert!(stdout.contains("resources=baseline"));
        assert!(stdout.contains("shutdown=complete"));
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn source_async_aot_executes_typed_arguments_control_flow_and_actual_results() {
    let root = std::env::temp_dir().join(format!("rinzu-typed-async-aot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("rinzu.toml"),
        "[package]\nname = \"typed_async\"\nlanguage = \"0.2\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/main.rinzu"),
        r#"
fn calculate(narrow: CInt8, wide: CUInt64, value: CFloat, enabled: CBool) -> CFloat {
    if narrow != -128 { panic("narrow_failure"); }
    if wide <= 9223372036854775808 { panic("unsigned_failure"); }
    if !enabled { panic("bool_failure"); }
    return (value + 1.0) - value;
}

fn many(a: Int, b: Int, c: Int, d: Int, e: Int, f: Int, g: Int, h: Int, i: Int) -> Int {
    return a + b + c + d + e + f + g + h + i;
}
fn echo(value: CUInt64) -> CUInt64 { return value; }
fn empty() {}
async fn ready() -> CUInt64 { return 4611686018427387905; }
async fn main() {
    let rounded = await spawn calculate(-128, 18446744073709551615, 16777216.0, true);
    if rounded != 0.0 { panic("rounding_failure"); }
    let high = await ready();
    let same = await spawn echo(high);
    if same != high { panic("identity_failure"); }
    var index = 0;
    while index < 3 {
        let sum = await spawn many(index, 2, 3, 4, 5, 6, 7, 8, 9);
        if sum != index + 44 { panic("argument_failure"); }
        index = index + 1;
    }
    await spawn empty();
}
"#,
    )
    .unwrap();
    let executable = root.join("build/program");
    let build = Command::new(frozen_cli::path())
        .arg("build")
        .arg(&root)
        .arg("--output")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    assert!(!root.join("build/program.resources").exists());
    let run = Command::new(&executable)
        .env("RINZU_TRANSPORT_EVIDENCE", "1")
        .output()
        .unwrap();
    let stdout = std::str::from_utf8(&run.stderr).unwrap().to_owned();
    assert!(
        run.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(stdout.contains("RINZU_FAILURE=none"), "{stdout}");
    assert!(stdout.contains("RINZU_AWAITED_VALUES=8:0000000000000000,7:4000000000000001,7:4000000000000001,3:000000000000002c,3:000000000000002d,3:000000000000002e"), "{stdout}");
    assert!(stdout.contains("RINZU_TRANSPORT=0,1,0,1,0,72"), "{stdout}");
    let verify = Command::new(frozen_cli::path())
        .arg("verify-aot")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        verify.status.success(),
        "{}",
        String::from_utf8_lossy(&verify.stderr)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn empty_standalone_runs_on_v1_target() {
    let output = Command::new(frozen_cli::path())
        .arg("run-empty")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("empty standalone completed")
    );
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn run_executes_package_and_prints_normalized_trace() {
    let package =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../compiler/tests/fixtures/package");
    let output = Command::new(frozen_cli::path())
        .arg("run")
        .arg(package)
        .arg("--trace")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = std::str::from_utf8(&output.stderr).unwrap().to_owned();
    assert!(stdout.contains("ran package_fixture with baseline JIT"));
    assert!(stdout.contains("\"schema\": \"rinzu.event-trace\""));
    assert!(stdout.contains("\"domain\": \"main\""));
    assert!(!stdout.contains("observation"));
    assert!(stdout.contains("exit status: 0"));
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn build_links_typed_ir_object_into_a_standalone_executable() {
    let package =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../compiler/tests/fixtures/package");
    let directory = std::env::temp_dir().join(format!("rinzu-aot-test-{}", std::process::id()));
    let executable = directory.join("package_fixture");
    let output = Command::new(frozen_cli::path())
        .arg("build")
        .arg(package)
        .arg("--output")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(executable.is_file());
    assert!(executable.with_file_name("package_fixture.o").is_file());
    let metadata = executable.with_file_name("package_fixture.metadata.json");
    let metadata = std::fs::read_to_string(metadata).unwrap();
    assert!(metadata.contains("rinzu.aot-runtime-metadata"));
    assert!(metadata.contains("rinzu_managed_1_main"));

    let execution = Command::new(&executable).output().unwrap();
    assert!(execution.status.success());
    let stdout = std::str::from_utf8(&execution.stderr).unwrap().to_owned();
    assert!(stdout.contains("RINZU_RESULT=0"));
    assert!(stdout.contains("RINZU_TRACE="));
    std::fs::remove_dir_all(directory).unwrap();
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn jit_and_aot_traces_match_for_success_error_cancel_overload_and_shutdown() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let cases = [
        (
            manifest.join("../compiler/tests/fixtures/package"),
            &[][..],
            "outcome=success",
        ),
        (
            manifest.join("tests/fixtures/aot_failure"),
            &[][..],
            "outcome=failure",
        ),
        (
            manifest.join("tests/fixtures/aot_cancel"),
            &["--cancel-after-polls", "6"][..],
            "outcome=cancelled",
        ),
        (
            manifest.join("tests/fixtures/aot_overload"),
            &["--max-allocations", "1"][..],
            "outcome=overloaded",
        ),
        (
            manifest.join("tests/fixtures/aot_cancel"),
            &["--shutdown-after-polls", "6"][..],
            "outcome=shutdown",
        ),
    ];
    for (package, controls, expected) in cases {
        let mut command = Command::new(frozen_cli::path());
        command.arg("verify-aot").arg(package);
        command.args(controls);
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains(expected), "{stdout}");
        assert!(stdout.contains("resume-domain=main"));
        assert!(stdout.contains("resources=baseline"));
        assert!(stdout.contains("shutdown=complete"));
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn dev_once_uses_the_incremental_watch_path_and_runs_the_package() {
    let package =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../compiler/tests/fixtures/package");
    let output = Command::new(frozen_cli::path())
        .arg("dev")
        .arg(package)
        .arg("--once")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = std::str::from_utf8(&output.stderr).unwrap().to_owned();
    assert!(stdout.contains("dev changed=[main,math] invalidated=[main,math]"));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("ran package_fixture with baseline JIT")
    );
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn release_sample_saves_a_reproducible_aot_artifact_and_runtime_evidence() {
    let package = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/programs/async-spawn");
    let directory = std::env::temp_dir().join(format!("rinzu-release-test-{}", std::process::id()));
    let output = Command::new(frozen_cli::path())
        .arg("release")
        .arg(&package)
        .arg("--output")
        .arg(&directory)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let executable = directory.join("v0_1_async_spawn");
    let execution = Command::new(&executable).output().unwrap();
    assert!(execution.status.success());
    assert!(
        std::str::from_utf8(&execution.stderr)
            .unwrap()
            .to_owned()
            .contains("RINZU_TRACE=")
    );
    let manifest = std::fs::read_to_string(directory.join("release-manifest.json")).unwrap();
    assert!(manifest.contains("\"reproducible\": true"));
    assert!(manifest.contains("\"installerIncluded\": false"));
    let evidence = std::fs::read_to_string(directory.join("runtime-evidence.json")).unwrap();
    assert!(evidence.contains("\"asyncFileBytes\": 20"));
    assert!(evidence.contains("\"cpuSpawnScore\": 40"));
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn build_generates_source_sdk_facade_without_handwritten_metadata() {
    let root = std::env::temp_dir().join(format!("rinzu-cli-source-build-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("include")).unwrap();
    std::fs::write(root.join("rinzu.toml"), "[package]\nname = \"api\"\nlanguage = \"0.2\"\nkind = \"sdk\"\nentry = \"src/api.rinzu\"\n[native]\nsources = [\"api.c\"]\nheaders = [\"include/api.h\"]\ninclude-dirs = [\"include\"]\nframeworks = [\"Foundation\"]\n").unwrap();
    std::fs::write(
        root.join("src/api.rinzu"),
        "native { fn answer() -> CInt symbol \"answer\"; } export answer;",
    )
    .unwrap();
    std::fs::write(
        root.join("include/api.h"),
        "#include <types.h>\nanswer_t answer(void);\n",
    )
    .unwrap();
    std::fs::write(root.join("include/types.h"), "typedef int answer_t;\n").unwrap();
    std::fs::write(root.join("api.c"), "int answer(void) { return 42; }\n").unwrap();
    for command in ["check", "build"] {
        let relative = Command::new(frozen_cli::path())
            .current_dir(root.parent().unwrap())
            .arg(command)
            .arg(root.file_name().unwrap())
            .output()
            .unwrap();
        assert!(
            relative.status.success(),
            "{command}: {}",
            String::from_utf8_lossy(&relative.stderr)
        );
    }
    let output = Command::new(frozen_cli::path())
        .arg("build")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let directory = root.join("build/api");
    assert!(directory.join("source-package-v2.json").is_file());
    assert!(directory.join("source-package.h").is_file());
    assert!(directory.join("source-table.c").is_file());
    assert!(String::from_utf8_lossy(&output.stdout).contains("facade:"));
    // The generated contract is forced into each C translation unit.
    std::fs::write(root.join("api.c"), "double answer(void) { return 42.0; }\n").unwrap();
    let output = Command::new(frozen_cli::path())
        .arg("build")
        .arg(&root)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("conflicting types"));
    // A declaration-only header supplies no code: the link must report its missing symbol.
    std::fs::write(root.join("rinzu.toml"), "[package]\nname = \"api\"\nlanguage = \"0.2\"\nkind = \"sdk\"\nentry = \"src/api.rinzu\"\n[native]\nheaders = [\"api.h\"]\n").unwrap();
    std::fs::write(root.join("api.h"), "int answer(void);\n").unwrap();
    let output = Command::new(frozen_cli::path())
        .arg("build")
        .arg(&root)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unresolved native symbol `answer`"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_sdk_dependency_changes_invalidate_parent_descriptor() {
    let root =
        std::env::temp_dir().join(format!("rinzu-cli-dependency-build-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for name in ["parent", "child"] {
        let package = root.join(name);
        std::fs::create_dir_all(package.join("src")).unwrap();
        let dependency = if name == "parent" {
            "[dependencies]\nchild = { path = \"../child\" }\n"
        } else {
            ""
        };
        std::fs::write(package.join("rinzu.toml"), format!("[package]\nname = \"{name}\"\nlanguage = \"0.2\"\nkind = \"sdk\"\nentry = \"src/{name}.rinzu\"\n{dependency}")).unwrap();
    }
    std::fs::write(
        root.join("parent/src/parent.rinzu"),
        "import child; export child.answer;",
    )
    .unwrap();
    let mut wires = Vec::new();
    for answer in [42, 43] {
        std::fs::write(
            root.join("child/src/child.rinzu"),
            format!("fn answer() -> Int {{ return {answer}; }} export answer;"),
        )
        .unwrap();
        let output = Command::new(frozen_cli::path())
            .arg("build")
            .arg(root.join("parent"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let wire = std::fs::read(root.join("parent/build/parent/source-package-v2.json")).unwrap();
        let descriptor: serde_json::Value = serde_json::from_slice(&wire).unwrap();
        assert!(descriptor["interface"]["package"]["build_inputs"]["dependency:child"].is_string());
        wires.push(descriptor);
    }
    assert_ne!(wires[0]["semantic_digest"], wires[1]["semantic_digest"]);
    assert_eq!(
        wires[0]["interface"]["public_bindings"],
        wires[1]["interface"]["public_bindings"]
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn exact_public_constants_and_private_bytes_sizes_recheck_cached_consumers() {
    let root = std::env::temp_dir().join(format!(
        "rinzu-cli-constant-consumer-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("app/src")).unwrap();
    std::fs::create_dir_all(root.join("api/src")).unwrap();
    std::fs::write(
        root.join("api/rinzu.toml"),
        "[package]\nname=\"api\"\nlanguage=\"0.2\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("app/rinzu.toml"),
        "[package]\nname=\"app\"\nlanguage=\"0.2\"\n[dependencies]\napi={path=\"../api\"}\n",
    )
    .unwrap();
    std::fs::write(root.join("app/src/main.rinzu"),r#"
import api; import rinzu.core;
fn copy(value: Bytes(api.width)) -> Bytes(api.width) { return api.echo(value); }
fn number() -> CSize { return api.maximum; }
async fn main() {
    let waiting=spawn copy(b"A\xff"); let high=await spawn number(); let received=await waiting;
    try { if rinzu.core.byteLength(received) != CSize(api.width) || high != 18446744073709551615 { panic("bad_constant"); } } catch errors { panic("failed"); }
}
"#).unwrap();
    let sdk = |public: usize, private: usize, scalar: &str, maximum: &str| {
        std::fs::write(root.join("api/src/api.rinzu"),format!("let width: {scalar}={public}; let privateWidth: CSize={private}; let maximum: CSize={maximum}; fn echo(value: Bytes(privateWidth)) -> Bytes(privateWidth) {{ return value; }} export width; export maximum; export echo;")).unwrap();
    };
    let run = |command: &str, success: bool| {
        let output = Command::new(frozen_cli::path())
            .arg(command)
            .arg(root.join("app"))
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            success,
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    };
    let descriptor = || -> serde_json::Value {
        let output = Command::new(frozen_cli::path())
            .arg("build")
            .arg(root.join("api"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(
            &std::fs::read(root.join("api/build/api/source-package-v2.json")).unwrap(),
        )
        .unwrap()
    };
    sdk(4, 4, "CUInt8", "18446744073709551615");
    assert!(String::from_utf8_lossy(&run("verify-aot", true).stdout).contains("outcome=success"));
    let initial = descriptor();
    let width = initial["interface"]["public_bindings"]["api.width"]
        .as_str()
        .unwrap();
    let maximum = initial["interface"]["public_bindings"]["api.maximum"]
        .as_str()
        .unwrap();
    assert_eq!(
        initial["interface"]["constants"][maximum],
        "18446744073709551615"
    );
    assert_eq!(initial["interface"]["constant_types"][maximum], "CUInt64");
    assert_eq!(initial["interface"]["constant_types"][width], "CUInt8");
    assert_eq!(
        initial["interface"]["constants"].as_object().unwrap().len(),
        2
    );
    assert!(
        initial["interface"]["functions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|function| function["source_type"]
                .as_str()
                .is_some_and(|ty| ty.contains("Bytes(4)")))
    );
    // Only the dependency changes: the previous success must not survive checking.
    sdk(4, 8, "CUInt8", "18446744073709551615");
    run("check", false);
    sdk(8, 8, "CUInt16", "18446744073709551615");
    assert!(String::from_utf8_lossy(&run("verify-aot", true).stdout).contains("outcome=success"));
    let changed = descriptor();
    assert_ne!(initial["semantic_digest"], changed["semantic_digest"]);
    assert_eq!(changed["interface"]["constant_types"][width], "CUInt16");
    assert_eq!(changed["interface"]["constants"][width], "8");
    assert!(
        changed["interface"]["functions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|function| function["source_type"]
                .as_str()
                .is_some_and(|ty| ty.contains("Bytes(8)")))
    );
    sdk(8, 8, "CUInt16", "18446744073709551614");
    assert!(String::from_utf8_lossy(&run("verify-aot", true).stdout).contains("outcome=failure"));
    assert_eq!(
        descriptor()["interface"]["constants"][maximum],
        "18446744073709551614"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_sdk_cache_reuses_verified_facade_and_regenerates_corrupt_or_changed_inputs() {
    let root = std::env::temp_dir().join(format!("rinzu-cli-source-cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("rinzu.toml"), "[package]\nname = \"cache\"\nlanguage = \"0.2\"\nkind = \"sdk\"\nentry = \"src/cache.rinzu\"\n[native]\nsources = [\"api.c\"]\n").unwrap();
    std::fs::write(
        root.join("src/cache.rinzu"),
        "native { fn answer() -> CInt symbol \"answer\"; } export answer;",
    )
    .unwrap();
    std::fs::write(root.join("api.c"), "int answer(void) { return 42; }\n").unwrap();
    let build = || {
        let output = Command::new(frozen_cli::path())
            .arg("build")
            .arg(&root)
            .env("RINZU_CACHE_DIR", root.join("user-cache"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(
            &std::fs::read(root.join("build/cache/source-cache-report.json")).unwrap(),
        )
        .unwrap()
    };
    let first = build();
    assert_eq!(first["hit"], false);
    let reused = build();
    assert_eq!(reused["hit"], true);
    assert_eq!(reused["completed"], true);
    assert!(!root.join(".rinzu").exists());
    // Inspect the generation selected by the report. The cache also contains
    // independent facade and native-image-path entries.
    let matching_entries = |digest: &serde_json::Value| {
        std::fs::read_dir(root.join("user-cache/source-v4"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                std::fs::read(path.join("source-package-v2.json"))
                    .ok()
                    .is_some_and(|bytes| {
                        rinzu_bindgen::source_package::sha256(&bytes) == digest.as_str().unwrap()
                    })
            })
            .collect::<Vec<_>>()
    };
    let entries = matching_entries(&first["descriptorDigest"]);
    let completed = entries
        .iter()
        .find(|path| !path.join("facade.dylib").exists())
        .unwrap();
    let receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(completed.join("receipt.json")).unwrap()).unwrap();
    let library = receipt["files"]
        .as_object()
        .unwrap()
        .keys()
        .find(|name| name.starts_with("source-") && name.ends_with(".dylib"))
        .unwrap();
    std::fs::write(completed.join(library), b"corrupt completed image").unwrap();
    assert_ne!(build()["completed"], true);
    assert_eq!(build()["completed"], true);
    std::fs::write(completed.join("source-package-v2.json"), b"{}").unwrap();
    assert_ne!(build()["completed"], true);
    assert_eq!(build()["completed"], true);
    // Force both tiers to regenerate instead of damaging an unused facade
    // while a valid completed generation can satisfy the build.
    std::fs::write(completed.join(library), b"corrupt completed image").unwrap();
    let facade = entries
        .iter()
        .find(|path| path.join("facade.dylib").exists())
        .unwrap();
    std::fs::write(facade.join("facade.dylib"), b"corrupt facade").unwrap();
    assert_eq!(build()["hit"], false);
    assert_eq!(build()["completed"], true);
    std::fs::write(root.join("api.c"), "int answer(void) { return 43; }\n").unwrap();
    let changed = build();
    assert_eq!(changed["hit"], false);
    assert_ne!(changed["inputFingerprint"], first["inputFingerprint"]);
    assert_ne!(changed["descriptorDigest"], first["descriptorDigest"]);
    assert_eq!(build()["completed"], true);
    assert!(entries.iter().all(|entry| entry.is_dir()));
    assert!(!matching_entries(&changed["descriptorDigest"]).is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn jit_and_aot_save_the_same_generated_source_sdk_interface() {
    let root =
        std::env::temp_dir().join(format!("rinzu-cli-source-sharing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for name in ["app", "api"] {
        std::fs::create_dir_all(root.join(name).join("src")).unwrap();
        let kind = if name == "app" { "application" } else { "sdk" };
        let dependency = if name == "app" {
            "[dependencies]\napi = { path = \"../api\" }\n"
        } else {
            ""
        };
        std::fs::write(root.join(name).join("rinzu.toml"), format!("[package]\nname = \"{name}\"\nlanguage = \"0.2\"\nkind = \"{kind}\"\nentry = \"src/{name}.rinzu\"\n{dependency}")).unwrap();
    }
    std::fs::write(
        root.join("api/src/api.rinzu"),
        "native { fn unused() -> CInt symbol \"unused\"; } fn answer() -> Int { return 42; } export answer;",
    )
    .unwrap();
    std::fs::write(root.join("app/src/app.rinzu"), "import api; fn main() { let answer = api.answer(); if answer != 42 { let crash = 1 / (answer - answer); } }").unwrap();
    std::fs::write(root.join("api/api.c"), "int unused(void) { return 7; }\n").unwrap();
    let manifest = root.join("api/rinzu.toml");
    std::fs::write(
        &manifest,
        std::fs::read_to_string(&manifest).unwrap() + "[native]\nsources = [\"api.c\"]\n",
    )
    .unwrap();
    let binary = frozen_cli::path();
    for command in ["run", "build"] {
        let output = Command::new(binary)
            .arg(command)
            .arg(root.join("app"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{command}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let output = Command::new(root.join("app/build/app")).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("RINZU_FAILURE=none"));
    let jit = root.join("app/.rinzu/generated/source-packages-v2");
    let aot = root.join("app/build/app.source-packages");
    let jit_manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(jit.join("manifest.json")).unwrap()).unwrap();
    let aot_manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(aot.join("manifest.json")).unwrap()).unwrap();
    let mut interfaces = Vec::new();
    for (directory, manifest) in [(&jit, &jit_manifest), (&aot, &aot_manifest)] {
        assert_eq!(manifest["schema"], "rinzu.source-package-bundle");
        assert_eq!(manifest["version"], 2);
        let packages = manifest["packages"].as_array().unwrap();
        assert_eq!(packages.len(), 1);
        let package = &packages[0];
        assert_eq!(package["package"], "api");
        for (relative, digest) in package["files"].as_object().unwrap() {
            assert_eq!(
                rinzu_bindgen::source_package::sha256(
                    &std::fs::read(directory.join(relative)).unwrap()
                ),
                digest.as_str().unwrap()
            );
        }
        assert_eq!(
            package["facade_digest"],
            package["files"][package["library"].as_str().unwrap()]
        );
        let wire = std::fs::read(directory.join(package["descriptor"].as_str().unwrap())).unwrap();
        let descriptor = rinzu_bindgen::source_package::validate_source_descriptor(&wire).unwrap();
        let mut interface = descriptor.interface;
        // AOT deployment rewrites the private provider plan and records its
        // own compilation receipt; every published byte is verified above.
        // The checked public/typed interface must still be identical.
        interface.package.build_inputs.clear();
        interfaces.push(interface);
    }
    assert_eq!(interfaces[0], interfaces[1]);
    assert_eq!(
        std::fs::read(jit.join("sdk-14c2529eb4498c5d1ffd6915/source-package.h")).unwrap(),
        std::fs::read(aot.join("sdk-14c2529eb4498c5d1ffd6915/source-package.h")).unwrap()
    );
    let metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("app/build/app.metadata.json")).unwrap())
            .unwrap();
    assert_eq!(metadata["sourcePackages"]["version"], 2);
    assert_eq!(
        metadata["sourcePackages"]["manifest"],
        "app.source-packages/manifest.json"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn typed_scalar_arithmetic_has_equivalent_jit_and_aot_results() {
    let root = std::env::temp_dir().join(format!("rinzu-scalar-aot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("rinzu.toml"),
        "[package]\nname = \"scalar_aot\"\nlanguage = \"0.2\"\n",
    )
    .unwrap();
    let cases = [
        (
            "let high: CUInt64 = 18446744073709551615; let low: CUInt64 = 9223372036854775808; if high <= low { panic(\"unsigned_failure\"); } let rounded: CFloat = (16777216.0 + 1.0) - 16777216.0; if rounded != 0.0 { panic(\"rounding_failure\"); } if (0.0 / 0.0) == (0.0 / 0.0) { panic(\"nan_failure\"); }",
            "outcome=success",
        ),
        (
            "let value: CUInt8 = 255; let overflow = value + 1;",
            "outcome=failure",
        ),
        (
            "let value: CInt8 = -128; let overflow = value % -1;",
            "outcome=failure",
        ),
        (
            "try { let value = CInt8(200); } catch error { let ok: CInt8 = 42; }",
            "outcome=success",
        ),
        ("let value = CInt8(200);", "outcome=failure"),
        (
            "var order = 0; try { try { let bad = CInt8(200); } finally { order = order * 10 + 1; } } catch error { order = order * 10 + 2; } finally { order = order * 10 + 3; } if order != 123 { panic(\"failed\"); }",
            "outcome=success",
        ),
        (
            "var order = 0; try { try { let bad = CInt8(200); } finally { try { order = order * 10 + 1; } finally { order = order * 10 + 2; } if true { order = order * 10 + 3; } } } catch error { order = order * 10 + 4; } if order != 1234 { panic(\"failed\"); }",
            "outcome=success",
        ),
        (
            "var order = 0; try { try { let bad = CInt8(200); } finally { let other = CInt8(201); order = 100; } } catch error { order = order + 1; } finally { order = order * 10 + 2; } if order != 12 { panic(\"failed\"); }",
            "outcome=success",
        ),
        (
            "try { try { let bad = CInt8(200); } finally { panic(\"other\"); } } catch error { let incorrect = 42; }",
            "outcome=failure",
        ),
        (
            "try { try { panic(\"bad\"); } finally { let other = CInt8(201); } } catch error { let incorrect = 42; }",
            "outcome=failure",
        ),
        (
            "var order = 0; try { try { let bad = CInt8(200); } finally { try { let other = CInt8(201); } catch localError { order = 1; } order = order * 10 + 2; } } catch outerError { order = order * 10 + 3; } if order != 123 { panic(\"failed\"); }",
            "outcome=success",
        ),
    ];
    for (body, expected) in cases {
        std::fs::write(
            root.join("src/main.rinzu"),
            checked_entry::source(&format!("fn main() {{ {body} }}")),
        )
        .unwrap();
        let output = Command::new(frozen_cli::path())
            .arg("verify-aot")
            .arg(&root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains(expected), "{stdout}");
    }

    std::fs::write(root.join("src/main.rinzu"), "import rinzu.list;\nfn main() { try { try { let primary = CInt8(200); } finally { try { let second = CInt8(300); } finally { let third = CInt8(400); } } } catch [error: Error, ...errorRest] { if rinzu.list.length(errorRest) != 2 || error.message != \"numeric conversion is out of range or loses required integer precision\" { panic(\"incorrect_aggregation\"); } } }").unwrap();
    let nested = Command::new(frozen_cli::path())
        .arg("verify-aot")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        nested.status.success(),
        "{}",
        String::from_utf8_lossy(&nested.stderr)
    );
    std::fs::write(root.join("src/main.rinzu"), "fn main() { try { try { while true { let tick = 1; } } finally { let bad = CInt8(200); } } catch error { let ignored = 1; } finally {} }").unwrap();
    let output = Command::new(frozen_cli::path())
        .arg("verify-aot")
        .arg(&root)
        .args(["--cancel-after-polls", "6"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("outcome=cancelled"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_eager_workers_preserve_prefix_and_prove_concurrent_transport() {
    let root = std::env::temp_dir().join(format!("rinzu-eager-source-aot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("rinzu.toml"),
        "[package]\nname = \"eager_source\"\nlanguage = \"0.2\"\n",
    )
    .unwrap();
    let functions = r#"
fn shrink(a: CInt8, b: CUInt64, c: CFloat, d: Bool) -> CInt32 {
    if a != -1 || b != 18446744073709551615 || c != -0.0 || !d { panic("wrong"); }
    return 42;
}
"#;
    for body in [
        "let first = spawn shrink(-1, 18446744073709551615, -0.0, true); let second = spawn shrink(-1, 18446744073709551615, -0.0, true); var prefix = 7; prefix = prefix + 1; let two = await second; let one = await first; let alias = await first; if prefix != 8 || one != 42 || two != 42 || alias != 42 { panic(\"wrong\"); }",
        "let first = spawn shrink(-1, 18446744073709551615, -0.0, true); let second = spawn shrink(-1, 18446744073709551615, -0.0, true);",
    ] {
        std::fs::write(
            root.join("src/main.rinzu"),
            checked_entry::source(&format!("{functions}\nasync fn main() {{ {body} }}")),
        )
        .unwrap();
        for _ in 0..4 {
            let verify = Command::new(frozen_cli::path())
                .arg("verify-aot")
                .arg(&root)
                .output()
                .unwrap();
            assert!(
                verify.status.success(),
                "{}",
                String::from_utf8_lossy(&verify.stderr)
            );
        }
        let executable = root.join("program");
        let build = Command::new(frozen_cli::path())
            .arg("build")
            .arg(&root)
            .arg("--output")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(
            build.status.success(),
            "{}",
            String::from_utf8_lossy(&build.stderr)
        );
        let run = Command::new(&executable)
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "{} / {}",
            String::from_utf8_lossy(&run.stderr),
            String::from_utf8_lossy(&run.stderr)
        );
        let stdout = std::str::from_utf8(&run.stderr).unwrap().to_owned();
        assert!(
            stdout
                .lines()
                .any(|line| line == "RINZU_STARTED_CHILDREN=2,0")
        );
        let before = if body.contains("await") { "2,0" } else { "0,0" };
        assert!(
            stdout
                .lines()
                .any(|line| line == format!("RINZU_STARTED_BEFORE_FIRST_AWAIT={before}"))
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rooted_strings_and_error_messages_match_between_jit_and_aot() {
    let root = std::env::temp_dir().join(format!("rinzu-string-aot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("rinzu.toml"),
        "[package]\nname = \"string_aot\"\nlanguage = \"0.2\"\n",
    )
    .unwrap();
    std::fs::write(root.join("src/main.rinzu"), r#"
import rinzu.core;
fn keep(text: String, depth: Int) -> String {
    let temporary = "123456789";
    if depth == 0 { return text; }
    return keep(text, depth - 1);
}
fn message() -> String {
    try { let bad = CInt8(200); } catch [error: Error, ...errorRest] { return error.message; }
    return "";
}
fn cleanupMessage() -> String {
    try { try { let bad = CInt8(200); } finally { let other = CInt8(201); } }
    catch [first: Error, second: Error] { return f"{first.message}|{second.message}"; }
    catch errors { return "unexpected error list"; }
    return "";
}
fn main() {
    let text = message();
    var iteration = 0;
    while iteration < 100 {
        let copy = keep("日本語🙂\u{0}tail", 3);
        if copy != "日本語🙂\u{0}tail" { panic("payload_failure"); }
        if utf8ByteLength(copy) != 18 { panic("length_failure"); }
        iteration = iteration + 1;
    }
    if text != "numeric conversion is out of range or loses required integer precision" { panic("message_failure"); }
    if utf8ByteLength(text) != 70 { panic("message_length_failure"); }
    let aggregate = cleanupMessage();
    if aggregate != "numeric conversion is out of range or loses required integer precision|numeric conversion is out of range or loses required integer precision" { panic("cleanup_failure"); }
    if "é" == "e\u{301}" { panic("normalization_failure"); }
    if "" != "" { panic("empty_failure"); }
    if utf8ByteLength("") != 0 { panic("empty_length_failure"); }
    if "a\u{0}b" == "a" { panic("nul_failure"); }
}
"#).unwrap();
    let output = Command::new(frozen_cli::path())
        .arg("verify-aot")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("outcome=success"));
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn error_message_module_matches_fields_in_all_execution_modes() {
    let root = std::env::temp_dir().join(format!("rinzu-error-message-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("rinzu.toml"),
        "[package]\nname = \"error_message\"\nlanguage = \"0.5\"\n",
    )
    .unwrap();
    let program = r#"
import rinzu.error as err;
import rinzu.list;

error WriteFailed<T> { payload: T }
error CloseFailed {}

fn compare(problem: Error, expected: String) nothrow {
    if err.message(problem) != expected || problem.message != expected {
        panic("error_message_mismatch");
    }
}

fn argument() -> Error {
    throw CloseFailed(message: "close");
}

fn check() nothrow {
    let value = WriteFailed<Int>(message: "日本語🙂\n", payload: 42, cause: err.create(77, "disk"));
    compare(value, value.message);
    let generic = err.create(1, "");
    compare(generic, "");
    switch err.code(generic) {
        case Option.Some(code): { if code != 1 { panic("wrong_code"); } }
        case Option.None: { panic("missing_code"); }
    }
    switch err.code(value) {
        case Option.Some(code): { panic("named_code"); }
        case Option.None: {}
    }
    switch err.context(generic) {
        case Option.Some(context): { if context != "" { panic("wrong_context"); } }
        case Option.None: { panic("missing_context"); }
    }
    switch err.withContext(value, "context") {
        case Option.Some(updated): { panic("named_update"); }
        case Option.None: {}
    }
    switch err.cause(value) {
        case Option.Some(cause): { compare(cause, "disk"); }
        case Option.None: { panic("missing_cause"); }
    }
    if rinzu.list.length(err.cleanup(value)) != 0 { panic("unexpected_cleanup"); }
    try {
        try { throw value; }
        finally { let invalid = CInt8(200); }
    }
    catch [first: Error, second: Error] {
        compare(first, "日本語🙂\n");
        switch err.sourceStartByte(first) {
            case Option.Some(position): { if position <= 0 { panic("wrong_source"); } }
            case Option.None: { panic("missing_source"); }
        }
        compare(second, "numeric conversion is out of range or loses required integer precision");
    }
    catch errors { panic("wrong_error_list"); }
    try {
        let text = err.message(argument());
        panic("argument_returned");
    }
    catch [problem: CloseFailed] { compare(problem, "close"); }
    catch errors { panic("argument_error_changed"); }
}
"#;
    for main in [
        "fn main() { check(); }",
        "async fn main() { check(); let value = err.create(3, \"async\"); compare(value, \"async\"); if err.message(value) != \"async\" { panic(\"async_message\"); } }",
    ] {
        std::fs::write(root.join("src/main.rinzu"), format!("{program}\n{main}\n")).unwrap();
        let output = Command::new(frozen_cli::path())
            .arg("verify-aot")
            .arg(&root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("outcome=success"));
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn source_async_jit_aot_compare_every_scalar_await_and_transport_width() {
    use std::fmt::Write;
    let root =
        std::env::temp_dir().join(format!("rinzu-all-scalar-async-aot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("rinzu.toml"),
        "[package]\nname = \"scalar_async\"\nlanguage = \"0.2\"\n",
    )
    .unwrap();
    let cases = [
        ("CInt8", "-128"),
        ("CInt16", "-32768"),
        ("CInt32", "-2147483648"),
        ("CInt64", "-9223372036854775808"),
        ("CUInt8", "255"),
        ("CUInt16", "65535"),
        ("CUInt32", "4294967295"),
        ("CUInt64", "18446744073709551615"),
        ("CFloat", "-0.0"),
        ("CDouble", "-0.0"),
        ("CBool", "true"),
    ];
    let mut functions = String::new();
    let mut body = String::new();
    for (index, (name, literal)) in cases.into_iter().enumerate() {
        writeln!(
            functions,
            "fn echo{index}(value: {name}) -> {name} {{ return value; }}"
        )
        .unwrap();
        writeln!(body, "let original{index}: {name} = {literal}; let copy{index} = await spawn echo{index}(original{index}); if copy{index} != original{index} {{ panic(\"failure{index}\"); }}").unwrap();
    }
    functions.push_str("async fn nan() -> CFloat { return 0.0 / 0.0; }\n");
    body.push_str("let value = await nan(); let copy = await spawn echo8(value); if copy == copy { panic(\"nan_failure\"); }\n");
    std::fs::write(
        root.join("src/main.rinzu"),
        checked_entry::source(&format!("{functions}\nasync fn main() {{ {body} }}")),
    )
    .unwrap();
    let verify = Command::new(frozen_cli::path())
        .arg("verify-aot")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        verify.status.success(),
        "{}",
        String::from_utf8_lossy(&verify.stderr)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn source_async_ready_failures_are_caught_at_await_and_preserve_cleanup_diagnostics() {
    let root = std::env::temp_dir().join(format!("rinzu-async-errors-aot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("rinzu.toml"),
        "[package]\nname = \"async_errors\"\nlanguage = \"0.2\"\n",
    )
    .unwrap();
    let functions = r#"
import rinzu.list;
import rinzu.core;
fn score(value: Int) -> Int { return value * 2; }
fn fail() -> CInt8 { return CInt8(200); }
async fn ready() -> CInt8 { return 42; }
async fn bad() -> CInt8 { try { return fail(); } finally { let second = CInt8(300); } }
async fn trapped() -> Int { return 1 / 0; }
"#;
    for (body, outcome) in [
        (
            "let value = await ready(); if value != 42 { panic(\"wrong_value\"); } try { let bad = CInt8(200); } catch [error: Error, ...errorRest] { if utf8ByteLength(error.message) != 70 { panic(\"wrong_message\"); } }",
            "success",
        ),
        (
            "var order = 0; try { let future = bad(); order = 1; try { let value = await future; } catch [error: Error, ...errorRest] { order = order * 10 + 2; if rinzu.list.length(errorRest) != 1 || error.message != \"numeric conversion is out of range or loses required integer precision\" { panic(\"wrong_message\"); } } } catch unexpected { order = 99; } finally { order = order * 10 + 3; } if order != 123 { panic(\"wrong_order\"); }",
            "success",
        ),
        (
            "let future = ready(); let first = await future; let second = await future; if first != second { panic(\"changed_result\"); } var catches = 0; let failed = bad(); try { let ignored_one = await failed; } catch one { catches = catches + 1; } try { let ignored_two = await failed; } catch two { catches = catches + 1; } if catches != 2 { panic(\"missing_catch\"); }",
            "success",
        ),
        (
            "let future = spawn score(21); let alias = future; let first = await future; let second = await alias; if first != 42 || second != 42 { panic(\"changed_result\"); }",
            "success",
        ),
        (
            "let ready_value = await ready(); let bad_value = CInt8(200);",
            "failure",
        ),
        (
            "try { let value = await trapped(); } catch should_not_catch { let ignored = 0; } finally { let cleanup = CInt8(201); }",
            "failure",
        ),
        (
            "var order = 0; try { try { let value = await bad(); } finally { try { let second = CInt8(300); } catch local { order = 1; } order = order * 10 + 2; } } catch outer { order = order * 10 + 3; } if order != 123 { panic(\"wrong_order\"); }",
            "success",
        ),
    ] {
        std::fs::write(
            root.join("src/main.rinzu"),
            checked_entry::source(&format!("{functions}\nasync fn main() {{ {body} }}")),
        )
        .unwrap();
        let verify = Command::new(frozen_cli::path())
            .arg("verify-aot")
            .arg(&root)
            .output()
            .unwrap();
        assert!(
            verify.status.success(),
            "{body}: {}",
            String::from_utf8_lossy(&verify.stderr)
        );
        assert!(
            String::from_utf8_lossy(&verify.stdout).contains(&format!("outcome={outcome}")),
            "{}",
            String::from_utf8_lossy(&verify.stdout)
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn source_workers_handle_errors_locally_and_repeat_scalar_reports() {
    let root = std::env::temp_dir().join(format!("rinzu-worker-errors-aot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("rinzu.toml"),
        "[package]\nname = \"worker_errors\"\nlanguage = \"0.2\"\n",
    )
    .unwrap();
    let functions = r#"
import rinzu.list;
fn fail(value: Int) -> CInt8 { return CInt8(value); }
fn failThree() -> CInt8 { try { return fail(200); } finally { try { let second = CInt8(300); } finally { let third = CInt8(400); } } }
fn failTwo() -> CInt8 { try { return fail(200); } finally { try { let second = CInt8(300); } catch [inside: Error, ...insideRest] { let message = inside.message; } let third = CInt8(400); } }
fn bad() -> Int { try { let ignored=failThree(); } catch errors { return rinzu.list.length(errors); } return 0; }
fn local() -> Int { try { let ignored=failTwo(); } catch errors { return rinzu.list.length(errors); } return 0; }
fn caught() -> CInt8 { var value: CInt8 = 0; try { let first = fail(200); } catch one { value = 7; } try { let second = fail(300); } catch two { value = value + 1; } return value; }
fn trapped() -> Int { try { try { return 1 / 0; } finally { let cleanup = CInt8(200); } } catch errors { panic("unexpected"); return 0; } }
"#;
    std::fs::write(
        root.join("src/main.rinzu"),
        format!("{functions} async fn main() {{ let result=await spawn failThree(); }}"),
    )
    .unwrap();
    assert_static_rejection(&root, "RZ0413");
    for (body, outcome) in [
        (
            "let future = spawn bad(); let alias = future; let one=await future; let two=await alias; if one!=3 || two!=3 { panic(\"wrong_count\"); }",
            "success",
        ),
        (
            "let future = spawn local(); let one=await future; let two=await future; if one!=2 || two!=2 { panic(\"wrong_count\"); }",
            "success",
        ),
        (
            "let future = spawn caught(); let one = await future; let two = await future; if one != 8 || two != 8 { panic(\"wrong_value\"); }",
            "success",
        ),
        (
            "try { let ignored = await spawn bad(); } finally { let cleanup = CInt8(500); }",
            "failure",
        ),
        (
            "try { let ignored = await spawn trapped(); } catch should_not_catch { let wrong_catch = CInt8(900); } finally { let cleanup = CInt8(500); }",
            "failure",
        ),
    ] {
        std::fs::write(
            root.join("src/main.rinzu"),
            checked_entry::source(&format!("{functions}\nasync fn main() {{ {body} }}")),
        )
        .unwrap();
        let verify = Command::new(frozen_cli::path())
            .arg("verify-aot")
            .arg(&root)
            .output()
            .unwrap();
        assert!(
            verify.status.success(),
            "{body}: {}",
            String::from_utf8_lossy(&verify.stderr)
        );
        assert!(
            String::from_utf8_lossy(&verify.stdout).contains(&format!("outcome={outcome}")),
            "{}",
            String::from_utf8_lossy(&verify.stdout)
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn source_async_cancellation_keeps_source_and_cleanup_on_main_and_worker() {
    let root = std::env::temp_dir().join(format!("rinzu-cancel-async-aot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("rinzu.toml"),
        "[package]\nname = \"cancel_async\"\nlanguage = \"0.2\"\n",
    )
    .unwrap();
    let worker = r#"
fn work() -> Int {
    try { var value = 0; while true { value = value + 1; } return value; }
    finally {
        try { let local = CInt8(300); } catch [inside: Error, ...insideRest] { let message = inside.message; }
        try { let cleanup = CInt8(400); } finally { let other = CInt8(500); }
    }
}
async fn main() {
    try { let value = await spawn work(); }
    catch wrong { let should_not_catch = CInt8(900); }
    finally { let cleanup = CInt8(600); }
}
"#;
    std::fs::write(root.join("src/main.rinzu"), worker).unwrap();
    for limit in [1, 2, 3, 7] {
        let verify = Command::new(frozen_cli::path())
            .arg("verify-aot")
            .arg(&root)
            .args(["--cancel-worker-after-polls", &limit.to_string()])
            .output()
            .unwrap();
        assert!(
            verify.status.success(),
            "Worker poll {limit}: {}",
            String::from_utf8_lossy(&verify.stderr)
        );
        assert!(String::from_utf8_lossy(&verify.stdout).contains("outcome=cancelled"));
    }
    let main = r#"
fn echo() -> Int { return 7; }
async fn ready() -> Int { return echo(); }
async fn main() {
    try {
        let value = await spawn echo(); let ready_value = await ready();
        var count = value + ready_value; while true { count = count + 1; }
    } catch wrong { let should_not_catch = CInt8(900); }
    finally {
        try { let local = CInt8(300); } catch [inside: Error, ...insideRest] { let message = inside.message; }
        try { let cleanup = CInt8(400); } finally { let other = CInt8(500); }
    }
}
"#;
    std::fs::write(root.join("src/main.rinzu"), main).unwrap();
    for limit in [1, 4, 5, 7, 11] {
        let verify = Command::new(frozen_cli::path())
            .arg("verify-aot")
            .arg(&root)
            .args(["--cancel-after-polls", &limit.to_string()])
            .output()
            .unwrap();
        assert!(
            verify.status.success(),
            "Main poll {limit}: {}",
            String::from_utf8_lossy(&verify.stderr)
        );
        assert!(String::from_utf8_lossy(&verify.stdout).contains("outcome=cancelled"));
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn source_task_discard_requires_no_fail_and_explicit_observation_is_repeatable() {
    let root = std::env::temp_dir().join(format!("rinzu-unawaited-aot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("rinzu.toml"),
        "[package]\nname = \"unawaited\"\nlanguage = \"0.2\"\n",
    )
    .unwrap();
    let functions = r#"
fn echo(value: Int) -> Int { if value != 7 { panic("wrong_snapshot"); } return value; }
fn bad() -> CInt8 { try { return CInt8(200); } finally { let cleanup = CInt8(300); } }
fn trapped() -> Int { return 1 / 0; }
fn caught() -> Int { try { let bad = CInt8(200); } catch [local: Error, ...localRest] { let message = local.message; } return 42; }
fn forever() -> Int { while true { } return 0; }
async fn ready() -> Int { return 7; }
async fn failed() -> CInt8 { return bad(); }
"#;
    for (body, outcome) in [
        ("let dropped = ready();", "success"),
        ("let dropped = failed();", "rejected:RZ0406"),
        (
            "var order = 0; try { let dropped = failed(); order = 1; } catch wrong { order = 99; } finally { order = order * 10 + 2; } if order != 12 { panic(\"wrong_order\"); }",
            "rejected:RZ0406",
        ),
        (
            "var argument = 7; try { let dropped = spawn echo(argument); } finally { argument = 9; }",
            "success",
        ),
        (
            "try { let dropped = spawn bad(); } catch wrong { panic(\"wrong_catch\"); }",
            "rejected:RZ0413",
        ),
        ("let dropped = spawn caught();", "success"),
        (
            "let one = failed(); let two = spawn trapped();",
            "rejected:RZ0406",
        ),
        (
            "let future = failed(); try { let value = await future; } catch [handled: Error, ...handledRest] { let message = handled.message; }",
            "success",
        ),
        (
            "let dropped = failed(); try { panic(\"failure\"); } finally { let cleanup = CInt8(400); }",
            "failure",
        ),
        (
            "let dropped = spawn forever(); try { panic(\"failure\"); } finally { let cleanup = CInt8(400); }",
            "failure",
        ),
    ] {
        std::fs::write(
            root.join("src/main.rinzu"),
            checked_entry::source(&format!("{functions}\nasync fn main() {{ {body} }}")),
        )
        .unwrap();
        if let Some(code) = outcome.strip_prefix("rejected:") {
            assert_static_rejection(&root, code);
            continue;
        }
        let verify = Command::new(frozen_cli::path())
            .arg("verify-aot")
            .arg(&root)
            .output()
            .unwrap();
        assert!(
            verify.status.success(),
            "{body}: {}",
            String::from_utf8_lossy(&verify.stderr)
        );
        assert!(
            String::from_utf8_lossy(&verify.stdout).contains(&format!("outcome={outcome}")),
            "{}",
            String::from_utf8_lossy(&verify.stdout)
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn source_ready_async_callees_transfer_observation_explicitly() {
    let root = std::env::temp_dir().join(format!("rinzu-ready-scopes-aot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("rinzu.toml"),
        "[package]\nname = \"ready_scopes\"\nlanguage = \"0.2\"\n",
    )
    .unwrap();
    let functions = r#"
import rinzu.list;
async fn ready() -> CUInt64 { return 18446744073709551615; }
async fn bad() -> CInt8 { try { return CInt8(200); } finally { let cleanup = CInt8(300); } }
async fn trapped() -> Int { return 1 / 0; }
fn launch() -> Future<CInt8> { return bad(); }
async fn forwarded() -> CInt8 { return await launch(); }
async fn observed() -> CUInt64 {
    let failed = bad(); let alias = failed;
    try { let first = await failed; } catch [one: Error, ...oneRest] { let firstMessage = one.message; }
    try { let second = await alias; } catch [two: Error, ...twoRest] { let secondMessage = two.message; }
    return await ready();
}
async fn recursive(depth: Int) -> CUInt64 {
    if depth == 0 { return await observed(); }
    return await recursive(depth - 1);
}
async fn siblings() -> Int { let first = forwarded(); let second = forwarded(); try all { let one=await first; let two=await second; } catch errors { if rinzu.list.length(errors)!=4 { panic("wrong_count"); } return 9; } return 0; }
async fn primary() -> Int { try { let child = bad(); let value=await child; return 1 / 0; } finally { let cleanup = CInt8(400); } }
async fn trappedChild() -> Int { let child = trapped(); return 8; }
"#;
    for (body, outcome) in [
        (
            "let value = await recursive(8); if value != 18446744073709551615 { panic(\"wrong_value\"); }",
            "success",
        ),
        (
            "var catches = 0; let failed = forwarded(); let alias = failed; try { let first = await failed; } catch [one: Error, ...oneRest] { catches = catches + 1; if rinzu.list.length(oneRest) != 1 || one.message != \"numeric conversion is out of range or loses required integer precision\" { panic(\"wrong_message\"); } } try { let second = await alias; } catch two { catches = catches + 1; } if catches != 2 { panic(\"wrong_count\"); }",
            "success",
        ),
        (
            "var order = 0; try { let future = forwarded(); order = 1; try { let value = await future; } catch child { order = order * 10 + 2; } } catch wrong { order = 99; } finally { order = order * 10 + 3; } if order != 123 { panic(\"wrong_order\"); }",
            "success",
        ),
        (
            "let first = await observed(); let dropped_child = forwarded();",
            "rejected:RZ0406",
        ),
        (
            "let value = await siblings(); if value!=9 { panic(\"wrong\"); }",
            "success",
        ),
        ("let value = await primary();", "failure"),
        (
            "try { let value = await trappedChild(); } catch wrong { let wrong_catch = CInt8(900); } finally { let cleanup = CInt8(500); }",
            "failure",
        ),
        (
            "let ignored = primary(); let outer_failure = CInt8(600);",
            "failure",
        ),
    ] {
        std::fs::write(
            root.join("src/main.rinzu"),
            checked_entry::source(&format!("{functions}\nasync fn main() {{ {body} }}")),
        )
        .unwrap();
        if let Some(code) = outcome.strip_prefix("rejected:") {
            assert_static_rejection(&root, code);
            continue;
        }
        let verify = Command::new(frozen_cli::path())
            .arg("verify-aot")
            .arg(&root)
            .output()
            .unwrap();
        assert!(
            verify.status.success(),
            "{body}: {}",
            String::from_utf8_lossy(&verify.stderr)
        );
        assert!(
            String::from_utf8_lossy(&verify.stdout).contains(&format!("outcome={outcome}")),
            "{}",
            String::from_utf8_lossy(&verify.stdout)
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn source_spawn_admission_precedes_await_and_releases_rejected_children() {
    let root =
        std::env::temp_dir().join(format!("rinzu-spawn-admission-aot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("rinzu.toml"),
        "[package]\nname = \"spawn_admission\"\nlanguage = \"0.2\"\n",
    )
    .unwrap();
    let functions = "fn echo(value: Int) -> Int { return value; }\n";
    for body in [
        "var count = 0; while count < 8 { let child = spawn echo(count); count = count + 1; } let rejected = spawn echo(9);",
        "var count = 0; while count < 8 { let child = spawn echo(count); count = count + 1; } try { let rejected = spawn echo(9); count = 9; let value = await rejected; } catch wrong { count = 99; } finally { if count != 9 { panic(\"wrong_prefix\"); } let cleanup = CInt8(300); }",
        "let first = spawn echo(1); let second = spawn echo(2); let firstValue = await first; let third = spawn echo(3); let thirdValue = await third; let secondValue = await second; if firstValue != 1 || secondValue != 2 || thirdValue != 3 { panic(\"wrong_value\"); }",
    ] {
        std::fs::write(
            root.join("src/main.rinzu"),
            checked_entry::source(&format!("{functions}async fn main() {{ {body} }}")),
        )
        .unwrap();
        let verify = Command::new(frozen_cli::path())
            .arg("verify-aot")
            .arg(&root)
            .output()
            .unwrap();
        assert!(
            verify.status.success(),
            "{body}: {}",
            String::from_utf8_lossy(&verify.stderr)
        );
        let expected = if body.starts_with("let first") {
            "success"
        } else {
            "failure"
        };
        assert!(
            String::from_utf8_lossy(&verify.stdout).contains(&format!("outcome={expected}")),
            "{}",
            String::from_utf8_lossy(&verify.stdout)
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn source_file_service_requires_explicit_observation_and_preserves_cleanup() {
    let root = std::env::temp_dir().join(format!("rinzu-child-io-aot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("rinzu.toml"),
        "[package]\nname=\"child_io\"\nlanguage=\"0.5\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/data.txt"), "実際のfile child\n").unwrap();
    let functions = r#"
async fn read(path: String) -> Int { return await rinzu.io.readSize(path); }
async fn nestedRead(path: String) -> Int { return await read(path); }
"#;
    for (body, outcome) in [
        (
            "let dropped = rinzu.io.readSize(\"data/data.txt\");",
            "rejected:RZ0406",
        ),
        (
            "let dropped = rinzu.io.readSize(\"data/missing.txt\");",
            "rejected:RZ0406",
        ),
        (
            "var catches = 0; let future = rinzu.io.readSize(\"data/missing.txt\"); let alias = future; try { let ignored_one = await future; } catch [one: Error, ...oneRest] { catches = catches + 1; if one.message != \"I/O operation failed\" { panic(\"wrong_message\"); } } try { let ignored_two = await alias; } catch two { catches = catches + 1; } if catches != 2 { panic(\"wrong_count\"); }",
            "success",
        ),
        (
            "try { let size = await rinzu.io.readSize(\"data/missing.txt\"); } finally { let cleanup = CInt8(200); }",
            "failure",
        ),
        (
            "let first = rinzu.io.readSize(\"../outside.txt\"); let second = rinzu.io.readSize(\"data/missing.txt\");",
            "rejected:RZ0406",
        ),
        (
            "try { let size = await rinzu.io.readSize(\"./data/data.txt\"); } catch [error: Error, ...errorRest] { if error.message != \"I/O operation failed\" { panic(\"wrong_message\"); } }",
            "success",
        ),
        (
            "let dropped = rinzu.io.readSize(\"../outside.txt\"); panic(\"failure\");",
            "failure",
        ),
        (
            "let dropped = rinzu.io.readSize(\"data/missing.txt\"); panic(\"failure\");",
            "failure",
        ),
        (
            "let dropped = nestedRead(\"data/data.txt\");",
            "rejected:RZ0406",
        ),
        (
            "var order = 0; let future = nestedRead(\"data/data.txt\"); order = 1; var count = 0; while count < 20 { let moved = \"caller GC\"; count = count + 1; } let size = await future; if size != 20 || order != 1 { panic(\"wrong\"); }",
            "success",
        ),
        (
            "let future = nestedRead(\"data/missing.txt\"); let alias = future; try { let one = await future; } catch [first: Error, ...firstRest] { if first.message != \"I/O operation failed\" { panic(\"wrong\"); } } try { let two = await alias; } catch [second: Error, ...secondRest] { let message = second.message; }",
            "success",
        ),
    ] {
        eprintln!("readSize vector: {body}");
        std::fs::write(
            root.join("src/main.rinzu"),
            checked_entry::source(&format!(
                "import rinzu.io; {functions}\nasync fn main() {{ {body} }}"
            )),
        )
        .unwrap();
        if let Some(code) = outcome.strip_prefix("rejected:") {
            assert_static_rejection(&root, code);
            continue;
        }
        let verify = Command::new(frozen_cli::path())
            .arg("verify-aot")
            .arg(&root)
            .output()
            .unwrap();
        assert!(
            verify.status.success(),
            "{body}: {}",
            String::from_utf8_lossy(&verify.stderr)
        );
        assert!(
            String::from_utf8_lossy(&verify.stdout).contains(&format!("outcome={outcome}")),
            "{}",
            String::from_utf8_lossy(&verify.stdout)
        );
    }
    std::fs::remove_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("src/main.rinzu"), "import rinzu.io; async fn main() { try { let value = await rinzu.io.readSize(\"data/missing.txt\"); } catch [error: Error, ...errorRest] { if error.message != \"I/O operation failed\" { panic(\"wrong_message\"); } } }").unwrap();
    let verify = Command::new(frozen_cli::path())
        .arg("verify-aot")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        verify.status.success(),
        "{}",
        String::from_utf8_lossy(&verify.stderr)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn source_pending_async_callees_preserve_queued_roots_and_explicit_observation() {
    let root =
        std::env::temp_dir().join(format!("rinzu-pending-callee-aot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("rinzu.toml"),
        "[package]\nname = \"pending_callee\"\nlanguage = \"0.2\"\n",
    )
    .unwrap();
    let functions = r#"
fn echo(value: Int) -> Int { return value; }
fn high(value: CUInt64) -> CUInt64 { return value; }
fn forever() -> Int { while true { } return 0; }
async fn delayed(value: Int) -> Int {
    let first = await spawn echo(value);
    let second = await spawn echo(first + 1);
    return second;
}
async fn nested(value: Int) -> Int { return await delayed(value); }
fn forward(value: Int) -> Future<Int> { return nested(value); }
async fn full(value: CUInt64, text: String) -> CUInt64 {
    let first = await spawn high(value);
    var count = 0;
    while count < 20 { let moved = "GC temporary"; count = count + 1; }
    if text != "日本語\u{0}tail" { panic("lost_root"); }
    return first;
}
async fn many(a: Int, b: Int, c: Int, d: Int, e: Int, f: Int, g: Int, h: Int, i: Int) -> Int {
    let first = await spawn echo(a + b + c + d + e + f + g + h + i);
    return first;
}
async fn failed() -> CInt8 {
    try { let first = await spawn echo(7); return CInt8(200); }
    finally { let cleanup = CInt8(300); }
}
async fn waiting() -> Int {
    try { let value = await spawn forever(); return value; }
    finally { let cleanup = CInt8(400); }
}
async fn joining() -> Int { let child = waiting(); return 7; }
async fn caught() -> Int {
    try { let prefixFailure = CInt8(200); } catch prefix { }
    let value = await spawn echo(7);
    try { let resumedFailure = CInt8(300); } catch resumed { }
    return value;
}
fn launch() -> Int {
    try { let beforeFailure = CInt8(400); } catch before { }
    let child = caught();
    try { let afterFailure = CInt8(500); } catch after { }
    let second = spawn echo(8);
    return 7;
}
"#;
    for (body, outcome) in [
        (
            "var value = 7; let future = nested(value); value = 100; let copy = await future; if copy != 8 || value != 100 { panic(\"wrong\"); }",
            "success",
        ),
        (
            "var value = 7; let future = forward(value); value = 100; let copy = await future; if copy != 8 || value != 100 { panic(\"wrong\"); }",
            "success",
        ),
        (
            "let first = full(18446744073709551615, \"日本語\\u{0}tail\"); let second = full(18446744073709551614, \"日本語\\u{0}tail\"); var count = 0; while count < 20 { let temporary = \"caller GC\"; count = count + 1; } let one = await first; let two = await second; if one != 18446744073709551615 || two != 18446744073709551614 { panic(\"wrong\"); }",
            "success",
        ),
        (
            "let value = await many(1, 2, 3, 4, 5, 6, 7, 8, 9); if value != 45 { panic(\"wrong\"); }",
            "success",
        ),
        ("let dropped = failed();", "rejected:RZ0406"),
        (
            "var order = 0; let future = failed(); let alias = future; order = 1; try { let one = await future; } catch first { order = order * 10 + 2; } try { let two = await alias; } catch second { order = order * 10 + 3; } if order != 123 { panic(\"wrong\"); }",
            "success",
        ),
        ("let child = waiting(); panic(\"failure\");", "failure"),
        ("let child = joining(); panic(\"failure\");", "failure"),
        (
            "try { let child = joining(); panic(\"failure\"); } finally { let outerCleanup = CInt8(500); }",
            "failure",
        ),
        ("let value = launch();", "success"),
    ] {
        std::fs::write(
            root.join("src/main.rinzu"),
            checked_entry::source(&format!("{functions}\nasync fn main() {{ {body} }}")),
        )
        .unwrap();
        if let Some(code) = outcome.strip_prefix("rejected:") {
            assert_static_rejection(&root, code);
            continue;
        }
        let verify = Command::new(frozen_cli::path())
            .arg("verify-aot")
            .arg(&root)
            .output()
            .unwrap();
        assert!(
            verify.status.success(),
            "{body}: {}",
            String::from_utf8_lossy(&verify.stderr)
        );
        assert!(
            String::from_utf8_lossy(&verify.stdout).contains(&format!("outcome={outcome}")),
            "{}",
            String::from_utf8_lossy(&verify.stdout)
        );
    }
    // A ready dependency can coexist with a pending sibling. Both aliasing and
    // repeated iterations must return native continuation mappings to baseline.
    std::fs::write(root.join("src/main.rinzu"), format!("{functions}\nasync fn main() {{ var count = 0; while count < 40 {{ let value = await nested(count); count = count + 1; }} }}")).unwrap();
    let executable = root.join("program");
    let build = Command::new(frozen_cli::path())
        .arg("build")
        .arg(&root)
        .arg("--output")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&executable).output().unwrap();
    assert!(
        run.status.success(),
        "{} / {}",
        String::from_utf8_lossy(&run.stderr),
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = std::str::from_utf8(&run.stderr).unwrap().to_owned();
    let accounting = stdout
        .lines()
        .find_map(|line| line.strip_prefix("RINZU_CONTINUATIONS="))
        .unwrap();
    let accounting = accounting
        .split(',')
        .map(|value| value.parse::<usize>().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(accounting[0], 0);
    assert_eq!(accounting[1], 2);
    assert_eq!(accounting[2], 0);
    assert!(accounting[3] > 0);
    std::fs::write(root.join("src/main.rinzu"), checked_entry::source(&format!("{functions}\nasync fn main() {{ try {{ let future = nested(7); var count = 0; while count < 20 {{ count = count + 1; }} let value = await future; }} finally {{ let cleanup = CInt8(700); }} }}"))).unwrap();
    for limit in [1, 3, 5, 8, 12, 16, 24] {
        let verify = Command::new(frozen_cli::path())
            .arg("verify-aot")
            .arg(&root)
            .arg("--cancel-after-polls")
            .arg(limit.to_string())
            .output()
            .unwrap();
        assert!(
            verify.status.success(),
            "cancel after {limit}: {}",
            String::from_utf8_lossy(&verify.stderr)
        );
        assert!(String::from_utf8_lossy(&verify.stdout).contains("outcome=cancelled"));
    }
    std::fs::create_dir_all(root.join("probe/src")).unwrap();
    std::fs::write(root.join("probe/rinzu.toml"), "[package]\nname = \"probe\"\nlanguage = \"0.2\"\nkind = \"sdk\"\nentry = \"src/probe.rinzu\"\n").unwrap();
    std::fs::write(root.join("probe/src/probe.rinzu"), "fn echo(value: Int) -> Int { return value + 1000; } async fn delayed(value: Int) -> Int { return await spawn echo(value); } export echo; export delayed;").unwrap();
    std::fs::write(root.join("rinzu.toml"), "[package]\nname = \"pending_callee\"\nlanguage = \"0.2\"\n[dependencies]\nprobe = { path = \"probe\" }\n").unwrap();
    std::fs::write(root.join("src/main.rinzu"), "import probe; fn echo(value: Int) -> Int { return 0; } async fn main() { let first = await probe.delayed(7); let second = await spawn probe.echo(8); if first != 1007 || second != 1008 { panic(\"wrong_target\"); } }").unwrap();
    let verify = Command::new(frozen_cli::path())
        .arg("verify-aot")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        verify.status.success(),
        "qualified target: {}",
        String::from_utf8_lossy(&verify.stderr)
    );
    assert!(String::from_utf8_lossy(&verify.stdout).contains("outcome=success"));
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn compiled_composite_payload_dispatch_equality_and_roots_match_jit_aot() {
    let root = std::env::temp_dir().join(format!("rinzu-composite-aot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("rinzu.toml"),
        "[package]\nname = \"composite_aot\"\nlanguage = \"0.2\"\n",
    )
    .unwrap();
    let source = include_str!("../../codegen/tests/fixtures/composite.rinzu");
    for source in [
        source.to_owned(),
        source.replace("fn main()", "async fn main()"),
    ] {
        std::fs::write(root.join("src/main.rinzu"), source).unwrap();
        let output = Command::new(frozen_cli::path())
            .arg("verify-aot")
            .arg(&root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("outcome=success"));
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn compiled_fixed_bytes_and_collection_content_keys_match_jit_aot() {
    let root =
        std::env::temp_dir().join(format!("rinzu-bytes-collection-aot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("rinzu.toml"),
        "[package]\nname = \"bytes_collection_aot\"\nlanguage = \"0.2\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/main.rinzu"),
        include_str!("../../codegen/tests/fixtures/bytes_collection.rinzu"),
    )
    .unwrap();
    let output = Command::new(frozen_cli::path())
        .arg("verify-aot")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("outcome=success"));
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn typed_composite_worker_copies_and_heap_settlement_match_jit_aot() {
    let root =
        std::env::temp_dir().join(format!("rinzu-composite-worker-aot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("rinzu.toml"),
        "[package]\nname = \"composite_worker_aot\"\nlanguage = \"0.2\"\n",
    )
    .unwrap();
    let fixture = include_str!("../../codegen/tests/fixtures/worker_composite.rinzu");
    for (source, flags, outcome) in [
        (fixture.to_owned(), vec![], "outcome=success"),
        (
            format!(
                "{}\n{}",
                fixture.replace("async fn main()", "async fn successful_main()"),
                include_str!("../../codegen/tests/fixtures/worker_composite_cancel.rinzu")
            ),
            vec!["--cancel-worker-after-polls", "100"],
            "outcome=cancelled",
        ),
    ] {
        std::fs::write(root.join("src/main.rinzu"), source).unwrap();
        let output = Command::new(frozen_cli::path())
            .arg("verify-aot")
            .arg(&root)
            .args(flags)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains(outcome));
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn typed_function_references_and_core_lengths_match_jit_aot_on_main_and_workers() {
    let root = std::env::temp_dir().join(format!("rinzu-cli-reference-aot-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("rinzu.toml"),
        "[package]\nname=\"reference\"\nlanguage=\"0.2\"\n",
    )
    .unwrap();
    let fixture = include_str!("../../codegen/tests/fixtures/function_reference.rinzu");
    let asynchronous = format!(
        "{}\nasync fn forwarded() -> Int {{ return await spawn verify(); }} async fn main() {{ let on_main=verify(); let reference=forwarded; let pending=reference(); let on_worker=await pending; if on_main!=42 || on_worker!=42 {{ panic(\"bad_result\"); }} }}",
        fixture.replace("fn main()", "fn unused_main()")
    );
    for source in [fixture.to_owned(), asynchronous] {
        std::fs::write(root.join("src/main.rinzu"), source).unwrap();
        let output = Command::new(frozen_cli::path())
            .arg("verify-aot")
            .arg(&root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("outcome=success"));
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn parked_prepared_and_completed_composite_callee_roots_match_jit_aot() {
    let root =
        std::env::temp_dir().join(format!("rinzu-composite-callee-aot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("rinzu.toml"),
        "[package]\nname = \"composite_callee_aot\"\nlanguage = \"0.2\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/main.rinzu"),
        include_str!("../../codegen/tests/fixtures/async_composite.rinzu"),
    )
    .unwrap();
    let output = Command::new(frozen_cli::path())
        .arg("verify-aot")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("outcome=success"));
    let source = include_str!("../../codegen/tests/fixtures/async_composite.rinzu")
        .replace("async fn main()", "async fn successful_main()");
    let source = format!(
        "{source}\n{}",
        include_str!("../../codegen/tests/fixtures/async_composite_cancel.rinzu")
    );
    std::fs::write(root.join("src/main.rinzu"), source).unwrap();
    let output = Command::new(frozen_cli::path())
        .arg("verify-aot")
        .arg(&root)
        .args(["--cancel-after-polls", "100"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("outcome=cancelled"));
    std::fs::remove_dir_all(root).unwrap();
}
