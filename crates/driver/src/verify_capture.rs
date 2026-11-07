//! CLI verification executes in a disposable process. Its original typed run
//! stays there for semantic checks; the parent compares bytes after atexit.
use std::cell::RefCell;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use rinzu_codegen::BaselineOptions;
use serde::{Deserialize, Serialize};

thread_local! {
    static WORKER_DIRECTORY: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u32,
    input: InputRequest,
    cancel_after_polls: Option<usize>,
    cancel_worker_after_polls: Option<usize>,
    max_allocations: Option<usize>,
    shutdown_after_polls: Option<usize>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputRequest {
    path: PathBuf,
    options: crate::input::Options,
    native: rinzu_compiler::NativeCliInputs,
    mode: rinzu_compiler::CompilationMode,
    entry: PathBuf,
    inputs: Vec<rinzu_compiler::CompilationInput>,
}
impl InputRequest {
    fn capture(context: &rinzu_compiler::CompilationContext) -> Self {
        Self {
            path: match context.mode {
                rinzu_compiler::CompilationMode::File => context.entry.clone(),
                rinzu_compiler::CompilationMode::Package => context.metadata.root.clone(),
            },
            options: crate::input::Options {
                language: context.options.language.clone(),
                target: Some(context.target.clone()),
            },
            native: context.artifact.native_inputs.clone(),
            mode: context.mode,
            entry: context.entry.clone(),
            inputs: context.inputs.clone(),
        }
    }
    fn check(&self) -> Result<rinzu_compiler::CompilationContext, String> {
        let context = crate::input::check(&self.path, &self.options, &self.native)?;
        if context.mode != self.mode || context.entry != self.entry || context.inputs != self.inputs
        {
            return Err("verification input changed between JIT/AOT checks".into());
        }
        Ok(context)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BuildRequest {
    version: u32,
    input: InputRequest,
    executable: PathBuf,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    version: u32,
    result: Option<i64>,
    outcome: Option<String>,
    error: Option<String>,
    diagnostic_blocks: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BuildResponse {
    result: Result<crate::aot::AotBuild, String>,
    diagnostic_blocks: Vec<String>,
}

pub(crate) struct TemporaryDirectory(pub PathBuf);
impl TemporaryDirectory {
    pub fn new(label: &str) -> Result<Self, String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "rinzu-{label}-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&path)
            .map_err(|error| format!("create verification directory: {error}"))?;
        Ok(Self(path))
    }
}
impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn worker_file(name: &str) -> Option<PathBuf> {
    WORKER_DIRECTORY.with(|directory| directory.borrow().as_ref().map(|path| path.join(name)))
}

pub fn active() -> bool {
    worker_file("request.json").is_some()
}

const CONTROLS: [&str; 10] = [
    // Compiler instrumentation must not enter the program-byte comparison.
    "RINZU_COMPILE_TIMINGS",
    "RINZU_CANCEL_AFTER_POLLS",
    "RINZU_CANCEL_WORKER_AFTER_POLLS",
    "RINZU_SHUTDOWN_AFTER_POLLS",
    "RINZU_SHUTDOWN_WORKER_AFTER_POLLS",
    "RINZU_MAX_ALLOCATIONS",
    "RINZU_EVIDENCE_OUTPUT",
    "RINZU_DIAGNOSTIC_OUTPUT",
    "RINZU_RESOURCE_REPORT",
    "RINZU_RESOURCE_INTERVAL_MS",
];

pub fn configure_aot(command: &mut Command) {
    for name in CONTROLS {
        command.env_remove(name);
    }
    if let Some(report) = worker_file("aot-evidence") {
        command.env("RINZU_EVIDENCE_OUTPUT", report);
        command.env(
            "RINZU_DIAGNOSTIC_OUTPUT",
            worker_file("aot-diagnostics").unwrap(),
        );
    }
}

/// Native descriptor validation may dlopen an Objective-C SDK. Keep its classes
/// and constructors out of the JIT process, where that SDK is already loaded.
pub fn build_aot(
    context: &rinzu_compiler::CompilationContext,
    executable: PathBuf,
) -> Result<crate::aot::AotBuild, String> {
    if !active() {
        return crate::aot::build_checked_input(context, Some(executable));
    }
    let response = worker_file("build.json").unwrap();
    let request_path = worker_file("build-request.json").unwrap();
    fs::write(
        &request_path,
        serde_json::to_vec(&BuildRequest {
            version: 2,
            input: InputRequest::capture(context),
            executable,
        })
        .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let mut command = Command::new(std::env::current_exe().map_err(|error| error.to_string())?);
    command
        .arg("__verify-aot-build")
        .arg(request_path)
        .arg(&response);
    for name in CONTROLS {
        command.env_remove(name);
    }
    let output = command
        .output()
        .map_err(|error| format!("execute verification build: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "verification build failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let response: BuildResponse =
        serde_json::from_slice(&fs::read(response).map_err(|error| error.to_string())?)
            .map_err(|error| format!("invalid verification build protocol: {error}"))?;
    for block in response.diagnostic_blocks {
        crate::cli_diagnostics::write_block(&block);
    }
    response.result
}

pub fn build_worker(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let request_path = PathBuf::from(
        arguments
            .next()
            .ok_or("missing verification build request")?,
    );
    let response = PathBuf::from(
        arguments
            .next()
            .ok_or("missing verification build result")?,
    );
    if arguments.next().is_some() {
        return Err("unexpected verification build argument".into());
    }
    let request: BuildRequest =
        serde_json::from_slice(&fs::read(request_path).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    if request.version != 2 {
        return Err("unsupported verification build request protocol".into());
    }
    let (result, diagnostic_blocks) = crate::cli_diagnostics::capture(|| {
        let context = request.input.check()?;
        crate::aot::build_checked_input(&context, Some(request.executable))
    });
    fs::write(
        response,
        serde_json::to_vec(&BuildResponse {
            result,
            diagnostic_blocks,
        })
        .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

/// Defer byte comparison until the JIT worker has exited, including SDK atexit
/// handlers and buffered writes. The typed comparator consumes only evidence.
pub fn evidence(output: &Output) -> Result<Vec<u8>, String> {
    let Some(report) = worker_file("aot-evidence") else {
        validate_evidence(output, &output.stderr, false)?;
        return Ok(output.stderr.clone());
    };
    fs::write(worker_file("aot-stdout").unwrap(), &output.stdout)
        .and_then(|()| fs::write(worker_file("aot-stderr").unwrap(), &output.stderr))
        .map_err(|error| format!("save AOT program output: {error}"))?;
    let report = fs::read(report)
        .map_err(|error| incomplete_evidence(output, &format!("no complete evidence: {error}")))?;
    validate_evidence(output, &report, true)?;
    Ok(report)
}

fn incomplete_evidence(output: &Output, reason: &str) -> String {
    const LIMIT: usize = 4096;
    let stderr = String::from_utf8_lossy(&output.stderr[..output.stderr.len().min(LIMIT)]);
    format!(
        "AOT comparison incomplete: status={}, {reason}; native stderr: {}{}",
        output.status,
        if stderr.is_empty() {
            "(empty)"
        } else {
            &stderr
        },
        if output.stderr.len() > LIMIT {
            " [truncated]"
        } else {
            ""
        }
    )
}

fn validate_evidence(output: &Output, report: &[u8], needs_completion: bool) -> Result<(), String> {
    if output.status.code().is_none() {
        return Err(incomplete_evidence(output, "process terminated by signal"));
    }
    if needs_completion && !report.ends_with(b"RINZU_REPORT_COMPLETE=1\n") {
        return Err(incomplete_evidence(output, "evidence not complete"));
    }
    if !report
        .split(|byte| *byte == b'\n')
        .any(|line| line.starts_with(b"RINZU_RESULT="))
    {
        return Err(incomplete_evidence(
            output,
            "runtime result report is missing",
        ));
    }
    // Managed failures can legitimately return nonzero with a complete report.
    // Their status/result comparison belongs to the typed verifier below.
    Ok(())
}

pub fn verify_with_input(
    path: &Path,
    input_options: &crate::input::Options,
    native: &rinzu_compiler::NativeCliInputs,
    options: BaselineOptions,
    cancel_worker_after_polls: Option<usize>,
    program_arguments: &[std::ffi::OsString],
) -> Result<crate::aot::EquivalenceReport, String> {
    let context = crate::input::check(path, input_options, native)?;
    let directory = TemporaryDirectory::new("verify-worker")?;
    let request = Request {
        version: 2,
        input: InputRequest::capture(&context),
        cancel_after_polls: options.cancel_after_polls,
        cancel_worker_after_polls,
        max_allocations: options.max_allocations,
        shutdown_after_polls: options.shutdown_after_polls,
    };
    fs::write(
        directory.0.join("request.json"),
        serde_json::to_vec(&request).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let mut command = Command::new(std::env::current_exe().map_err(|error| error.to_string())?);
    command
        .arg("__verify-aot-worker")
        .arg(&directory.0)
        .arg("--")
        .args(program_arguments);
    for name in CONTROLS {
        command.env_remove(name);
    }
    command.env(
        "RINZU_DIAGNOSTIC_OUTPUT",
        directory.0.join("jit-diagnostics"),
    );
    command
        .stdout(File::create(directory.0.join("jit-stdout")).map_err(|error| error.to_string())?);
    command
        .stderr(File::create(directory.0.join("jit-stderr")).map_err(|error| error.to_string())?);
    let status = command
        .status()
        .map_err(|error| format!("execute verification worker: {error}"))?;
    let response = fs::read(directory.0.join("response.json")).map_err(|error| {
        format!(
            "JIT/AOT comparison incomplete: worker status={status}, no result protocol: {error}"
        )
    })?;
    let response: Response = serde_json::from_slice(&response)
        .map_err(|error| format!("invalid verification result protocol: {error}"))?;
    if response.version != 1 {
        return Err("unsupported verification result protocol".into());
    }
    for block in response.diagnostic_blocks {
        crate::cli_diagnostics::write_block(&block);
    }
    if let Some(error) = response.error {
        return Err(error);
    }
    if !status.success() {
        return Err(format!(
            "JIT/AOT comparison incomplete: worker status={status}"
        ));
    }
    for stream in ["stdout", "stderr"] {
        let jit = fs::read(directory.0.join(format!("jit-{stream}")))
            .map_err(|error| error.to_string())?;
        let aot = fs::read(directory.0.join(format!("aot-{stream}")))
            .map_err(|error| error.to_string())?;
        if jit != aot {
            let first = jit
                .iter()
                .zip(&aot)
                .position(|(left, right)| left != right)
                .unwrap_or(jit.len().min(aot.len()));
            return Err(format!(
                "JIT/AOT program {stream} bytes differ at byte {first} (JIT={}, AOT={})",
                jit.len(),
                aot.len()
            ));
        }
    }
    let outcome = match response.outcome.as_deref() {
        Some("success") => "success",
        Some("cancelled") => "cancelled",
        Some("overloaded") => "overloaded",
        Some("shutdown") => "shutdown",
        Some("failure") => "failure",
        _ => return Err("invalid verification result outcome".into()),
    };
    Ok(crate::aot::EquivalenceReport {
        result: response.result.ok_or("missing verification result")?,
        outcome,
    })
}

pub fn worker(
    mut arguments: impl Iterator<Item = String>,
    program_arguments: &[std::ffi::OsString],
) -> Result<(), String> {
    let directory = PathBuf::from(
        arguments
            .next()
            .ok_or("missing verification worker directory")?,
    );
    if arguments.next().is_some() {
        return Err("unexpected verification worker argument".into());
    }
    let request: Request = serde_json::from_slice(
        &fs::read(directory.join("request.json")).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if request.version != 2 {
        return Err("unsupported verification request protocol".into());
    }
    WORKER_DIRECTORY.with(|slot| *slot.borrow_mut() = Some(directory.clone()));
    let (result, diagnostic_blocks) = crate::cli_diagnostics::capture(|| {
        request.input.check().and_then(|context| {
            if let Some(limit) = request.cancel_worker_after_polls {
                crate::aot::verify_worker_cancel_checked(&context, limit, program_arguments)
            } else {
                crate::aot::verify_checked_input(
                    &context,
                    BaselineOptions {
                        cancel_after_polls: request.cancel_after_polls,
                        max_allocations: request.max_allocations,
                        shutdown_after_polls: request.shutdown_after_polls,
                        ..BaselineOptions::default()
                    },
                    program_arguments,
                )
            }
        })
    });
    let response = match result {
        Ok(report) => Response {
            version: 1,
            result: Some(report.result),
            outcome: Some(report.outcome.into()),
            error: None,
            diagnostic_blocks,
        },
        Err(error) => Response {
            version: 1,
            result: None,
            outcome: None,
            error: Some(error),
            diagnostic_blocks,
        },
    };
    fs::write(
        directory.join("response.json"),
        serde_json::to_vec(&response).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

#[cfg(all(test, unix))]
mod evidence_tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    fn output(status: i32, stderr: &[u8]) -> Output {
        Output {
            status: std::process::ExitStatus::from_raw(status),
            stdout: b"partly written program stdout".to_vec(),
            stderr: stderr.to_vec(),
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn program_argument_snapshots_belong_to_each_embedding() {
        let directory = TemporaryDirectory::new("embedded-arguments").unwrap();
        let entry = directory.0.join("entry.rinzu");
        fs::write(
            &entry,
            include_str!("../tests/fixtures/process_arguments.rinzu"),
        )
        .unwrap();
        let checked = crate::input::check(
            &entry,
            &crate::input::Options::default(),
            &rinzu_compiler::NativeCliInputs::default(),
        )
        .unwrap();
        for value in ["初回", "second"] {
            let run = rinzu_codegen::run_baseline_jit_with_arguments(
                &checked.artifact.ir,
                "main",
                BaselineOptions {
                    capture_stdout: true,
                    collect_every_allocation: true,
                    ..Default::default()
                },
                None,
                None,
                vec![value.as_bytes().to_vec()],
            )
            .unwrap();
            assert_eq!(run.result, Ok(0));
            assert_eq!(run.stdout, format!("<{value}>\n").as_bytes());
            assert_eq!(run.heap_objects, 0);
        }
    }

    #[test]
    fn aot_evidence_reports_native_fatal_status_before_missing_proof() {
        let error = evidence(&output(70 << 8, b"native guard diagnostic")).unwrap_err();
        assert!(
            error.contains("70") && error.contains("native guard diagnostic"),
            "{error}"
        );
        assert!(error.contains("runtime result report is missing"));
        assert!(!error.contains("Future admission proof"));
        let signaled = evidence(&output(11, b"native signal diagnostic")).unwrap_err();
        assert!(
            signaled.contains("terminated by signal")
                && signaled.contains("native signal diagnostic")
        );
    }

    #[test]
    fn aot_evidence_allows_managed_nonzero_and_bounds_native_diagnostics() {
        let report = b"RINZU_RESULT=0\nRINZU_FAILURE=error\n";
        assert_eq!(evidence(&output(1 << 8, report)).unwrap(), report);
        let mut native = vec![255; 5000];
        native[0] = b'X';
        let error = evidence(&output(70 << 8, &native)).unwrap_err();
        assert!(error.contains("[truncated]") && error.len() < 13000);
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn aot_evidence_keeps_real_sync_and_async_failure_equivalence() {
        let directory = TemporaryDirectory::new("nonzero-equivalence").unwrap();
        for (name, source) in [
            ("sync", "fn main() {let arithmeticFailure = 1 / 0;}"),
            ("async", "async fn main() {let arithmeticFailure = 1 / 0;}"),
        ] {
            let entry = directory.0.join(format!("{name}.rinzu"));
            fs::write(&entry, source).unwrap();
            crate::aot::verify_equivalence(&entry, None).unwrap();
        }
    }

    #[test]
    fn aot_evidence_file_requires_completion_and_keeps_failure_context() {
        let directory = TemporaryDirectory::new("evidence-check").unwrap();
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                WORKER_DIRECTORY.with(|slot| *slot.borrow_mut() = None);
            }
        }
        WORKER_DIRECTORY.with(|slot| {
            assert!(slot.borrow().is_none());
            *slot.borrow_mut() = Some(directory.0.clone());
        });
        let _reset = Reset;
        let failed = output(70 << 8, b"native fatal in captured mode");
        assert!(
            evidence(&failed)
                .unwrap_err()
                .contains("native fatal in captured mode")
        );
        let file = directory.0.join("aot-evidence");
        fs::write(&file, b"RINZU_RESULT=0\n").unwrap();
        assert!(
            evidence(&failed)
                .unwrap_err()
                .contains("evidence not complete")
        );
        let complete = b"RINZU_RESULT=0\nRINZU_FAILURE=error\nRINZU_REPORT_COMPLETE=1\n";
        fs::write(&file, complete).unwrap();
        assert_eq!(
            evidence(&output(1 << 8, b"managed failure")).unwrap(),
            complete
        );
        assert_eq!(
            fs::read(directory.0.join("aot-stderr")).unwrap(),
            b"managed failure"
        );
        fs::write(&file, b"RINZU_REPORT_COMPLETE=1\n").unwrap();
        assert!(
            evidence(&failed)
                .unwrap_err()
                .contains("runtime result report is missing")
        );
    }
}

#[cfg(test)]
mod input_tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        let source = rinzu_compiler::SourceFile::new(rinzu_compiler::FileId(1), path, text);
        fs::write(path, rinzu_compiler::format_source(&source).unwrap()).unwrap();
    }

    #[test]
    fn build_request_rejects_import_edits_and_same_bytes_symlink_retargeting() {
        let directory = TemporaryDirectory::new("import-request").unwrap();
        let path = directory.0.join("entry.rinzu");
        let first = directory.0.join("first.rinzu");
        let second = directory.0.join("second.rinzu");
        let alias = directory.0.join("alias.rinzu");
        write(
            &path,
            "import \"./alias.rinzu\" as model;fn main(){let n:Int=model.value();}",
        );
        write(&first, "fn value()->Int{return 1;}export value;");
        write(&second, "fn value()->Int{return 1;}export value;");
        std::os::unix::fs::symlink(&first, &alias).unwrap();
        let context = crate::input::check(
            &path,
            &crate::input::Options::default(),
            &rinzu_compiler::NativeCliInputs::default(),
        )
        .unwrap();
        let request = InputRequest::capture(&context);
        assert_eq!(request.inputs.len(), 2);
        assert!(request.check().is_ok());
        write(&first, "fn value()->Int{return 2;}export value;");
        assert!(request.check().unwrap_err().contains("input changed"));
        write(&first, "fn value()->Int{return 1;}export value;");
        assert!(request.check().is_ok());
        fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(&second, &alias).unwrap();
        assert!(request.check().unwrap_err().contains("input changed"));
    }

    #[test]
    fn build_request_keeps_explicit_entry_and_rejects_changed_source_receipt() {
        let directory = TemporaryDirectory::new("file-request").unwrap();
        let path = directory.0.join("arbitrary name.rinzu");
        write(&path, "fn main() { let value=1; }");
        fs::write(
            directory.0.join("rinzu.toml"),
            "invalid nearby configuration",
        )
        .unwrap();
        let context = crate::input::check(
            &path,
            &crate::input::Options::default(),
            &rinzu_compiler::NativeCliInputs::default(),
        )
        .unwrap();
        let request = InputRequest::capture(&context);
        let encoded = serde_json::to_vec(&request).unwrap();
        let decoded: InputRequest = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.check().unwrap().entry, context.entry);
        write(&path, "fn main() { let value=2; }");
        assert!(decoded.check().unwrap_err().contains("input changed"));
    }

    #[test]
    fn build_request_rejects_changed_package_entry_even_when_both_mains_are_valid() {
        let directory = TemporaryDirectory::new("package-request").unwrap();
        fs::create_dir(directory.0.join("src")).unwrap();
        write(&directory.0.join("src/first.rinzu"), "fn main() {}");
        write(&directory.0.join("src/second.rinzu"), "fn main() {}");
        let manifest = directory.0.join("rinzu.toml");
        fs::write(
            &manifest,
            "[package]\nname=\"request\"\nlanguage=\"0.5\"\nentry=\"src/first.rinzu\"\n",
        )
        .unwrap();
        let context = crate::input::check(
            &directory.0,
            &crate::input::Options::default(),
            &rinzu_compiler::NativeCliInputs::default(),
        )
        .unwrap();
        let request = InputRequest::capture(&context);
        fs::write(
            &manifest,
            "[package]\nname=\"request\"\nlanguage=\"0.5\"\nentry=\"src/second.rinzu\"\n",
        )
        .unwrap();
        assert!(request.check().unwrap_err().contains("input changed"));
    }
}
