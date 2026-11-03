//! Explicit test supervision; legacy `test` continues to execute package main.
use rinzu_compiler::{
    NativeCliInputs, PackageTestCase, PackageTestOptions, SourceCache, SourceSnapshot,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

const OUTPUT_LIMIT: usize = 1024 * 1024;
const REPORT_OUTPUT_LIMIT: usize = 4 * OUTPUT_LIMIT;
thread_local! {
    static SCRATCH: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    static EVIDENCE: RefCell<Option<Value>> = const { RefCell::new(None) };
}

pub(crate) fn generated_directory(manifest: &rinzu_compiler::Manifest) -> PathBuf {
    SCRATCH.with(|scratch| {
        scratch
            .borrow()
            .as_ref()
            .unwrap_or(&manifest.root)
            .join(".rinzu/generated/source-packages-v2")
    })
}

fn observe(value: impl FnOnce() -> Value) {
    if is_worker() || crate::diagnostics::is_active() {
        let value = value();
        crate::diagnostics::observe(&value);
        if is_worker() {
            EVIDENCE.with(|evidence| *evidence.borrow_mut() = Some(value));
        }
    }
}

pub(crate) fn is_worker() -> bool {
    SCRATCH.with(|scratch| scratch.borrow().is_some())
}

pub(crate) fn failure(failure: Option<&rinzu_runtime::failure::Failure>) -> Value {
    let diagnostic = |d: &rinzu_runtime::failure::Diagnostic| {
        json!({
            "cause": format!("{:?}", d.cause), "message": d.cause.message(),
            "source": d.source.map(|s| json!({"file":s.file,"startByte":s.start_byte,"endByte":s.end_byte}))
        })
    };
    failure.map_or(Value::Null, |f| {
        let limit = if crate::diagnostics::is_active() {4096} else {usize::MAX};
        let mut value = json!({"primary":diagnostic(&f.primary),"cleanup":f.cleanup.iter().take(limit).map(diagnostic).collect::<Vec<_>>()});
        if !f.traces.is_empty() {
            value["errorTraces"] = json!(f.traces.iter().take(limit).map(|(index,trace)| {
                json!({"diagnostic":index,"frames":trace.frames.iter().take(limit).map(|frame| {
                    json!({"function":frame.function.as_ref(),"source":frame.source.map(|source| {
                        json!({"file":source.file,"startByte":source.start_byte,"endByte":source.end_byte})
                    })})
                }).collect::<Vec<_>>(),"omittedFrames":trace.frames.len().saturating_sub(limit)})
            }).collect::<Vec<_>>());
        }
        if crate::diagnostics::is_active() {
            value["omittedCleanup"] = json!(f.cleanup.len().saturating_sub(limit));
        }
        value
    })
}

pub(crate) fn observe_source(run: &rinzu_codegen::SourceAsyncRun) {
    observe(|| {
        json!({
            "engine":"source-async", "status": if run.root_cancellation_requested || run.failure_details.as_ref().is_some_and(|f|f.primary.cause == rinzu_runtime::failure::FailureCause::Cancelled) {"cancelled"} else if run.exit_status == 0 && run.failure_details.is_none() {"pass"} else {"fail"},
            "settlement": if run.resources_returned && run.native_calls.live_calls == 0 && run.native_calls.foreign.live == 0 {"confirmed"} else {"unconfirmed"},
            "failure": failure(run.failure_details.as_ref()), "sourceFiles": run.source_files,
            "heapObjects":run.heap.objects,"heapRoots":run.heap.roots,"nativeCalls":run.native_calls.live_calls,
            "transport": {"domains":run.transport.worker_domains,"inFlight":run.transport.in_flight_for_origin,"bytes":run.transport.transport_bytes},
        })
    });
}

pub(crate) fn observe_baseline(run: &rinzu_codegen::BaselineRun) {
    observe(|| {
        json!({"engine":"baseline-jit", "status":if run.result.is_ok() {"pass"} else {"fail"},
        "settlement":if run.heap_objects == 0 && run.native_calls.live_calls == 0 && run.native_calls.foreign.live == 0 {"confirmed"} else {"unconfirmed"},
        "failure":failure(run.failure_details.as_ref()),"sourceFiles":run.source_files,
        "heapObjects":run.heap_objects,"nativeCalls":run.native_calls.live_calls,
        "codeVersions":{"published":run.published_code_versions,"retired":run.retired_code_versions,
            "peakLiveFrames":run.peak_live_code_frames,"dispatchResolutions":run.dispatch_resolutions,
            "callbackDispatches":run.callback_dispatches}})
    });
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) fn observe_compiled(run: &crate::compiled_async::CompiledAsyncRun) {
    // Diagnostic capture reads this private buffer independently with finite limits.
    if !is_worker() {
        return;
    }
    observe(|| {
        let line = |key: &str| run.output.lines().find_map(|line| line.strip_prefix(key));
        let trace = line("RINZU_TRACE=").and_then(|text| serde_json::from_str::<Value>(text).ok());
        let cause = line("RINZU_FAILURE=");
        json!({"engine":"compiled-jit", "status":if cause==Some("cancelled") {"cancelled"} else if run.exit_code==0 && cause==Some("none") {"pass"} else {"fail"},
            "settlement":if trace.as_ref().is_some_and(|t|t["context"]["resources"]=="baseline") && run.retired {"confirmed"} else {"unconfirmed"},
            "failure":if cause==Some("none") {Value::Null} else {json!({"cause":cause,"message":line("RINZU_FAILURE_MESSAGE="),"runtimeReport":run.output})},"trace":trace})
    });
}

struct Options {
    compiler: PackageTestOptions,
    native: NativeCliInputs,
    start: PathBuf,
    list: bool,
    json: bool,
    filter: Option<String>,
    timeout_ms: u64,
}

fn parse(arguments: &[String]) -> Result<Option<Options>, String> {
    let mut compiler = PackageTestOptions::default();
    let mut seen = BTreeSet::new();
    let mut remaining = Vec::new();
    let (mut list, mut json, mut filter, mut timeout_ms) = (false, false, None, 60000);
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        match argument.as_str() {
            "--unit" | "--list" => {
                if !seen.insert(argument.clone()) {
                    return Err(format!("duplicate test flag {argument}"));
                }
                if argument == "--unit" {
                    compiler.unit = true;
                } else {
                    list = true;
                }
            }
            "--doc" | "--filter" | "--format" | "--timeout-ms" => {
                if argument != "--doc" && !seen.insert(argument.clone()) {
                    return Err(format!("duplicate test option {argument}"));
                }
                index += 1;
                let value = arguments
                    .get(index)
                    .filter(|v| !v.starts_with('-') && !v.is_empty())
                    .ok_or_else(|| format!("{argument} needs a value"))?;
                match argument.as_str() {
                    "--doc" => compiler.documents.push(value.into()),
                    "--filter" => filter = Some(value.clone()),
                    "--format" => {
                        json = match value.as_str() {
                            "text" => false,
                            "json" => true,
                            _ => return Err("test format must be text or json".into()),
                        }
                    }
                    _ => {
                        timeout_ms = value.parse().map_err(|_| "invalid test timeout")?;
                        if !(1..=600000).contains(&timeout_ms) {
                            return Err("test timeout must be 1..600000 ms".into());
                        }
                    }
                }
            }
            _ => remaining.push(argument.clone()),
        }
        index += 1;
    }
    if !compiler.unit && compiler.documents.is_empty() {
        if !seen.is_empty() {
            return Err("test runner options require --unit or --doc FILE".into());
        }
        return Ok(None);
    }
    let parsed = crate::native_cli::parse("test", remaining.into_iter())?;
    if parsed.resource_report.is_some() {
        return Err("unit/doc tests use per-case resource reports; legacy --resource-report requires main mode".into());
    }
    Ok(Some(Options {
        compiler,
        native: parsed.native,
        start: parsed
            .path
            .unwrap_or(std::env::current_dir().map_err(|e| e.to_string())?),
        list,
        json,
        filter,
        timeout_ms,
    }))
}

fn location(range: rinzu_compiler::SourceRange, sources: &SourceSnapshot<'_>) -> Value {
    let path = sources.path_for_id(range.file_id);
    let position = path
        .and_then(|path| sources.sources().get(path))
        .and_then(|source| {
            // Semantic ranges are physical UTF-8 boundaries; manifest has no source.
            (range.start_byte as usize <= source.original_offset(source.text().len()))
                .then(|| source.line_column(range.start_byte as usize))
        });
    json!({"path":path,"startByte":range.start_byte,"endByte":range.end_byte,"line":position.map(|p|p.0),"column":position.map(|p|p.1)})
}

fn diagnostics(errors: &[rinzu_compiler::Diagnostic], sources: &SourceSnapshot<'_>) -> Vec<Value> {
    errors.iter().map(|d|json!({"code":d.code.as_str(),"message":d.message,"location":location(d.primary,sources),"related":d.secondary.iter().map(|(range,message)|json!({"location":location(*range,sources),"message":message})).collect::<Vec<_>>()})).collect()
}

fn case_metadata(case: &PackageTestCase, sources: &SourceSnapshot<'_>) -> Value {
    json!({"identity":case.identity,"kind":case.kind,"path":case.path,"isAsync":case.is_async,"location":location(case.range,sources),"status":"not-run","settlement":"not-run"})
}

fn print_report(report: &Value, json_output: bool) -> Result<(), String> {
    if json_output {
        println!(
            "{}",
            serde_json::to_string(report).map_err(|e| e.to_string())?
        );
    } else {
        if let Some(cases) = report["cases"].as_array() {
            for case in cases {
                println!(
                    "{} {} ({} ms) [{}:{}:{}]",
                    case["status"].as_str().unwrap_or("not-run"),
                    case["identity"].as_str().unwrap_or("?"),
                    case["elapsedMs"].as_u64().unwrap_or(0),
                    case["location"]["path"].as_str().unwrap_or("?"),
                    case["location"]["line"].as_u64().unwrap_or(0),
                    case["location"]["column"].as_u64().unwrap_or(0)
                );
                for key in ["stdoutBytes", "stderrBytes"] {
                    if let Some(bytes) = case[key].as_array() {
                        let bytes = bytes
                            .iter()
                            .filter_map(|b| b.as_u64().map(|b| b as u8))
                            .collect::<Vec<_>>();
                        print!("{}", String::from_utf8_lossy(&bytes));
                    }
                }
                if let Some(message) = case["worker"]["error"].as_str() {
                    eprintln!("{message}");
                }
            }
        }
        for error in report["diagnostics"].as_array().into_iter().flatten() {
            eprintln!("{error}");
        }
        println!(
            "tests: {} selected, {} passed, {} failed",
            report["summary"]["selected"], report["summary"]["passed"], report["summary"]["failed"]
        );
    }
    Ok(())
}

pub(crate) fn run(arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let arguments = arguments.collect::<Vec<_>>();
    let Some(options) = parse(&arguments)? else {
        let parsed = crate::native_cli::parse("test", arguments.into_iter())?;
        let path = parsed
            .path
            .unwrap_or(std::env::current_dir().map_err(|e| e.to_string())?);
        crate::input::require_package("test", &path)?;
        return crate::run_package_with_native_inputs(
            path,
            parsed.trace,
            &parsed.native,
            parsed.resource_report.as_ref(),
        );
    };
    crate::input::require_package("test", &options.start)?;
    let mut cache = SourceCache::default();
    let mut sources = SourceSnapshot::new(&mut cache, Default::default());
    let checked =
        rinzu_compiler::check_package_tests(&options.start, &mut sources, &options.compiler);
    let mut report = json!({"schema":"rinzu.test-report","version":1,"mode":if options.list {"list"} else {"run"},"target":rinzu_runtime::current_target(),"semantics":rinzu_compiler::LANGUAGE_SEMANTICS_REVISION,"cases":[],"diagnostics":[],"summary":{"selected":0,"passed":0,"failed":0,"notRun":0}});
    let (manifest, mut artifact, cases) = match checked {
        Ok(value) => value,
        Err(errors) => {
            report["diagnostics"] = json!(diagnostics(&errors, &sources));
            print_report(&report, options.json)?;
            crate::PROGRAM_EXIT_STATUS.with(|s| s.set(1));
            return Ok(());
        }
    };
    artifact.native_inputs = options
        .native
        .normalize(&manifest)
        .map_err(|errors| format!("{:?}", diagnostics(&errors, &sources)))?;
    let cases = cases
        .into_iter()
        .filter(|case| {
            options
                .filter
                .as_ref()
                .is_none_or(|f| case.identity.contains(f))
        })
        .collect::<Vec<_>>();
    report["package"] = json!(manifest.name);
    if cases.is_empty() {
        report["diagnostics"] = json!([{"message":"no matching test cases"}]);
        print_report(&report, options.json)?;
        crate::PROGRAM_EXIT_STATUS.with(|s| s.set(1));
        return Ok(());
    }
    report["summary"]["selected"] = json!(cases.len());
    let mut rows = Vec::new();
    let mut output_remaining = REPORT_OUTPUT_LIMIT;
    let _signals = Signals::install()?;
    for case in &cases {
        let mut row = case_metadata(case, &sources);
        if options.list || INTERRUPTED.load(Ordering::SeqCst) != 0 {
            rows.push(row);
            continue;
        }
        let scratch = Temporary::new()?;
        let specification = WorkerSpec {
            root: manifest.root.clone(),
            unit: options.compiler.unit,
            documents: options.compiler.documents.clone(),
            identity: case.identity.clone(),
            native_flags: native_flags(&options.native),
            receipts: sources
                .input_receipts()
                .into_iter()
                .map(|(p, k, n, h)| (p, k.to_owned(), n, h))
                .collect(),
        };
        let outcome = supervise(
            &scratch.0,
            &specification,
            options.timeout_ms,
            (output_remaining / 2).min(OUTPUT_LIMIT),
        )?;
        output_remaining -= outcome["stdoutBytes"].as_array().map_or(0, Vec::len)
            + outcome["stderrBytes"].as_array().map_or(0, Vec::len);
        for (key, value) in outcome.as_object().unwrap() {
            row[key] = value.clone();
        }
        rows.push(row);
    }
    let passed = rows.iter().filter(|r| r["status"] == "pass").count();
    let not_run = rows.iter().filter(|r| r["status"] == "not-run").count();
    let failed = rows.len() - passed - not_run;
    report["cases"] = json!(rows);
    report["outputByteBudget"] = json!(REPORT_OUTPUT_LIMIT);
    report["summary"] =
        json!({"selected":cases.len(),"passed":passed,"failed":failed,"notRun":not_run});
    print_report(&report, options.json)?;
    let interrupted = INTERRUPTED.load(Ordering::SeqCst);
    crate::PROGRAM_EXIT_STATUS.with(|s| {
        s.set(if interrupted != 0 {
            (128 + interrupted) as u8
        } else if options.list || passed == cases.len() {
            0
        } else {
            1
        })
    });
    Ok(())
}

fn native_flags(inputs: &NativeCliInputs) -> Vec<String> {
    let mut result = Vec::new();
    for path in &inputs.search_paths {
        result.extend(["-L".into(), path.display().to_string()]);
    }
    for library in &inputs.libraries {
        result.extend(["-l".into(), library.clone()]);
    }
    for path in &inputs.objects {
        result.extend(["--native-object".into(), path.display().to_string()]);
    }
    result
}

#[derive(Serialize, Deserialize)]
struct WorkerSpec {
    root: PathBuf,
    unit: bool,
    documents: Vec<PathBuf>,
    identity: String,
    native_flags: Vec<String>,
    receipts: Vec<(PathBuf, String, usize, u64)>,
}

pub(crate) fn worker(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let scratch = PathBuf::from(arguments.next().ok_or("test worker requires scratch")?);
    if arguments.next().is_some()
        || std::env::var_os("RINZU_TEST_TMP")
            .map(PathBuf::from)
            .as_ref()
            != Some(&scratch)
    {
        return Err("invalid test worker context".into());
    }
    let spec: WorkerSpec =
        serde_json::from_reader(std::io::stdin().take((2 * OUTPUT_LIMIT + 1) as u64))
            .map_err(|e| e.to_string())?;
    SCRATCH.with(|s| *s.borrow_mut() = Some(scratch.clone()));
    let mut cache = SourceCache::default();
    let mut sources = SourceSnapshot::new(&mut cache, Default::default());
    let options = PackageTestOptions {
        unit: spec.unit,
        documents: spec.documents,
        selected: Some(spec.identity.clone()),
    };
    let mut source_files = Value::Null;
    let mut source_paths = Value::Null;
    let outcome = (|| {
        let (manifest, mut artifact, _) =
            rinzu_compiler::check_package_tests(&spec.root, &mut sources, &options)
                .map_err(|e| format!("{}", json!(diagnostics(&e, &sources))))?;
        let mut labels = artifact.ir.source_files.iter().collect::<Vec<_>>();
        labels.sort_by(|a, b| a.1.cmp(b.1));
        source_files = json!(
            labels
                .iter()
                .enumerate()
                .map(|(i, (_, label))| ((i + 1).to_string(), (*label).clone()))
                .collect::<std::collections::BTreeMap<_, _>>()
        );
        source_paths = json!(
            labels
                .iter()
                .map(|(id, label)| (
                    (*label).clone(),
                    sources.path_for_id(rinzu_compiler::FileId(**id))
                ))
                .collect::<std::collections::BTreeMap<_, _>>()
        );
        let receipts = sources
            .input_receipts()
            .into_iter()
            .map(|(p, k, n, h)| (p, k.to_owned(), n, h))
            .collect::<Vec<_>>();
        if receipts != spec.receipts || !sources.disk_is_current() {
            return Err("test inputs changed after discovery".to_owned());
        }
        artifact.native_inputs = crate::native_cli::parse("test", spec.native_flags.into_iter())?
            .native
            .normalize(&manifest)
            .map_err(|e| format!("{e:?}"))?;
        crate::native_contract::check_artifact(&manifest, &artifact)?;
        let config = rinzu_runtime::resource_report::Config {
            path: scratch.join("resource.jsonl"),
            interval_ms: 0,
        };
        let compiled = artifact
            .checked
            .functions
            .iter()
            .find(|f| f.declaration.name == "main")
            .is_some_and(|f| f.declaration.is_async)
            && !artifact.ir.source_native_signatures.is_empty()
            && cfg!(all(target_os = "macos", target_arch = "aarch64"));
        let mut session = if compiled {
            None
        } else {
            Some(
                rinzu_runtime::resource_report::Session::start(
                    &config,
                    "test-worker",
                    labels
                        .iter()
                        .enumerate()
                        .map(|(i, (_, label))| ((i + 1) as u64, (*label).clone()))
                        .collect(),
                )
                .map_err(|e| e.to_string())?,
            )
        };
        let outcome =
            crate::run_package_engines(&manifest, &artifact, false, Some(&config), &mut session);
        if let Some(session) = session {
            session.finish().map_err(|e| e.to_string())?;
        }
        outcome
    })();
    let mut result = EVIDENCE
        .with(|e| e.borrow_mut().take())
        .unwrap_or_else(|| json!({"status":"protocol-error","settlement":"unconfirmed"}));
    result["identity"] = json!(spec.identity);
    result["sourceFiles"] = source_files;
    result["sourcePaths"] = source_paths;
    result["inputReceipts"] = json!(spec.receipts);
    if let Err(error) = outcome {
        result["error"] = json!(error);
        if result["status"] == "pass" {
            result["status"] = json!("fail");
        }
    }
    let report = scratch.join("resource.jsonl");
    if let Ok(bytes) = bounded_read(&report, OUTPUT_LIMIT) {
        match bytes
            .split(|b| *b == b'\n')
            .filter(|line| !line.is_empty())
            .map(serde_json::from_slice::<Value>)
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(rows) if rows.last().is_some_and(|r| r["phase"] == "end") => {
                result["resourceReport"] = json!(rows)
            }
            _ => {
                result["settlement"] = json!("unconfirmed");
                result["status"] = json!("protocol-error");
            }
        }
    } else {
        result["settlement"] = json!("unconfirmed");
    }
    let pass = result["status"] == "pass"
        && result["settlement"] == "confirmed"
        && crate::PROGRAM_EXIT_STATUS.with(|s| s.get()) == 0;
    if !pass && result["status"] == "pass" {
        result["status"] = json!("fail");
    }
    result["exitCode"] = json!(if pass { 0 } else { 1 });
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(scratch.join("result.json"))
        .map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut file, &result).map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())?;
    crate::PROGRAM_EXIT_STATUS.with(|s| s.set(if pass { 0 } else { 1 }));
    Ok(())
}

fn bounded_read(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err("test result exceeds its size limit".into());
    }
    Ok(bytes)
}

struct Temporary(PathBuf);
impl Temporary {
    fn new() -> Result<Self, String> {
        for attempt in 0..16 {
            let path = std::env::temp_dir().join(format!(
                "rinzu-case-{}-{}-{attempt}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| e.to_string())?
                    .as_nanos()
            ));
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => return Ok(Self(path.canonicalize().map_err(|e| e.to_string())?)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        Err("cannot allocate unique test scratch".into())
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

static INTERRUPTED: AtomicI32 = AtomicI32::new(0);
extern "C" fn signal_handler(signal: i32) {
    INTERRUPTED.store(signal, Ordering::SeqCst);
}
struct Signals {
    #[cfg(unix)]
    old: Vec<(i32, libc::sigaction)>,
}
impl Signals {
    fn install() -> Result<Self, String> {
        INTERRUPTED.store(0, Ordering::SeqCst);
        #[cfg(unix)]
        {
            let mut guard = Self { old: Vec::new() };
            for signal in [libc::SIGINT, libc::SIGTERM] {
                // SAFETY: sigaction is a C POD; initialize its mask and handler before use.
                let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
                action.sa_sigaction = signal_handler as *const () as usize;
                // SAFETY: the POD output buffer is initialized by successful sigaction.
                let mut old = unsafe { std::mem::zeroed() };
                // SAFETY: both buffers are live; the handler only stores an atomic signal.
                let installed = unsafe {
                    libc::sigemptyset(&mut action.sa_mask) == 0
                        && libc::sigaction(signal, &action, &mut old) == 0
                };
                if !installed {
                    return Err(std::io::Error::last_os_error().to_string());
                }
                guard.old.push((signal, old));
            }
            Ok(guard)
        }
        #[cfg(not(unix))]
        {
            Err("isolated package tests require a Unix process supervisor".into())
        }
    }
}
impl Drop for Signals {
    fn drop(&mut self) {
        #[cfg(unix)]
        for (signal, old) in &self.old {
            // SAFETY: restore the action returned for this signal; old remains live.
            unsafe {
                libc::sigaction(*signal, old, std::ptr::null_mut());
            }
        }
    }
}

fn kill_group(child: &mut std::process::Child) {
    #[cfg(unix)]
    // SAFETY: this Child was started with a fresh process group whose ID is its PID.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
}
struct Running(std::process::Child, bool);
impl Drop for Running {
    fn drop(&mut self) {
        if !self.1 {
            kill_group(&mut self.0);
            let _ = self.0.wait();
        }
    }
}

fn capture(mut reader: impl Read, limit: usize) -> Result<(Vec<u8>, u64), String> {
    let mut bytes = Vec::new();
    let mut total = 0_u64;
    let mut buffer = [0; 8192];
    loop {
        let n = reader.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        total = total
            .checked_add(n as u64)
            .ok_or("test output byte count overflow")?;
        let keep = n.min(limit.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&buffer[..keep]);
    }
    Ok((bytes, total))
}

fn supervise(
    scratch: &Path,
    spec: &WorkerSpec,
    timeout_ms: u64,
    capture_limit: usize,
) -> Result<Value, String> {
    let input = serde_json::to_vec(spec).map_err(|e| e.to_string())?;
    if input.len() > 2 * OUTPUT_LIMIT {
        return Err("test worker request exceeds size limit".into());
    }
    let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    command
        .arg("__test-worker")
        .arg(scratch)
        .env("RINZU_TEST_TMP", scratch)
        .env("TMPDIR", scratch)
        .current_dir(scratch)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let started = Instant::now();
    let mut child = Running(command.spawn().map_err(|e| e.to_string())?, false);
    let stdout = child.0.stdout.take().unwrap();
    let stderr = child.0.stderr.take().unwrap();
    let out = std::thread::spawn(move || capture(stdout, capture_limit));
    let err = std::thread::spawn(move || capture(stderr, capture_limit));
    {
        let mut stdin = child.0.stdin.take().unwrap();
        stdin.write_all(&input).map_err(|e| e.to_string())?;
    }
    let mut forced = None;
    let status = loop {
        if let Some(status) = child.0.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if INTERRUPTED.load(Ordering::SeqCst) != 0 {
            forced = Some("interrupted");
            kill_group(&mut child.0);
            break child.0.wait().map_err(|e| e.to_string())?;
        }
        if started.elapsed() >= Duration::from_millis(timeout_ms) {
            forced = Some("timeout");
            kill_group(&mut child.0);
            break child.0.wait().map_err(|e| e.to_string())?;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    // A fixture subprocess cannot retain the parent's pipes after worker return.
    kill_group(&mut child.0);
    child.1 = true;
    let (stdout, out_total) = out.join().map_err(|_| "test stdout capture panicked")??;
    let (stderr, err_total) = err.join().map_err(|_| "test stderr capture panicked")??;
    let worker = bounded_read(&scratch.join("result.json"), 4 * OUTPUT_LIMIT)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let valid = worker.as_ref().is_some_and(|w| {
        w["identity"] == spec.identity && w["exitCode"].as_i64() == status.code().map(i64::from)
    });
    let case_status = forced.unwrap_or_else(|| {
        if !valid {
            "protocol-error"
        } else {
            worker.as_ref().unwrap()["status"]
                .as_str()
                .unwrap_or("protocol-error")
        }
    });
    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        status.signal()
    };
    #[cfg(not(unix))]
    let signal: Option<i32> = None;
    Ok(
        json!({"status":case_status,"settlement":if forced.is_none() && valid {worker.as_ref().unwrap()["settlement"].clone()} else {json!("unconfirmed")},"exitCode":status.code(),"signal":signal,"elapsedMs":started.elapsed().as_millis(),"stdoutBytes":stdout,"stdoutTotal":out_total,"stdoutTruncated":out_total>stdout.len() as u64,"stderrBytes":stderr,"stderrTotal":err_total,"stderrTruncated":err_total>stderr.len() as u64,"worker":worker}),
    )
}
