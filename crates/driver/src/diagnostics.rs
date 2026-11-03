//! Opt-in execution evidence. Never infer outcomes from program stdout/stderr.
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use rinzu_compiler::{NativeCliInputs, SourceCache, SourceSnapshot};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

mod analysis;
#[cfg(test)]
mod tests;
pub(crate) use analysis::analyze;

const SCHEMA: &str = "rinzu.diagnostic-report";
const REPORT_LIMIT: usize = 16 * 1024 * 1024;
const STREAM_LIMIT: usize = 8 * 1024 * 1024;
const ROW_LIMIT: usize = 4096;
const SOURCE_LIMIT: usize = 1024 * 1024;
const SOURCE_TOTAL: usize = 8 * SOURCE_LIMIT;
const SOURCE_FILES: usize = 256;
const RAW_LIMIT: usize = 65536;

thread_local! {
    static OBSERVATION: RefCell<Option<Value>> = const { RefCell::new(None) };
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CapturedSource {
    file: u64,
    parse_file: Option<u32>,
    path: String,
    disk_path: Option<PathBuf>,
    byte_length: Option<usize>,
    digest: Option<String>,
    text: Option<String>,
    omission: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InputReceipt {
    path: PathBuf,
    kind: String,
    byte_length: usize,
    digest: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Report {
    schema: String,
    version: u32,
    run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    development_nonce: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    process_memory: Option<Value>,
    package: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    input_mode: Option<rinzu_compiler::CompilationMode>,
    language: String,
    target: String,
    semantics: String,
    phase: String,
    process_id: u32,
    digest_algorithm: String,
    sources: Vec<CapturedSource>,
    inputs: Vec<InputReceipt>,
    resource_report: Option<PathBuf>,
    resource_producer: Option<String>,
    outcome: Option<Value>,
    driver_error: Option<String>,
    exit_code: Option<u8>,
    omitted_sources: usize,
    omitted_inputs: usize,
    limitations: Vec<String>,
}

pub(crate) fn is_active() -> bool {
    OBSERVATION.with(|value| value.borrow().is_some())
}

/// Own-process high-water memory only; no inspection of unrelated processes.
pub(crate) fn process_memory() -> Value {
    // SAFETY: getrusage initializes this POD for the current process only.
    let (success, usage) = unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        (libc::getrusage(libc::RUSAGE_SELF, &mut usage) == 0, usage)
    };
    json!({"producer":"getrusage-self","maxResidentRaw":success.then_some(usage.ru_maxrss),
        "unit":if cfg!(target_os="macos") {"bytes"} else {"KiB"},"kind":"process-high-water; not instantaneous RSS or a component ledger"})
}

/// Shared baseline/source observation is already independent of user output.
pub(crate) fn observe(value: &Value) {
    OBSERVATION.with(|slot| {
        if slot.borrow().is_some() {
            let mut value = value.clone();
            if let Some(cleanup) = value
                .get_mut("failure")
                .and_then(|failure| failure.get_mut("cleanup"))
                .and_then(Value::as_array_mut)
            {
                let omitted = cleanup.len().saturating_sub(ROW_LIMIT);
                cleanup.truncate(ROW_LIMIT);
                value["failure"]["omittedCleanup"] = json!(
                    omitted + value["failure"]["omittedCleanup"].as_u64().unwrap_or(0) as usize
                );
            }
            *slot.borrow_mut() = Some(value);
        }
    });
}

fn clipped(text: &str, limit: usize) -> String {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) fn observe_compiled(run: &crate::compiled_async::CompiledAsyncRun) {
    if !is_active() {
        return;
    }
    let line = |key: &str| run.output.lines().find_map(|line| line.strip_prefix(key));
    let cause = line("RINZU_FAILURE=");
    let packet = line("RINZU_DIAGNOSTICS=");
    let error_traces = line("RINZU_ERROR_TRACES=").and_then(|text| {
        (text.len() <= STREAM_LIMIT)
            .then(|| serde_json::from_str::<Value>(text).ok())
            .flatten()
    });
    let (failure, packet_error) = match packet.map(parse_packet).transpose() {
        Ok(failure) => (failure.flatten(), None),
        Err(error) => (None, Some(error)),
    };
    let trace = line("RINZU_TRACE=").and_then(|text| {
        (text.len() <= STREAM_LIMIT)
            .then(|| serde_json::from_str::<rinzu_trace::TraceDocument>(text).ok())
            .flatten()
    });
    let settled = trace.as_ref().is_some_and(|trace| {
        trace
            .context
            .get("resources")
            .is_some_and(|value| value == "baseline")
    }) && run.retired;
    observe(&json!({
        "engine":"compiled-jit",
        "status": if cause==Some("cancelled") {"cancelled"} else if run.exit_code==0 && cause==Some("none") {"pass"} else {"fail"},
        "settlement":if settled {"confirmed"} else {"unconfirmed"},
        "failure":failure,
        "errorTraces":error_traces,
        "packetError":packet_error,
        "runtimeCause":cause.map(|text| clipped(text, RAW_LIMIT)),
        "message":line("RINZU_FAILURE_MESSAGE=").map(|text| clipped(text,RAW_LIMIT)),
        "traceSummary":{
            "eventCount":trace.as_ref().map(|trace| trace.events.len()),
            "association":"same-private-runtime-buffer",
            "note":"Normal compiled run does not enable event history; empty is not evidence of no activity"
        },
        "privateReportPrefix":clipped(&run.output,RAW_LIMIT),
        "privateReportOmittedBytes":run.output.len().saturating_sub(RAW_LIMIT)
    }));
}

fn parse_position(text: &str) -> Result<Value, String> {
    let parts = text.split(':').collect::<Vec<_>>();
    if parts.len() != 3 {
        return Err("source position needs file:start:end".into());
    }
    let number = |part: &str| {
        part.parse::<u64>()
            .map_err(|_| "invalid source position".to_owned())
    };
    let (file, start, end) = (number(parts[0])?, number(parts[1])?, number(parts[2])?);
    if start > end || end > u64::from(u32::MAX) {
        return Err("invalid source range".into());
    }
    Ok(json!({"file":file,"startByte":start,"endByte":end}))
}

fn parse_packet(text: &str) -> Result<Option<Value>, String> {
    if text.len() > STREAM_LIMIT {
        return Err("private failure packet exceeds the capture budget".into());
    }
    use rinzu_runtime::failure::{ErrorKind as E, FailureCause as F};
    if text == "0" {
        return Ok(None);
    }
    let mut diagnostics = Vec::new();
    let mut omitted = 0;
    for item in text.split(',') {
        if diagnostics.len() == ROW_LIMIT {
            omitted += 1;
            continue;
        }
        let (code, source) = item
            .split_once('@')
            .ok_or("missing failure source marker")?;
        let cause = match code {
            "1" => F::ArithmeticTrap,
            "2" => F::Cancelled,
            "3" => F::Overloaded,
            "4" => F::Shutdown,
            "5" => F::Error(E::Conversion),
            "6" => F::Unsupported,
            "7" => F::RuntimePanic,
            "8" => F::Error(E::NullNativeValue),
            "9" => F::Error(E::InvalidUtf8),
            "10" => F::Error(E::NativeRecordBusy),
            "11" => F::Error(E::UnobservedChildFailure),
            "12" => F::Error(E::Io),
            "13" => F::Error(E::SupplantBusy),
            "14" => F::Error(E::UnboundValue),
            _ => return Err("unknown private failure cause".into()),
        };
        diagnostics.push(json!({
            "cause":format!("{cause:?}"), "message":cause.message(),
            "source":if source=="-" {Value::Null} else {parse_position(source)?}
        }));
    }
    if diagnostics.is_empty() {
        return Err("empty failure packet".into());
    }
    let primary = diagnostics.remove(0);
    Ok(Some(
        json!({"primary":primary,"cleanup":diagnostics,"omittedCleanup":omitted}),
    ))
}

fn identity(path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        path.canonicalize().map_err(|error| error.to_string())
    } else {
        crate::package_commands::output_identity(path)
    }
}

pub(crate) fn same_file(left: &Path, right: &Path) -> Result<bool, String> {
    if identity(left)? == identity(right)? {
        return Ok(true);
    }
    use std::os::unix::fs::MetadataExt;
    Ok(std::fs::metadata(left)
        .ok()
        .zip(std::fs::metadata(right).ok())
        .is_some_and(|(left, right)| left.dev() == right.dev() && left.ino() == right.ino()))
}

fn save(path: &Path, report: &Report) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(report).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    if bytes.len() > REPORT_LIMIT {
        return Err("diagnostic report exceeds 16 MiB".into());
    }
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = path.with_extension(format!("diagnostic-tmp-{}", report.run_id));
    use std::os::unix::fs::OpenOptionsExt;
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        file.write_all(&bytes).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        std::fs::rename(&temporary, path).map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

struct Capture;
impl Drop for Capture {
    fn drop(&mut self) {
        OBSERVATION.with(|slot| *slot.borrow_mut() = None);
    }
}

pub(crate) fn run_package(
    path: PathBuf,
    trace: bool,
    native: &NativeCliInputs,
    resources: Option<&rinzu_runtime::resource_report::Config>,
    output: PathBuf,
    options: &crate::input::Options,
) -> Result<(), String> {
    crate::PROGRAM_EXIT_STATUS.with(|status| status.set(0));
    let mut cache = SourceCache::default();
    let mut snapshot = SourceSnapshot::new(&mut cache, BTreeMap::new());
    let context = rinzu_compiler::check_input_with_sources(
        &path,
        &options.compiler()?,
        native,
        &mut snapshot,
    )
    .map_err(|errors| {
        crate::print_diagnostics(errors);
        "package failed to compile".to_owned()
    })?;
    crate::input::protect_outputs(
        &context,
        &std::iter::once(output.clone())
            .chain(resources.map(|config| config.path.clone()))
            .collect::<Vec<_>>(),
    )?;
    let input_mode = context.mode;
    let manifest = context.metadata;
    let artifact = context.artifact;
    let receipts = snapshot.input_receipts();
    let protected = receipts
        .iter()
        .map(|(path, _, _, _)| path.clone())
        .chain(std::iter::once(manifest.root.join("rinzu.lock")))
        .chain(std::iter::once(manifest.root.join("rinzu.api.json")))
        .chain(
            artifact
                .sdk_packages
                .iter()
                .map(|package| package.manifest.root.join("rinzu.lock")),
        )
        .chain(artifact.native_inputs.objects.iter().cloned())
        .chain(
            std::iter::once(&manifest)
                .chain(
                    artifact
                        .sdk_packages
                        .iter()
                        .map(|package| &package.manifest),
                )
                .flat_map(|manifest| {
                    manifest
                        .native
                        .sources
                        .iter()
                        .chain(&manifest.native.headers)
                        .chain(&manifest.native.objects)
                        .cloned()
                }),
        )
        .collect::<Vec<_>>();
    for destination in std::iter::once(&output).chain(resources.map(|config| &config.path)) {
        for input in &protected {
            if same_file(destination, input)? {
                return Err(format!(
                    "diagnostic/resource output collides with input {}",
                    input.display()
                ));
            }
        }
    }
    if let Some(resources) = resources
        && same_file(&output, &resources.path)?
    {
        return Err("diagnostic and resource outputs must be distinct".into());
    }
    let runtime_files = rinzu_codegen::resource_source_files(&artifact.ir);
    let mut sources = Vec::new();
    let (mut total, mut captured) = (0, 0);
    for (file, logical) in runtime_files.iter().take(ROW_LIMIT) {
        let parse_file = artifact
            .ir
            .source_files
            .iter()
            .find_map(|(id, path)| (path == logical).then_some(*id));
        let source = parse_file.and_then(|id| {
            snapshot
                .sources()
                .values()
                .find(|source| source.id().0 == id)
        });
        let mut row = CapturedSource {
            file: *file,
            parse_file,
            path: logical.clone(),
            disk_path: source.map(|source| source.path().to_owned()),
            byte_length: None,
            digest: None,
            text: None,
            omission: None,
        };
        if let Some(source) = source {
            let receipt = receipts
                .iter()
                .find(|(path, _, _, _)| path == source.path())
                .ok_or("missing captured source receipt")?;
            row.byte_length = Some(receipt.2);
            row.digest = Some(format!("{:016x}", receipt.3));
            if captured < SOURCE_FILES
                && receipt.2 <= SOURCE_LIMIT
                && total + receipt.2 <= SOURCE_TOTAL
            {
                row.text = Some(format!(
                    "{}{}",
                    if source.original_offset(0) == 3 {
                        "\u{feff}"
                    } else {
                        ""
                    },
                    source.text()
                ));
                captured += 1;
                total += receipt.2;
            } else {
                row.omission = Some("source capture budget exceeded; receipt only".into());
            }
        } else {
            row.omission = Some("synthetic or uncaptured source; no body identity".into());
        }
        sources.push(row);
    }
    use std::io::Read;
    let mut random = [0_u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut random))
        .map_err(|error| format!("create diagnostic run identity: {error}"))?;
    let run_id = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let mut report = Report {
        schema:SCHEMA.into(),version:1,run_id,
        development_nonce:std::env::var("RINZU_DEV_NONCE").ok().filter(|nonce| nonce.len()==32 && nonce.bytes().all(|byte|byte.is_ascii_hexdigit())),
        process_memory:None,
        package:manifest.name.clone(),input_mode:Some(input_mode),language:manifest.language.clone(),
        target:rinzu_runtime::current_target().into(),semantics:rinzu_compiler::LANGUAGE_SEMANTICS_REVISION.into(),
        phase:"prepared".into(),process_id:std::process::id(),digest_algorithm:"fnv1a64".into(),sources,
        inputs:receipts.iter().take(ROW_LIMIT).map(|(path,kind,length,digest)| InputReceipt{path:path.clone(),kind:(*kind).into(),byte_length:*length,digest:format!("{digest:016x}")}).collect(),
        resource_report:resources.map(|config| identity(&config.path)).transpose()?,
        resource_producer:resources.map(|_| if crate::aot::main_uses_source_async(&artifact) && !artifact.ir.source_native_signatures.is_empty() {"c-compiled-jit".into()} else {"rust-jit".into()}),
        outcome:None,driver_error:None,exit_code:None,
        omitted_sources:runtime_files.len().saturating_sub(ROW_LIMIT),omitted_inputs:receipts.len().saturating_sub(ROW_LIMIT),
        limitations:vec!["Unpublished logical task/scope/Owner/CodeVersion IDs remain unknown".into(),"Native activity logs have no run ID; full stack and SDK allocator attribution are unavailable".into(),"Fatal/signal/OOM can leave only this initial report; settlement then remains unconfirmed".into()]
    };
    save(&output, &report).map_err(|error| format!("save initial diagnostic report: {error}"))?;
    OBSERVATION.with(|slot| *slot.borrow_mut() = Some(Value::Null));
    let _capture = Capture;
    let outcome = crate::run_checked_package(&manifest, &artifact, trace, resources);
    report.phase = "finished".into();
    report.process_memory = Some(process_memory());
    report.outcome = OBSERVATION.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|value| !value.is_null())
            .cloned()
    });
    report.driver_error = outcome
        .as_ref()
        .err()
        .map(|error| clipped(error, RAW_LIMIT));
    report.exit_code = Some(if outcome.is_ok() {
        crate::PROGRAM_EXIT_STATUS.with(|status| status.get())
    } else {
        1
    });
    save(&output, &report).map_err(|error| format!("save final diagnostic report: {error}"))?;
    outcome
}
