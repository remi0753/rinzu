//! Supervision for test files and executable documentation examples.
use rinzu_compiler::{
    NativeCliInputs, PackageTestCase, PackageTestOptions, SourceCache, SourceSnapshot,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

use crate::cli_diagnostics;

mod case_log;
mod files;
mod worker_io;

const OUTPUT_LIMIT: usize = 1024 * 1024;
const REPORT_OUTPUT_LIMIT: usize = 4 * OUTPUT_LIMIT;
thread_local! {
    static SCRATCH: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    static EVIDENCE: RefCell<Option<Value>> = const { RefCell::new(None) };
    static FILE_WORKER: Cell<bool> = const { Cell::new(false) };
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

pub(crate) fn is_file_worker() -> bool {
    FILE_WORKER.get()
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
        json!({"engine":"baseline-jit", "status":if run.result.is_ok() && run.exit_status == 0 {"pass"} else {"fail"},
        "settlement":if run.heap_objects == 0 && run.native_calls.live_calls == 0 && run.native_calls.foreign.live == 0 {"confirmed"} else {"unconfirmed"},
        "failure":failure(run.failure_details.as_ref()),"sourceFiles":run.source_files,
        "heapObjects":run.heap_objects,"nativeCalls":run.native_calls.live_calls,
        "codeVersions":{"published":run.published_code_versions,"retired":run.retired_code_versions,
            "peakLiveFrames":run.peak_live_code_frames,"dispatchResolutions":run.dispatch_resolutions,
            "callbackDispatches":run.callback_dispatches}})
    });
}

#[cfg(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "x86_64", target_env = "gnu")
))]
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
    documents: Vec<PathBuf>,
    list: bool,
    json: bool,
    filter: Option<String>,
    case: Option<String>,
    timeout_ms: u64,
    has_runner_flags: bool,
}

impl Options {
    fn matches(&self, identity: &str) -> bool {
        self.filter
            .as_ref()
            .is_none_or(|filter| identity.contains(filter))
    }
}

fn parse(arguments: &[String]) -> Result<(crate::native_cli::PackageArguments, Options), String> {
    let mut documents = Vec::new();
    let mut seen = BTreeSet::new();
    let mut remaining = Vec::new();
    let (mut list, mut json, mut filter, mut timeout_ms) = (false, false, None, 60000);
    let mut case = None;
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        let (flag, inline_value) = argument
            .strip_prefix("--case=")
            .map_or((argument.as_str(), None), |value| ("--case", Some(value)));
        match flag {
            "--list" => {
                if !seen.insert(argument.clone()) {
                    return Err(format!("duplicate test flag {argument}"));
                }
                list = true;
            }
            "--doc" | "--filter" | "--format" | "--timeout-ms" | "--case" => {
                if flag != "--doc" && !seen.insert(flag.to_owned()) {
                    return Err(format!("duplicate test option {flag}"));
                }
                let value = match inline_value {
                    Some(value) => Some(value),
                    None => {
                        index += 1;
                        arguments
                            .get(index)
                            .map(String::as_str)
                            .filter(|value| !value.starts_with('-'))
                    }
                }
                .filter(|value| !value.is_empty())
                .ok_or_else(|| format!("{flag} needs a value"))?;
                match flag {
                    "--doc" => documents.push(value.into()),
                    "--filter" => filter = Some(value.to_owned()),
                    "--case" => case = Some(value.to_owned()),
                    "--format" => {
                        json = match value {
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
    let parsed = crate::native_cli::parse("test", remaining.into_iter())?;
    Ok((
        parsed,
        Options {
            documents,
            list,
            json,
            filter,
            case,
            timeout_ms,
            has_runner_flags: !seen.is_empty(),
        },
    ))
}

fn location(range: rinzu_compiler::SourceRange, sources: &SourceSnapshot<'_>) -> Value {
    let input = cli_diagnostics::snapshot_input(sources, range.file_id);
    let path = input.map(cli_diagnostics::Input::path);
    let position = input.and_then(|input| input.position(range.start_byte as usize));
    json!({"path":path,"startByte":range.start_byte,"endByte":range.end_byte,"line":position.map(|p|p.0),"column":position.map(|p|p.1)})
}

fn diagnostics(errors: &[rinzu_compiler::Diagnostic], sources: &SourceSnapshot<'_>) -> Vec<Value> {
    errors.iter().map(|d| {
        let mut primary = location(d.primary, sources);
        if let Some(path) = d.primary_path() {
            primary["path"] = json!(path);
        }
        json!({"code":d.code.as_str(),"message":d.message,"location":primary,
            "related":d.secondary.iter().map(|(range,message)|json!({"location":location(*range,sources),"message":message})).collect::<Vec<_>>()})
    }).collect()
}

fn case_metadata(case: &PackageTestCase, sources: &SourceSnapshot<'_>) -> Value {
    json!({"identity":case.identity,"kind":case.kind,"path":case.path,"isAsync":case.is_async,"location":location(case.range,sources),"status":"not-run","settlement":"not-run"})
}

enum Entry<'a> {
    Document(&'a PackageTestCase),
    File(&'a files::FileEntry),
}

impl Entry<'_> {
    fn metadata(&self, sources: &SourceSnapshot<'_>) -> Value {
        match self {
            Self::Document(case) => case_metadata(case, sources),
            Self::File(file) => json!({
                "identity":file.identity(),"kind":"file","path":file.path,
                "relativePath":file.relative,"location":null,"isAsync":null,
                "status":"not-run","settlement":"not-run","cases":[],
            }),
        }
    }
}

fn entries_key(report: &Value) -> &'static str {
    if report["version"] == 2 {
        "entries"
    } else {
        "cases"
    }
}

fn print_report(report: &Value, json_output: bool) -> Result<(), String> {
    if json_output {
        println!(
            "{}",
            serde_json::to_string(report).map_err(|e| e.to_string())?
        );
    } else {
        if let Some(cases) = report[entries_key(report)].as_array() {
            for case in cases {
                let status = case["status"].as_str().unwrap_or("not-run");
                let identity = case["identity"].as_str().unwrap_or("?");
                let elapsed = case["elapsedMs"].as_u64().unwrap_or(0);
                if let Some(path) = case["location"]["path"].as_str() {
                    println!(
                        "{status} {identity} ({elapsed} ms) [{path}:{}:{}]",
                        case["location"]["line"].as_u64().unwrap_or(0),
                        case["location"]["column"].as_u64().unwrap_or(0),
                    );
                } else {
                    println!("{status} {identity} ({elapsed} ms)");
                }
                if let Some(named) = case["cases"].as_array() {
                    for named in named {
                        println!(
                            "  {} {}",
                            named["status"].as_str().unwrap_or("?"),
                            named["name"].as_str().unwrap_or("?").escape_debug(),
                        );
                        if let Some(errors) = named["errors"].as_array() {
                            for error in errors {
                                eprintln!(
                                    "    {}: {}",
                                    error["name"].as_str().unwrap_or("Error").escape_debug(),
                                    error["message"].as_str().unwrap_or("").escape_debug(),
                                );
                            }
                        }
                    }
                }
                for key in ["stdoutBytes", "stderrBytes"] {
                    if let Some(bytes) = case[key].as_array() {
                        let bytes = bytes
                            .iter()
                            .filter_map(|b| b.as_u64().map(|b| b as u8))
                            .collect::<Vec<_>>();
                        if key == "stderrBytes" {
                            let _ = std::io::stderr().lock().write_all(&bytes);
                        } else {
                            let _ = std::io::stdout().lock().write_all(&bytes);
                        }
                    }
                }
                if let Some(message) = case["worker"]["error"].as_str() {
                    eprintln!("{message}");
                }
                if let Some(message) = case["error"].as_str() {
                    eprintln!("{message}");
                }
            }
        }
        println!(
            "tests: {} selected, {} passed, {} failed",
            report["summary"]["selected"], report["summary"]["passed"], report["summary"]["failed"]
        );
        if let Some(summary) = report.get("caseSummary") {
            println!(
                "cases: {} started, {} passed, {} failed",
                summary["selected"], summary["passed"], summary["failed"]
            );
        }
    }
    Ok(())
}

pub(crate) fn run(arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let arguments = arguments.collect::<Vec<_>>();
    let (parsed, options) = parse(&arguments)?;
    let files = parsed
        .path
        .as_deref()
        .map(files::Selection::from_path)
        .transpose()?
        .flatten();
    if files.is_none() && options.documents.is_empty() {
        if options.has_runner_flags {
            return Err("test runner options require a path under tests/ or --doc FILE".into());
        }
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
    }
    if parsed.resource_report.is_some() {
        return Err(
            "file/doc tests use individual resource reports; --resource-report requires main mode"
                .into(),
        );
    }
    if options.case.is_some()
        && (options.list
            || !options.documents.is_empty()
            || files
                .as_ref()
                .is_none_or(|selection| !selection.scope.is_file()))
    {
        return Err("--case requires a single .test.rinzu file without --list or --doc".into());
    }
    let start = if let Some(selection) = &files {
        selection.root.clone()
    } else {
        let path = parsed
            .path
            .unwrap_or(std::env::current_dir().map_err(|e| e.to_string())?);
        crate::input::require_package("test", &path)?;
        path
    };
    let file_entries = files
        .as_ref()
        .map(files::Selection::entries)
        .transpose()?
        .unwrap_or_default();
    let mut cache = SourceCache::default();
    let mut sources = SourceSnapshot::new(&mut cache, Default::default());
    let checked = if options.documents.is_empty() {
        rinzu_compiler::load_manifest_with_sources(&start.join("rinzu.toml"), &mut sources)
            .map(|manifest| (manifest, Vec::new()))
    } else {
        rinzu_compiler::check_package_tests(
            &start,
            &mut sources,
            &PackageTestOptions {
                documents: options.documents.clone(),
                selected: None,
            },
        )
        .map(|(manifest, _, cases)| (manifest, cases))
    }
    .and_then(|(manifest, cases)| {
        parsed.native.normalize(&manifest)?;
        Ok((manifest, cases))
    });
    let mut report = json!({"schema":"rinzu.test-report","version":if files.is_some() {2} else {1},"mode":if options.list {"list"} else {"run"},"target":rinzu_runtime::current_target(),"semantics":rinzu_compiler::LANGUAGE_SEMANTICS_REVISION,"diagnostics":[],"summary":{"selected":0,"passed":0,"failed":0,"notRun":0}});
    let entries_key = entries_key(&report);
    report[entries_key] = json!([]);
    if files.is_some() {
        report["caseSummary"] = json!({"selected":0,"passed":0,"failed":0});
    }
    let (manifest, cases) = match checked {
        Ok(value) => value,
        Err(errors) => {
            if options.json {
                report["diagnostics"] = json!(diagnostics(&errors, &sources));
            } else {
                cli_diagnostics::emit_snapshot(&errors, &sources);
            }
            print_report(&report, options.json)?;
            crate::PROGRAM_EXIT_STATUS.with(|s| s.set(1));
            return Ok(());
        }
    };
    let entries = cases
        .iter()
        .filter(|case| options.matches(&case.identity))
        .map(Entry::Document)
        .chain(
            file_entries
                .iter()
                .filter(|file| options.matches(&file.identity()))
                .map(Entry::File),
        )
        .collect::<Vec<_>>();
    report["package"] = json!(manifest.name);
    if entries.is_empty() {
        report["diagnostics"] = json!([{"message":"no matching test cases"}]);
        if !options.json {
            eprintln!("no matching test cases");
        }
        print_report(&report, options.json)?;
        crate::PROGRAM_EXIT_STATUS.with(|s| s.set(1));
        return Ok(());
    }
    let file_inputs = (!options.list && files.is_some())
        .then(|| files::Inputs::read(&manifest.root))
        .transpose()?;
    if file_inputs.is_some() && !sources.disk_is_current() {
        return Err("test package configuration changed after discovery".into());
    }
    let document_receipts = sources
        .input_receipts()
        .into_iter()
        .map(|(p, k, n, h)| (p, k.to_owned(), n, h))
        .collect::<Vec<_>>();
    let mut rows = Vec::new();
    let mut worker_diagnostics = Vec::new();
    let mut output_remaining = REPORT_OUTPUT_LIMIT;
    let _signals = Signals::install()?;
    let diagnostic_cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    for entry in &entries {
        let mut row = entry.metadata(&sources);
        if options.list || INTERRUPTED.load(Ordering::SeqCst) != 0 {
            rows.push(row);
            continue;
        }
        let scratch = Temporary::new()?;
        let input = match entry {
            Entry::Document(_) => WorkerInput::Documents {
                documents: options.documents.clone(),
                receipts: document_receipts.clone(),
            },
            Entry::File(file) => WorkerInput::File {
                path: file.path.clone(),
                inputs: file_inputs.as_ref().expect("file inputs").clone(),
                case: options.case.clone(),
            },
        };
        let specification = WorkerSpec {
            root: manifest.root.clone(),
            input,
            identity: row["identity"].as_str().expect("entry identity").to_owned(),
            native_flags: native_flags(&parsed.native),
            json: options.json,
            diagnostic_cwd: diagnostic_cwd.clone(),
        };
        let mut outcome = supervise(
            &scratch.0,
            &specification,
            options.timeout_ms,
            (output_remaining / 2).min(OUTPUT_LIMIT),
        )?;
        if let Some(worker) = outcome["worker"].as_object_mut() {
            if let Some(Value::Array(mut errors)) = worker.remove("diagnostics") {
                if files.is_some() {
                    for error in &mut errors {
                        error["entryIdentity"] = json!(specification.identity);
                    }
                }
                worker_diagnostics.extend(errors);
            }
            if let Some(Value::Array(blocks)) = worker.remove("diagnosticBlocks") {
                for block in blocks.iter().filter_map(Value::as_str) {
                    cli_diagnostics::write_block(block);
                }
            }
        }
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
    if files.is_some() {
        let mut named_selected = 0;
        let mut named_passed = 0;
        for case in rows
            .iter()
            .filter_map(|row| row["cases"].as_array())
            .flatten()
        {
            named_selected += 1;
            named_passed += usize::from(case["status"] == "pass");
        }
        report["caseSummary"] = json!({"selected":named_selected,"passed":named_passed,"failed":named_selected-named_passed});
    }
    report[entries_key] = json!(rows);
    report["diagnostics"] = json!(worker_diagnostics);
    report["outputByteBudget"] = json!(REPORT_OUTPUT_LIMIT);
    report["summary"] =
        json!({"selected":entries.len(),"passed":passed,"failed":failed,"notRun":not_run});
    print_report(&report, options.json)?;
    let interrupted = INTERRUPTED.load(Ordering::SeqCst);
    crate::PROGRAM_EXIT_STATUS.with(|s| {
        s.set(if interrupted != 0 {
            (128 + interrupted) as u8
        } else if options.list || passed == entries.len() {
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
    for path in &inputs.headers {
        result.extend(["--native-header".into(), path.display().to_string()]);
    }
    result
}

#[derive(Serialize, Deserialize)]
struct WorkerSpec {
    root: PathBuf,
    input: WorkerInput,
    identity: String,
    native_flags: Vec<String>,
    json: bool,
    diagnostic_cwd: PathBuf,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum WorkerInput {
    Documents {
        documents: Vec<PathBuf>,
        receipts: Vec<(PathBuf, String, usize, u64)>,
    },
    File {
        path: PathBuf,
        inputs: files::Inputs,
        case: Option<String>,
    },
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
    FILE_WORKER.set(matches!(&spec.input, WorkerInput::File { .. }));
    let mut cache = SourceCache::default();
    let mut sources = SourceSnapshot::new(&mut cache, Default::default());
    let mut source_files = Value::Null;
    let mut source_paths = Value::Null;
    let mut metadata = Value::Null;
    let mut compilation_errors = Vec::new();
    let mut preparing = true;
    let outcome = (|| {
        let checked = match &spec.input {
            WorkerInput::Documents { documents, .. } => {
                let options = PackageTestOptions {
                    documents: documents.clone(),
                    selected: Some(spec.identity.clone()),
                };
                rinzu_compiler::check_package_tests(&spec.root, &mut sources, &options)
                    .map(|(manifest, artifact, _)| (manifest, artifact))
            }
            WorkerInput::File { path, inputs, .. } => {
                if files::Inputs::read(&spec.root)? != *inputs {
                    return Err("test package configuration changed after discovery".into());
                }
                rinzu_compiler::check_package_test_file(&spec.root, path, &mut sources).map(
                    |(manifest, artifact, file)| {
                        metadata = case_metadata(&file, &sources);
                        (manifest, artifact)
                    },
                )
            }
        };
        let (manifest, mut artifact) = checked.map_err(|errors| {
            compilation_errors = errors;
            "input failed to check".to_owned()
        })?;
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
        let matches = match &spec.input {
            WorkerInput::Documents { receipts, .. } => sources
                .input_receipts()
                .into_iter()
                .map(|(p, k, n, h)| (p, k.to_owned(), n, h))
                .eq(receipts.iter().cloned()),
            WorkerInput::File { inputs, .. } => {
                metadata["identity"] == spec.identity
                    && files::Inputs::read(&manifest.root)? == *inputs
            }
        };
        if !matches || !sources.disk_is_current() {
            return Err("test inputs changed after discovery".to_owned());
        }
        artifact.native_inputs = crate::native_cli::parse("test", spec.native_flags.into_iter())?
            .native
            .normalize(&manifest)
            .map_err(|errors| {
                compilation_errors = errors;
                "input failed to check".to_owned()
            })?;
        crate::native_contract::check_artifact(&manifest, &artifact)?;
        let config = rinzu_runtime::resource_report::Config {
            path: scratch.join("resource.jsonl"),
            interval_ms: 0,
        };
        let compiled = crate::aot::uses_compiled_async(&artifact);
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
        let mut ready = || preparing = false;
        let outcome = crate::run_package_engines(
            &manifest,
            &artifact,
            false,
            Some(&config),
            &mut session,
            &[],
            crate::RunPreparation {
                on_ready: Some(&mut ready),
                ..Default::default()
            },
        );
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
    if let WorkerInput::Documents { receipts, .. } = &spec.input {
        result["inputReceipts"] = json!(receipts);
    } else {
        result["location"] = metadata["location"].clone();
        result["isAsync"] = metadata["isAsync"].clone();
    }
    if let Err(error) = outcome {
        if compilation_errors.is_empty() {
            result["error"] = json!(error);
        } else if spec.json {
            result["diagnostics"] = json!(diagnostics(&compilation_errors, &sources));
        } else {
            let renderer = cli_diagnostics::Renderer::at(spec.diagnostic_cwd);
            result["diagnosticBlocks"] = json!(
                compilation_errors
                    .iter()
                    .map(|error| {
                        renderer.render(error, |id| cli_diagnostics::snapshot_input(&sources, id))
                    })
                    .collect::<Vec<_>>()
            );
        }
        if is_file_worker() && preparing {
            result["status"] = json!("compile-error");
            result["settlement"] = json!("not-run");
        } else if result["status"] == "pass" {
            result["status"] = json!("fail");
        }
    }
    let report = scratch.join("resource.jsonl");
    if result["status"] == "compile-error" {
        // The file did not start, so it has no runtime settlement to confirm.
    } else if let Ok(bytes) = bounded_read(&report, OUTPUT_LIMIT) {
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
    if let WorkerInput::File { case, .. } = &spec.input
        && result["status"] == "pass"
    {
        let log = case_log::CaseLog::read(&scratch);
        if log.problem.is_some() || log.has_active_case() {
            result["status"] = json!("protocol-error");
            result["error"] = json!(
                log.problem
                    .unwrap_or_else(|| "named case did not finish".into())
            );
        } else if log.failed() {
            result["status"] = json!("fail");
            if let Some(message) = log.misuse {
                result["error"] = json!(message);
            }
        } else if let Some(name) = case
            && !log.cases.iter().any(|case| &case.name == name)
        {
            result["status"] = json!("fail");
            result["error"] = json!(format!("no named test case matching {name:?}"));
        }
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

fn open_report(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("test report must be a regular file"));
    }
    Ok(file)
}

fn bounded_read(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    open_report(path)
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

fn valid_worker_result(worker: &Value, identity: &str, exit_code: Option<i32>) -> bool {
    if worker["identity"] != identity || worker["exitCode"].as_i64() != exit_code.map(i64::from) {
        return false;
    }
    let settlement = worker["settlement"].as_str();
    if settlement == Some("confirmed")
        && !worker["resourceReport"]
            .as_array()
            .and_then(|rows| rows.last())
            .is_some_and(|row| row["phase"] == "end")
    {
        return false;
    }
    matches!(
        (exit_code, worker["status"].as_str(), settlement),
        (Some(0), Some("pass"), Some("confirmed"))
            | (Some(1), Some("compile-error"), Some("not-run"))
            | (
                Some(1),
                Some("fail" | "cancelled" | "protocol-error"),
                Some("confirmed" | "unconfirmed")
            )
    )
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
        .env_remove(case_log::PATH_ENV)
        .env_remove(case_log::FILTER_ENV)
        .env_remove(case_log::SUPERVISOR_ENV)
        .current_dir(scratch)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let WorkerInput::File { case, .. } = &spec.input {
        let path = scratch.join(case_log::FILE);
        std::fs::File::create_new(&path).map_err(|error| error.to_string())?;
        command
            .env(case_log::PATH_ENV, path)
            .env(case_log::SUPERVISOR_ENV, std::process::id().to_string());
        if let Some(name) = case {
            command.env(case_log::FILTER_ENV, name);
        }
    }
    let started = Instant::now();
    let worker_io::WorkerExit {
        status,
        forced,
        stdout: (stdout, out_total),
        stderr: (stderr, err_total),
    } = worker_io::wait(
        &mut command,
        &input,
        started,
        Duration::from_millis(timeout_ms),
        capture_limit,
    )
    .map_err(|e| e.to_string())?;
    let worker = bounded_read(&scratch.join("result.json"), 4 * OUTPUT_LIMIT)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let valid = worker
        .as_ref()
        .is_some_and(|w| valid_worker_result(w, &spec.identity, status.code()));
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
    let mut outcome = json!({"status":case_status,"settlement":if forced.is_none() && valid {worker.as_ref().unwrap()["settlement"].clone()} else {json!("unconfirmed")},"exitCode":status.code(),"signal":signal,"elapsedMs":started.elapsed().as_millis(),"stdoutBytes":stdout,"stdoutTotal":out_total,"stdoutTruncated":out_total>stdout.len() as u64,"stderrBytes":stderr,"stderrTotal":err_total,"stderrTruncated":err_total>stderr.len() as u64,"worker":worker});
    if let WorkerInput::File { case, .. } = &spec.input {
        let mut log = case_log::CaseLog::read(scratch);
        if valid {
            outcome["location"] = outcome["worker"]["location"].clone();
            outcome["isAsync"] = outcome["worker"]["isAsync"].clone();
        }
        let problem = if let Some(problem) = log.problem.as_deref() {
            Some(problem)
        } else if case_status == "pass" && (log.has_active_case() || log.failed()) {
            Some("named case outcomes disagree with the file result")
        } else if let Some(wanted) = case
            && log.cases.iter().any(|case| case.name != *wanted)
        {
            Some("unexpected case in a filtered run")
        } else if case_status == "pass" && case.is_some() && log.cases.is_empty() {
            Some("filtered run passed without a named case result")
        } else {
            None
        };
        let mut named_status = case_status;
        if forced.is_none()
            && let Some(problem) = problem
        {
            named_status = "protocol-error";
            outcome["status"] = json!(named_status);
            outcome["settlement"] = json!("unconfirmed");
            outcome["error"] = json!(problem);
        }
        log.finish_active(named_status);
        outcome["cases"] = json!(log.cases);
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_result_must_match_its_exit_and_settlement_evidence() {
        let mut report = json!({
            "identity": "file:tests/sample.test.rinzu",
            "exitCode": 0,
            "status": "pass",
            "settlement": "confirmed",
            "resourceReport": [{"phase": "start"}, {"phase": "end"}]
        });
        let identity = "file:tests/sample.test.rinzu";
        assert!(valid_worker_result(&report, identity, Some(0)));
        assert!(!valid_worker_result(&report, identity, None));
        report["exitCode"] = json!(1);
        assert!(!valid_worker_result(&report, identity, Some(1)));
        report["status"] = json!("compile-error");
        assert!(!valid_worker_result(&report, identity, Some(1)));
        report["settlement"] = json!("not-run");
        assert!(valid_worker_result(&report, identity, Some(1)));
        report["status"] = json!("timeout");
        assert!(!valid_worker_result(&report, identity, Some(1)));

        report["status"] = json!("pass");
        report["settlement"] = json!("confirmed");
        report["exitCode"] = json!(0);
        report["resourceReport"] = json!([{"phase": "start"}]);
        assert!(!valid_worker_result(&report, identity, Some(0)));
    }

    #[cfg(unix)]
    #[test]
    fn report_reader_rejects_a_fifo_without_waiting_for_a_writer() {
        use std::os::unix::ffi::OsStrExt;

        let scratch = Temporary::new().unwrap();
        let path = scratch.0.join(case_log::FILE);
        let raw = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: raw names a new FIFO in this test's scratch directory.
        assert_eq!(unsafe { libc::mkfifo(raw.as_ptr(), 0o600) }, 0);
        assert!(bounded_read(&path, OUTPUT_LIMIT).is_err());
        assert!(case_log::CaseLog::read(&scratch.0).problem.is_some());
    }
}
