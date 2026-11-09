use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Output, Stdio};

const FUZZ_TARGETS: &[&str] = &["trace_schema", "source_formatter"];

fn main() -> ExitCode {
    match run(env::args_os().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("xtask: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: Vec<OsString>) -> Result<(), String> {
    let Some(command) = arguments.first().and_then(|value| value.to_str()) else {
        return Err(usage());
    };
    match command {
        "fmt" => {
            run_rinzu("fmt")?;
            format_rust(true)?;
            run_native("fmt", &arguments[1..])
        }
        "fmt-write" => {
            run_rinzu("fmt-write")?;
            format_rust(false)?;
            run_native("fmt-write", &arguments[1..])
        }
        "lint" => {
            run_rinzu("lint")?;
            run_inherited(spec(
                "cargo",
                [
                    "clippy",
                    "--workspace",
                    "--all-targets",
                    "--all-features",
                    "--",
                    "-D",
                    "warnings",
                ],
            ))?;
            run_native("lint", &arguments[1..])
        }
        "c-fmt" => run_native("fmt", &arguments[1..]),
        "rinzu-fmt" => run_rinzu_paths("fmt", &arguments[1..]),
        "rinzu-fmt-write" => run_rinzu_paths("fmt-write", &arguments[1..]),
        "rinzu-lint" => run_rinzu_paths("lint", &arguments[1..]),
        "c-fmt-write" => run_native("fmt-write", &arguments[1..]),
        "c-lint" => run_native("lint", &arguments[1..]),
        "c-db" => run_native("db", &arguments[1..]),
        "test" => {
            let mut failures = Vec::new();
            for (name, step) in test_steps() {
                println!("==> {name}");
                if let Err(error) = run_inherited(step) {
                    failures.push(format!("{name}: {error}"));
                }
            }
            if failures.is_empty() {
                Ok(())
            } else {
                Err(failures.join("\n"))
            }
        }
        "sanitize" => {
            let directory = root().join("tests/fixtures/c-abi");
            run_inherited(spec("make", ["sanitize"]).in_directory(directory.clone()))?;
            run_inherited(spec("make", ["thread-sanitize"]).in_directory(directory))
        }
        "memory-sanitize" => run_inherited(
            spec("make", ["memory-sanitize"]).in_directory(root().join("tests/fixtures/c-abi")),
        ),
        "race-sanitize" => run_inherited(
            spec("make", ["race-sanitize"]).in_directory(root().join("tests/fixtures/c-abi")),
        ),
        "model" => run_inherited(spec(
            "cargo",
            ["test", "--manifest-path", "tests/runtime-model/Cargo.toml"],
        )),
        "host-stress" => {
            let mut stress_arguments = vec![
                "run".into(),
                "--package".into(),
                "rinzu-conformance".into(),
                "--".into(),
                "host-stress".into(),
            ];
            stress_arguments.extend(arguments.into_iter().skip(1));
            run_inherited(Spec {
                program: "cargo".into(),
                arguments: stress_arguments,
                directory: root(),
                environment: Vec::new(),
            })
        }
        "differential" => run_inherited(spec(
            "cargo",
            [
                "test",
                "--package",
                "rinzu-driver",
                "--test",
                "cli",
                "jit_and_aot_traces_match_for_success_error_cancel_overload_and_shutdown",
                "--",
                "--exact",
                "--nocapture",
            ],
        )),
        "resource-profile" => {
            let output = arguments
                .get(1)
                .cloned()
                .unwrap_or_else(|| "artifacts/architecture/resource-profile.json".into());
            run_inherited(spec_os(
                "cargo",
                [
                    "run".into(),
                    "--package".into(),
                    "rinzu-conformance".into(),
                    "--".into(),
                    "resource-profile".into(),
                    output,
                ],
            ))
        }
        "fuzz-build" => run_fuzz_targets("build", &[]),
        "fuzz" => {
            let seconds = arguments
                .get(1)
                .and_then(|value| value.to_str())
                .unwrap_or("10");
            run_fuzz_targets("run", &["--", &format!("-max_total_time={seconds}")])
        }
        "fuzz-replay" => {
            let artifact = arguments
                .get(1)
                .cloned()
                .ok_or_else(|| "fuzz-replay requires an artifact path".to_owned())?;
            let target = arguments
                .get(2)
                .map(|name| name.to_str().filter(|name| FUZZ_TARGETS.contains(name)))
                .unwrap_or(Some("trace_schema"))
                .ok_or_else(|| {
                    "fuzz-replay target must be trace_schema or source_formatter".to_owned()
                })?;
            run_inherited(Spec {
                program: "cargo".into(),
                arguments: vec!["fuzz".into(), "run".into(), target.into(), artifact],
                directory: root().join("fuzz"),
                environment: vec![("RUSTC".into(), nightly_rustc()?)],
            })
        }
        "trace-validate" => {
            let path = arguments
                .get(1)
                .cloned()
                .unwrap_or_else(|| "tests/conformance/trace/valid/basic.json".into());
            run_inherited(spec_os(
                "cargo",
                [
                    "run".into(),
                    "--package".into(),
                    "rinzu-conformance".into(),
                    "--".into(),
                    "trace".into(),
                    "validate".into(),
                    path,
                ],
            ))
        }
        "backend-spike" => {
            let path = arguments
                .get(1)
                .cloned()
                .unwrap_or_else(|| "artifacts/backend-spike".into());
            run_inherited(spec_os(
                "cargo",
                [
                    "run".into(),
                    "--package".into(),
                    "rinzu-conformance".into(),
                    "--".into(),
                    "backend-spike".into(),
                    path,
                ],
            ))
        }
        "ci" => {
            let artifacts = artifact_argument(&arguments)?;
            run_ci(&artifacts)
        }
        _ => Err(usage()),
    }
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rinzu_spec(command: &str) -> Spec {
    let mut arguments = vec![
        "run",
        "--package",
        "rinzu-compiler",
        "--example",
        "repository_quality",
    ];
    if command == "lint" {
        arguments.push("--release");
    }
    arguments.extend(["--", command]);
    Spec {
        program: "cargo".into(),
        arguments: arguments.into_iter().map(OsString::from).collect(),
        directory: root(),
        environment: Vec::new(),
    }
}

fn run_rinzu(command: &str) -> Result<(), String> {
    run_inherited(rinzu_spec(command))
}

fn run_rinzu_paths(command: &str, paths: &[OsString]) -> Result<(), String> {
    let mut specification = rinzu_spec(command);
    specification.arguments.extend_from_slice(paths);
    run_inherited(specification)
}

fn run_native(command: &str, paths: &[OsString]) -> Result<(), String> {
    let mut arguments = vec!["tools/native_quality.py".into(), command.into()];
    arguments.extend_from_slice(paths);
    run_inherited(Spec {
        program: "python3".into(),
        arguments,
        directory: root(),
        environment: Vec::new(),
    })
}

fn rustfmt_steps(check: bool) -> impl Iterator<Item = (&'static str, Spec)> {
    [
        ("fmt", None),
        ("runtime-model-fmt", Some("tests/runtime-model/Cargo.toml")),
        ("fuzz-fmt", Some("fuzz/Cargo.toml")),
    ]
    .into_iter()
    .map(move |(name, manifest)| {
        let mut step = spec("cargo", ["fmt", "--all"]);
        if let Some(manifest) = manifest {
            step.arguments
                .extend(["--manifest-path".into(), manifest.into()]);
        }
        if check {
            step.arguments.extend(["--".into(), "--check".into()]);
        }
        (name, step)
    })
}

fn format_rust(check: bool) -> Result<(), String> {
    for (_, step) in rustfmt_steps(check) {
        run_inherited(step)?;
    }
    Ok(())
}

fn test_steps() -> impl IntoIterator<Item = (&'static str, Spec)> {
    [
        (
            "test",
            spec(
                "cargo",
                [
                    "test",
                    "--workspace",
                    "--all-targets",
                    "--all-features",
                    "--no-fail-fast",
                ],
            ),
        ),
        (
            "rust-doctests",
            spec("cargo", ["test", "--workspace", "--doc", "--all-features"]),
        ),
        (
            "runtime-model",
            spec(
                "cargo",
                ["test", "--manifest-path", "tests/runtime-model/Cargo.toml"],
            ),
        ),
        (
            "vscode-unit-tests",
            spec(
                "node",
                ["--test", "tools/vscode-rinzu/diagnostics.test.cjs"],
            ),
        ),
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        (
            "trampoline-test",
            spec("python3", ["tests/integration/test_trampoline.py"]),
        ),
    ]
}

fn run_fuzz_targets(command: &str, options: &[&str]) -> Result<(), String> {
    let rustc = nightly_rustc()?;
    for target in FUZZ_TARGETS {
        let mut step = spec("cargo", ["fuzz", command, target])
            .with_environment("RUSTC", rustc.clone())
            .in_directory(root().join("fuzz"));
        step.arguments.extend(options.iter().map(OsString::from));
        run_inherited(step)?;
    }
    Ok(())
}

struct Spec {
    program: OsString,
    arguments: Vec<OsString>,
    directory: PathBuf,
    environment: Vec<(OsString, OsString)>,
}

impl Spec {
    fn in_directory(mut self, directory: PathBuf) -> Self {
        self.directory = directory;
        self
    }

    fn with_environment(mut self, name: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.environment.push((name.into(), value.into()));
        self
    }
}

fn spec<const N: usize>(program: &str, arguments: [&str; N]) -> Spec {
    spec_os(program, arguments.map(OsString::from))
}

fn spec_os<const N: usize>(program: &str, arguments: [OsString; N]) -> Spec {
    Spec {
        program: program.into(),
        arguments: arguments.into(),
        directory: root(),
        environment: Vec::new(),
    }
}

fn command(specification: &Spec) -> Command {
    let mut command = Command::new(&specification.program);
    command
        .args(&specification.arguments)
        .current_dir(&specification.directory)
        .envs(specification.environment.iter().cloned());
    command
}

fn nightly_rustc() -> Result<OsString, String> {
    let output = Command::new("rustup")
        .args(["which", "--toolchain", "nightly", "rustc"])
        .output()
        .map_err(|error| format!("failed to locate nightly rustc: {error}"))?;
    if !output.status.success() {
        return Err("nightly Rust is required; run `rustup toolchain install nightly`".into());
    }
    let path = String::from_utf8(output.stdout)
        .map_err(|error| format!("nightly rustc path is not UTF-8: {error}"))?;
    Ok(path.trim().into())
}

fn run_inherited(specification: Spec) -> Result<(), String> {
    let status = command(&specification)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|error| format!("failed to start {:?}: {error}", specification.program))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{:?} exited with {status}", specification.program))
    }
}

fn artifact_argument(arguments: &[OsString]) -> Result<PathBuf, String> {
    match arguments.get(1).and_then(|value| value.to_str()) {
        None => Ok(root().join("artifacts/ci")),
        Some("--artifacts") => arguments
            .get(2)
            .map(PathBuf::from)
            .ok_or_else(|| "--artifacts requires a path".into()),
        Some(_) => Err("ci accepts only --artifacts <path>".into()),
    }
}

fn run_ci(artifacts: &Path) -> Result<(), String> {
    let artifacts = if artifacts.is_absolute() {
        artifacts.to_path_buf()
    } else {
        root().join(artifacts)
    };
    fs::create_dir_all(&artifacts)
        .map_err(|error| format!("create {}: {error}", artifacts.display()))?;

    let environment = [
        ("rustc", spec("rustc", ["--version", "--verbose"])),
        ("cargo", spec("cargo", ["--version", "--verbose"])),
        (
            "native-tools",
            spec("python3", ["tools/native_quality.py", "versions"]),
        ),
    ];
    for (name, specification) in environment {
        run_logged(name, specification, &artifacts)?;
    }

    let backend_directory = artifacts.join("backend-spike");
    let backend_argument = backend_directory.as_os_str().to_os_string();
    let resource_profile = artifacts.join("resource-profile.json");
    let resource_profile_argument = resource_profile.as_os_str().to_os_string();
    for (name, step) in [
        ("rinzu-fmt", rinzu_spec("fmt")),
        ("c-fmt", spec("python3", ["tools/native_quality.py", "fmt"])),
    ]
    .into_iter()
    .chain(rustfmt_steps(true))
    {
        run_logged(name, step, &artifacts)?;
    }

    let lint = [
        ("rinzu-lint", rinzu_spec("lint")),
        (
            "c-lint",
            spec("python3", ["tools/native_quality.py", "lint"]),
        ),
        (
            "c-tool-tests",
            spec("python3", ["tests/integration/test_native_quality.py"]),
        ),
        (
            "c-editor-tests",
            spec("python3", ["tests/integration/test_native_editor.py"]),
        ),
        (
            "lint",
            spec(
                "cargo",
                [
                    "clippy",
                    "--workspace",
                    "--all-targets",
                    "--all-features",
                    "--",
                    "-D",
                    "warnings",
                ],
            ),
        ),
    ];
    for (name, step) in lint {
        run_logged(name, step, &artifacts)?;
    }
    for (name, step) in test_steps() {
        let step = if name == "test" {
            step.with_environment("RINZU_GATE_ARTIFACTS", artifacts.join("native"))
        } else {
            step
        };
        run_logged(name, step, &artifacts)?;
    }

    let steps = [
        (
            "trace-schema",
            spec(
                "cargo",
                [
                    "run",
                    "--package",
                    "rinzu-conformance",
                    "--",
                    "trace-schema",
                ],
            ),
        ),
        (
            "trace-fixture",
            spec(
                "cargo",
                [
                    "run",
                    "--package",
                    "rinzu-conformance",
                    "--",
                    "trace",
                    "validate",
                    "tests/conformance/trace/valid/basic.json",
                ],
            ),
        ),
        (
            "sanitize",
            spec("make", ["sanitize"]).in_directory(root().join("tests/fixtures/c-abi")),
        ),
        (
            "thread-sanitize",
            spec("make", ["thread-sanitize"]).in_directory(root().join("tests/fixtures/c-abi")),
        ),
        (
            "host-stress",
            spec(
                "cargo",
                [
                    "run",
                    "--package",
                    "rinzu-conformance",
                    "--",
                    "host-stress",
                    "--seed",
                    "8241981735943431729",
                    "--rounds",
                    "100",
                ],
            ),
        ),
        (
            "resource-profile",
            spec_os(
                "cargo",
                [
                    "run".into(),
                    "--package".into(),
                    "rinzu-conformance".into(),
                    "--".into(),
                    "resource-profile".into(),
                    resource_profile_argument,
                ],
            ),
        ),
        (
            "backend-spike",
            spec_os(
                "cargo",
                [
                    "run".into(),
                    "--package".into(),
                    "rinzu-conformance".into(),
                    "--".into(),
                    "backend-spike".into(),
                    backend_argument,
                ],
            ),
        ),
    ];
    for (name, specification) in steps {
        run_logged(name, specification, &artifacts)?;
    }
    println!("CI artifacts: {}", artifacts.display());
    Ok(())
}

fn run_logged(name: &str, specification: Spec, artifacts: &Path) -> Result<(), String> {
    println!("==> {name}");
    let output = command(&specification)
        .output()
        .map_err(|error| format!("failed to start {:?}: {error}", specification.program))?;
    save_output(name, &output, artifacts)?;
    print_output(&output);
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("{name} failed with {}", output.status))
    }
}

fn save_output(name: &str, output: &Output, artifacts: &Path) -> Result<(), String> {
    let mut bytes = output.stdout.clone();
    bytes.extend_from_slice(&output.stderr);
    fs::write(artifacts.join(format!("{name}.log")), bytes)
        .map_err(|error| format!("save {name} log: {error}"))
}

fn print_output(output: &Output) {
    print!("{}", String::from_utf8_lossy(&output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
}

fn usage() -> String {
    "usage: cargo xtask <fmt|fmt-write|lint|rinzu-fmt [paths...]|rinzu-fmt-write [paths...]|rinzu-lint [paths...]|c-fmt [paths...]|c-fmt-write [paths...]|c-lint [paths...]|c-db|test|sanitize|memory-sanitize|race-sanitize|model|host-stress [options]|differential|resource-profile [file]|fuzz-build|fuzz [seconds]|fuzz-replay <artifact> [target]|trace-validate [file]|backend-spike [dir]|ci [--artifacts dir]>".into()
}
