use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use rinzu_codegen::{
    AotFunctionMetadata, BaselineAotObject, BaselineFailure, BaselineOptions, LoadedNativePackages,
    emit_baseline_aot_object_with_native_bindings, emit_source_async_aot_object,
    run_baseline_jit_with_arguments,
};
use rinzu_trace::TraceDocument;
use serde_json::json;

pub(crate) mod native_quality;
#[cfg(any(all(test, unix), all(target_os = "macos", target_arch = "aarch64")))]
mod poll_fixture;

#[derive(Clone, Debug)]
struct AsyncAotWorker {
    index: usize,
    symbol: String,
    parameters: Vec<usize>,
    result: u32,
    result_type: usize,
}

#[derive(Clone, Debug)]
struct AsyncAotCallee {
    index: usize,
    symbol: String,
    managed_parameters: Vec<bool>,
    rooted_parameters: Vec<bool>,
    parameter_types: Vec<usize>,
    result: u32,
    result_type: usize,
    source: rinzu_runtime::failure::SourcePosition,
}

#[derive(Clone, Debug)]
pub(crate) struct AsyncAotPlan {
    workers: Vec<AsyncAotWorker>,
    callees: Vec<AsyncAotCallee>,
    file_read_keys: Vec<u64>,
    source_file_read: bool,
    source_timer: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct AotBuild {
    pub executable: PathBuf,
    pub object: PathBuf,
    pub metadata: PathBuf,
}

pub struct EquivalenceReport {
    pub result: i64,
    pub outcome: &'static str,
}

pub fn verify_equivalence(
    package: &Path,
    cancel_after_polls: Option<usize>,
) -> Result<EquivalenceReport, String> {
    verify_equivalence_with_options(
        package,
        BaselineOptions {
            cancel_after_polls,
            ..BaselineOptions::default()
        },
    )
}

pub fn verify_equivalence_with_options(
    package: &Path,
    options: BaselineOptions,
) -> Result<EquivalenceReport, String> {
    let context = crate::input::check(
        package,
        &crate::input::Options::default(),
        &rinzu_compiler::NativeCliInputs::default(),
    )?;
    verify_checked_input(&context, options, &[])
}

pub(crate) fn verify_checked_input(
    context: &rinzu_compiler::CompilationContext,
    options: BaselineOptions,
    program_arguments: &[std::ffi::OsString],
) -> Result<EquivalenceReport, String> {
    let manifest = &context.metadata;
    let artifact = &context.artifact;
    if manifest.language != "0.1" && main_uses_source_async(artifact) {
        if options.max_allocations.is_some() {
            return Err(
                "async JIT/AOT equivalence does not accept terminal-control options".into(),
            );
        }
        return verify_source_async_equivalence(
            context,
            options.cancel_after_polls,
            None,
            options.shutdown_after_polls,
            program_arguments,
        );
    }
    let options = BaselineOptions {
        capture_stdout: !crate::verify_capture::active(),
        ..options
    };
    let has_native_artifacts = artifact
        .sdk_packages
        .iter()
        .flat_map(|package| &package.native_imports)
        .any(|import| import.runtime_service.is_none());
    let has_source_packages = artifact.source_interface.is_some()
        || artifact
            .sdk_packages
            .iter()
            .any(|package| package.source.is_some());
    let arguments = crate::argument_bytes(program_arguments);
    let jit = if has_native_artifacts || has_source_packages || !artifact.native_inputs.is_empty() {
        let built = crate::input::build_native(context)?;
        let loaded = LoadedNativePackages::load(&built.artifacts)
            .map_err(|error| format!("load JIT native packages: {error:#}"))?;
        let result = run_baseline_jit_with_arguments(
            &artifact.ir,
            "main",
            options,
            Some(loaded),
            Some(built.load_source_packages()?),
            arguments,
        )
        .map_err(|error| format!("execute JIT equivalence side: {error:#}"));
        let _ = fs::remove_dir_all(built.directory);
        result?
    } else {
        run_baseline_jit_with_arguments(&artifact.ir, "main", options, None, None, arguments)
            .map_err(|error| format!("execute JIT equivalence side: {error:#}"))?
    };
    let directory = crate::verify_capture::TemporaryDirectory::new("equivalence")?;
    let executable = directory.0.join("program");
    let build = crate::verify_capture::build_aot(context, executable)?;
    let metadata: serde_json::Value = serde_json::from_slice(
        &fs::read(&build.metadata).map_err(|error| format!("read AOT source metadata: {error}"))?,
    )
    .map_err(|error| format!("decode AOT source metadata: {error}"))?;
    if metadata.get("sourceFiles")
        != Some(
            &serde_json::to_value(&jit.source_files)
                .map_err(|error| format!("encode JIT source identities: {error}"))?,
        )
    {
        return Err("JIT/AOT diagnostic source file identities differ".into());
    }
    let mut command = Command::new(&build.executable);
    command.args(program_arguments);
    crate::verify_capture::configure_aot(&mut command);
    if options.capture_error_trace {
        command.env("RINZU_ERROR_TRACE", "1");
    }
    if let Some(limit) = options.cancel_after_polls {
        command.env("RINZU_CANCEL_AFTER_POLLS", limit.to_string());
    }
    if let Some(limit) = options.max_allocations {
        command.env("RINZU_MAX_ALLOCATIONS", limit.to_string());
    }
    if let Some(limit) = options.shutdown_after_polls {
        command.env("RINZU_SHUTDOWN_AFTER_POLLS", limit.to_string());
    }
    let output = command
        .output()
        .map_err(|error| format!("execute AOT equivalence side: {error}"))?;
    let stdout = String::from_utf8(crate::verify_capture::evidence(&output)?)
        .map_err(|error| format!("AOT output is not UTF-8: {error}"))?;
    if !crate::verify_capture::active() && output.stdout != jit.stdout {
        return Err("JIT/AOT program stdout bytes differ".into());
    }
    compare_native_call_evidence(jit.native_calls, &stdout)?;
    compare_userdata_evidence(jit.native_userdata, &stdout)?;
    compare_userdata_process(&stdout)?;
    let result = parse_i64_line(&stdout, "RINZU_RESULT=")?;
    let aot_failure = parse_line(&stdout, "RINZU_FAILURE=")?;
    let diagnostics = parse_line(&stdout, "RINZU_DIAGNOSTICS=")?;
    let caught = parse_line(&stdout, "RINZU_CAUGHT_ERRORS=")?;
    let expected_diagnostics = diagnostic_signature(jit.failure_details.as_ref());
    let expected_caught = jit
        .caught_errors
        .iter()
        .map(|failure| diagnostic_signature(Some(failure)))
        .collect::<Vec<_>>()
        .join(";");
    if diagnostics != expected_diagnostics || caught != expected_caught {
        return Err(format!(
            "JIT/AOT diagnostic mismatch: JIT={expected_diagnostics}/{expected_caught}, AOT={diagnostics}/{caught}"
        ));
    }

    let trace_json = parse_line(&stdout, "RINZU_TRACE=")?;
    let aot_trace = TraceDocument::from_reader(trace_json.as_bytes())
        .map_err(|error| format!("AOT normalized trace is invalid: {error}"))?;
    let jit_trace = crate::normalized_trace(&jit);
    let jit_failure = failure_name(&jit.result);
    let expected_status_success =
        jit.result.is_ok() || jit.result == Err(BaselineFailure::Shutdown);
    if output.status.code() != Some(i32::from(!expected_status_success)) {
        return Err(format!(
            "JIT/AOT exit mismatch: JIT success={expected_status_success}, AOT status={}",
            output.status
        ));
    }
    let jit_result = jit.result.as_ref().copied().unwrap_or(0);
    if result != jit_result || aot_failure != jit_failure {
        return Err(format!(
            "JIT/AOT result mismatch: JIT result={jit_result}, failure={jit_failure}; AOT result={result}, failure={aot_failure}"
        ));
    }
    if jit_trace.semantic() != aot_trace.semantic() {
        return Err("JIT/AOT normalized semantic traces differ".into());
    }
    for key in ["resume-domain", "resource-baseline", "shutdown"] {
        if jit_trace.context.get(key) != aot_trace.context.get(key) {
            return Err(format!("JIT/AOT context differs for `{key}`"));
        }
    }
    Ok(EquivalenceReport {
        result,
        outcome: match jit.result {
            Ok(_) => "success",
            Err(BaselineFailure::Cancelled) => "cancelled",
            Err(BaselineFailure::Overloaded) => "overloaded",
            Err(BaselineFailure::Shutdown) => "shutdown",
            Err(_) => "failure",
        },
    })
}

pub(crate) fn verify_worker_cancel_checked(
    context: &rinzu_compiler::CompilationContext,
    after_polls: usize,
    program_arguments: &[std::ffi::OsString],
) -> Result<EquivalenceReport, String> {
    let manifest = &context.metadata;
    let artifact = &context.artifact;
    if manifest.language == "0.1" || !main_uses_source_async(artifact) {
        return Err("Worker cancellation equivalence requires a v0.2 source async main".into());
    }
    verify_source_async_equivalence(context, None, Some(after_polls), None, program_arguments)
}

fn verify_source_async_equivalence(
    context: &rinzu_compiler::CompilationContext,
    cancel_after_polls: Option<usize>,
    cancel_worker_after_polls: Option<usize>,
    shutdown_after_polls: Option<usize>,
    program_arguments: &[std::ffi::OsString],
) -> Result<EquivalenceReport, String> {
    let manifest = &context.metadata;
    let artifact = &context.artifact;
    let has_native_artifacts = artifact
        .sdk_packages
        .iter()
        .flat_map(|package| &package.native_imports)
        .any(|import| import.runtime_service.is_none());
    let has_source_packages = artifact.source_interface.is_some()
        || artifact
            .sdk_packages
            .iter()
            .any(|package| package.source.is_some());
    let mut options = rinzu_codegen::SourceAsyncOptions::from_sdk_packages(
        &manifest.root,
        &artifact.sdk_packages,
    )
    .map_err(|error| format!("configure async JIT equivalence side: {error:#}"))?;
    options.cancel_after_polls = cancel_after_polls;
    options.shutdown_after_polls = shutdown_after_polls;
    options.cancel_worker_after_polls = cancel_worker_after_polls;
    options.record_transport_evidence = true;
    options.capture_stdout = !crate::verify_capture::active();
    options.arguments = crate::argument_bytes(program_arguments);
    let built = if has_native_artifacts || has_source_packages || !artifact.native_inputs.is_empty()
    {
        let built = crate::input::build_native(context)?;
        options.native_packages = Some(
            LoadedNativePackages::load(&built.artifacts)
                .map_err(|error| format!("load async JIT native packages: {error:#}"))?,
        );
        options.source_packages = Some(built.load_source_packages()?);
        Some(built)
    } else {
        None
    };
    let jit = rinzu_codegen::run_source_async_report(&artifact.ir, "main", options)
        .map_err(|error| format!("execute async JIT equivalence side: {error:#}"));
    if let Some(built) = built {
        let _ = fs::remove_dir_all(built.directory);
    }
    let jit = jit?;
    let directory = crate::verify_capture::TemporaryDirectory::new("async-equivalence")?;
    let build = crate::verify_capture::build_aot(context, directory.0.join("program"))?;
    let mut command = Command::new(&build.executable);
    command.args(program_arguments);
    crate::verify_capture::configure_aot(&mut command);
    command
        .env_remove("RINZU_CANCEL_AFTER_POLLS")
        .env_remove("RINZU_SHUTDOWN_AFTER_POLLS")
        .env_remove("RINZU_SHUTDOWN_WORKER_AFTER_POLLS")
        .env_remove("RINZU_CANCEL_WORKER_AFTER_POLLS");
    command.env("RINZU_TRANSPORT_EVIDENCE", "1");
    if let Some(limit) = cancel_after_polls {
        command.env("RINZU_CANCEL_AFTER_POLLS", limit.to_string());
    }
    if let Some(limit) = shutdown_after_polls {
        command.env("RINZU_SHUTDOWN_AFTER_POLLS", limit.to_string());
    }
    if let Some(limit) = cancel_worker_after_polls {
        command.env("RINZU_CANCEL_WORKER_AFTER_POLLS", limit.to_string());
    }
    let output = command
        .output()
        .map_err(|error| format!("execute async AOT equivalence side: {error}"))?;
    let stdout = String::from_utf8(crate::verify_capture::evidence(&output)?)
        .map_err(|error| format!("async AOT output is not UTF-8: {error}"))?;
    if !crate::verify_capture::active() && output.stdout != jit.stdout {
        return Err("async JIT/AOT program stdout bytes differ".into());
    }
    let stdout = crate::future_identity::normalize(&jit, &stdout)?;
    let result = parse_i64_line(&stdout, "RINZU_RESULT=")?;
    let failure = parse_line(&stdout, "RINZU_FAILURE=")?;
    let trace_json = parse_line(&stdout, "RINZU_TRACE=")?;
    let aot_trace = TraceDocument::from_reader(trace_json.as_bytes())
        .map_err(|error| format!("async AOT normalized trace is invalid: {error}"))?;
    let jit_trace = crate::normalized_source_async_trace(&jit);
    compare_source_async_failures(&jit, &stdout)?;
    if output.status.code() != Some(i32::from(jit.exit_status)) || result != jit.result {
        return Err(format!(
            "async JIT/AOT result mismatch: JIT result={}, AOT result={result}, failure={failure}, status={}",
            jit.result, output.status
        ));
    }
    let metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(&build.metadata).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    if metadata["sourceFiles"]
        != serde_json::to_value(&jit.source_files).map_err(|error| error.to_string())?
    {
        return Err("async JIT/AOT diagnostic source identities differ".into());
    }
    compare_source_async_values_and_transport(&jit, &stdout)?;
    compare_async_task_traces(&jit_trace, &aot_trace)?;
    for key in ["resume-domain", "resources"] {
        if jit_trace.context.get(key) != aot_trace.context.get(key) {
            return Err(format!("async JIT/AOT context differs for `{key}`"));
        }
    }
    Ok(EquivalenceReport {
        result,
        outcome: match jit
            .failure_details
            .as_ref()
            .map(|failure| failure.primary.cause)
        {
            None => "success",
            Some(rinzu_runtime::failure::FailureCause::Cancelled) => "cancelled",
            Some(rinzu_runtime::failure::FailureCause::Shutdown) => "shutdown",
            Some(_) => "failure",
        },
    })
}

pub(crate) fn compare_source_async_failures(
    jit: &rinzu_codegen::SourceAsyncRun,
    stdout: &str,
) -> Result<(), String> {
    let expected_failure =
        jit.failure_details
            .as_ref()
            .map_or("none", |failure| match failure.primary.cause {
                rinzu_runtime::failure::FailureCause::ArithmeticTrap => "arithmetic",
                rinzu_runtime::failure::FailureCause::Error(kind) => error_name(kind),
                rinzu_runtime::failure::FailureCause::Cancelled => "cancelled",
                rinzu_runtime::failure::FailureCause::Shutdown => "shutdown",
                rinzu_runtime::failure::FailureCause::Overloaded => "overloaded",
                rinzu_runtime::failure::FailureCause::Panic => "panic",
                _ => "runtime-panic",
            });
    let actual_failure = parse_line(stdout, "RINZU_FAILURE=")?;
    if actual_failure != expected_failure {
        return Err(format!(
            "async JIT/AOT failure differs: JIT={expected_failure}, AOT={actual_failure}"
        ));
    }
    let diagnostics = parse_line(stdout, "RINZU_DIAGNOSTICS=")?;
    let caught = parse_line(stdout, "RINZU_CAUGHT_ERRORS=")?;
    let expected_diagnostics = diagnostic_signature(jit.failure_details.as_ref());
    let expected_caught = jit
        .caught_errors
        .iter()
        .map(|failure| diagnostic_signature(Some(failure)))
        .collect::<Vec<_>>()
        .join(";");
    if diagnostics != expected_diagnostics || caught != expected_caught {
        return Err(format!(
            "async JIT/AOT diagnostics differ: JIT={expected_diagnostics}/{expected_caught}, AOT={diagnostics}/{caught}"
        ));
    }
    Ok(())
}

fn compare_native_call_evidence(
    jit: rinzu_codegen::SourceNativeCallEvidence,
    stdout: &str,
) -> Result<(), String> {
    if jit.prepared == 0
        && !stdout
            .lines()
            .any(|line| line.starts_with("RINZU_NATIVE_CALLS="))
    {
        return Ok(());
    }
    let actual = parse_resource_numbers(parse_line(stdout, "RINZU_NATIVE_CALLS=")?)?;
    let expected = [
        jit.prepared,
        jit.returned,
        jit.closed,
        jit.settled,
        jit.read,
        jit.released,
        jit.callback_entries,
        jit.copied_bytes,
        jit.live_calls,
        jit.live_buffers,
        jit.live_callbacks,
    ];
    let abandoned = parse_resource_numbers(parse_line(stdout, "RINZU_NATIVE_ABANDONED=")?)?;
    if jit.foreign.created != 0
        || stdout
            .lines()
            .any(|line| line.starts_with("RINZU_FOREIGN="))
    {
        let actual = parse_resource_numbers(parse_line(stdout, "RINZU_FOREIGN=")?)?;
        let foreign = jit.foreign;
        let expected = [
            foreign.created as usize,
            foreign.published as usize,
            foreign.received as usize,
            foreign.retired as usize,
            foreign.live,
            foreign.bytes,
            foreign.queued,
            foreign.wake_pins,
        ];
        if actual != expected || !foreign.is_baseline() {
            return Err(format!(
                "JIT/AOT foreign invocation ledger differs: JIT={expected:?}, AOT={actual:?}"
            ));
        }
    }
    if actual != expected || abandoned != [jit.abandoned] || !jit.is_baseline() {
        return Err(format!(
            "JIT/AOT native call ledger differs: JIT={expected:?}, AOT={actual:?}, abandoned JIT={}, AOT={abandoned:?}",
            jit.abandoned
        ));
    }
    let allocation_line = if stdout
        .lines()
        .any(|line| line.starts_with("RINZU_CALLBACK_ALLOCATIONS="))
    {
        "RINZU_CALLBACK_ALLOCATIONS="
    } else {
        "RINZU_CALLBACK_RETURNS="
    };
    let actual = parse_resource_numbers(parse_line(stdout, allocation_line)?)?;
    let returns = jit.callback_returns;
    let expected = [
        returns.normal,
        returns.fallback,
        returns.bytes,
        returns.metadata_bytes,
    ];
    if actual != expected {
        return Err(format!(
            "JIT/AOT callback return allocations differ: JIT={expected:?}, AOT={actual:?}"
        ));
    }
    if let Some(line) = stdout
        .lines()
        .find_map(|line| line.strip_prefix("RINZU_CALLBACK_SCOPES="))
    {
        let scopes = parse_resource_numbers(line)?;
        let process = parse_resource_numbers(parse_line(stdout, "RINZU_CALLBACK_RETURNS=")?)?;
        if scopes.len() != 11
            || process.len() != 4
            || scopes[6] != 0
            || scopes[9] != 0
            || scopes[10] != 0
            || scopes[1].checked_add(scopes[2]) != Some(scopes[0])
            || scopes[4].checked_add(scopes[5]) != Some(scopes[3])
            || process[0]
                .checked_add(process[1])
                .and_then(|n| n.checked_add(scopes[1]))
                != returns.normal.checked_add(returns.fallback)
            || process[2].checked_add(scopes[4]) != Some(returns.bytes)
        {
            return Err(format!(
                "JIT/AOT scoped settlement differs: scoped={scopes:?}, retained={process:?}"
            ));
        }
    }
    validate_callback_resource_evidence(stdout)?;
    Ok(())
}

fn validate_callback_resource_evidence(stdout: &str) -> Result<(), String> {
    let pool = parse_resource_numbers(parse_line(stdout, "RINZU_NATIVE_ENTRIES=")?)?;
    let process = parse_resource_numbers(parse_line(stdout, "RINZU_CALLBACK_PROCESS=")?)?;
    let temporary = parse_resource_numbers(parse_line(stdout, "RINZU_CALLBACK_TEMPORARY=")?)?;
    let retired = stdout
        .lines()
        .find_map(|line| line.strip_prefix("RINZU_CALLBACK_RETIREMENT="))
        .map(parse_resource_numbers)
        .transpose()?
        .unwrap_or_else(|| vec![0, 0, 0]);
    let retirement_valid = retired.len() == 3
        && pool.len() == 6
        && process.len() == 3
        && retired[0] <= pool[0]
        && retired[0].checked_mul(32) == Some(retired[1])
        && if retired[0] == 0 {
            retired[2] == 0 && (pool[0] == 0 || process[1] > 0)
        } else {
            retired[2] > 0
                && retired[2] % retired[0] == 0
                && (retired[2] / retired[0]).checked_mul(pool[0] - retired[0]) == Some(process[1])
        };
    let valid = pool.len() == 6
        && process.len() == 3
        && temporary == [0, 0]
        && retirement_valid
        && pool[4..] == [0, 0]
        && pool[0] == process[0]
        && if pool[0] == 0 {
            pool[..4] == [0; 4] && process == [0; 3]
        } else {
            pool[1] == pool[0].div_ceil(512)
                && pool[1].checked_mul(32768) == Some(pool[2])
                && pool[1]
                    .checked_mul(16384)
                    .and_then(|pages| pool[3].checked_sub(pages))
                    == Some(process[1])
                && process[2] > 0
        };
    if !valid {
        return Err(format!(
            "callback resource lifetimes differ: pool={pool:?}, process={process:?}, temporary={temporary:?}, retired={retired:?}"
        ));
    }
    Ok(())
}

// Physical suspension depends on when independent completions are received.
// Every explicit await records its ready observation, even for an already ready
// Future. Compare that complete task/Future/native order; retain await-pending
// in the diagnostic trace as timing evidence, separately tested by runtime gates.
pub(crate) fn compare_async_task_traces(
    expected: &TraceDocument,
    actual: &TraceDocument,
) -> Result<(), String> {
    let group = |trace: &TraceDocument| {
        let mut tasks: BTreeMap<Option<String>, Vec<rinzu_trace::SemanticEvent>> = BTreeMap::new();
        for mut event in trace.semantic().events {
            if matches!(event.event, rinzu_trace::EventKind::AwaitPending { .. }) {
                continue;
            }
            let task = tasks.entry(event.task.clone()).or_default();
            event.sequence = task.len() as u64;
            task.push(event);
        }
        tasks
    };
    let expected = group(expected);
    let actual = group(actual);
    for task in expected.keys().chain(actual.keys()) {
        let before = expected.get(task).map(Vec::as_slice).unwrap_or_default();
        let after = actual.get(task).map(Vec::as_slice).unwrap_or_default();
        if before != after {
            let index = (0..before.len().max(after.len()))
                .find(|index| before.get(*index) != after.get(*index))
                .unwrap();
            return Err(format!(
                "async JIT/AOT normalized task semantic traces differ: task={task:?}, event={index}, JIT={:?}, AOT={:?}, lengths={},{}",
                before.get(index),
                after.get(index),
                before.len(),
                after.len()
            ));
        }
    }
    Ok(())
}

fn compare_awaited_values(jit: &rinzu_codegen::SourceAsyncRun, stdout: &str) -> Result<(), String> {
    let raw = parse_line(stdout, "RINZU_AWAITED_VALUES=")?;
    let labeled = parse_line(stdout, "RINZU_AWAITED_TASK_VALUES=")?;
    let mut actual = BTreeMap::<u64, Vec<(u64, String)>>::new();
    let mut chronological = Vec::new();
    for row in labeled.split(',').filter(|row| !row.is_empty()) {
        let fields = row.split(':').collect::<Vec<_>>();
        if fields.len() != 4 {
            return Err("malformed task await evidence".into());
        }
        let scope = fields[0].parse::<u64>().map_err(|_| "invalid await task")?;
        let future = fields[1]
            .parse::<u64>()
            .map_err(|_| "invalid awaited Future")?;
        if scope.to_string() != fields[0] || future.to_string() != fields[1] {
            return Err("noncanonical await identity".into());
        }
        let value = format!("{}:{}", fields[2], fields[3]);
        chronological.push(value.clone());
        actual.entry(scope).or_default().push((future, value));
    }
    let expected = jit
        .awaited_tasks
        .iter()
        .map(|(scope, values)| {
            (
                *scope,
                values
                    .iter()
                    .map(|(future, value)| {
                        (
                            *future,
                            format!("{}:{:016x}", value.kind() as u32, value.bits()),
                        )
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut source_values = jit
        .awaited_scalars
        .iter()
        .map(|value| format!("{}:{:016x}", value.kind() as u32, value.bits()))
        .collect::<Vec<_>>();
    let mut source_labeled = expected
        .values()
        .flatten()
        .map(|(_, value)| value.clone())
        .collect::<Vec<_>>();
    source_values.sort();
    source_labeled.sort();
    if raw != chronological.join(",") || actual != expected || source_values != source_labeled {
        return Err(format!(
            "async JIT/AOT task/Future/type/bits/await order differ: JIT={expected:?}, AOT={actual:?}"
        ));
    }
    Ok(())
}

fn compare_cancelled_children(
    jit: &rinzu_codegen::SourceAsyncRun,
    actual: &std::collections::BTreeSet<u64>,
) -> Result<std::collections::BTreeSet<u64>, String> {
    use rinzu_codegen::SourceAsyncEvent;
    use rinzu_runtime::failure::FailureCause;
    let expected = jit.cancelled_children.iter().copied().collect();
    let ordinary_failure = jit.failure_details.as_ref().is_some_and(|failure| {
        matches!(
            failure.primary.cause,
            FailureCause::ArithmeticTrap | FailureCause::Panic | FailureCause::Error(_)
        )
    });
    let admission_failure = jit
        .failure_details
        .as_ref()
        .is_some_and(|failure| failure.primary.cause == FailureCause::Overloaded);
    let root_cancelled = jit.root_cancellation_requested
        && jit.failure_details.as_ref().is_some_and(|failure| {
            matches!(
                failure.primary.cause,
                FailureCause::Cancelled | FailureCause::Shutdown
            )
        });
    let observed_await = |future: &u64| {
        jit.events.iter().any(|event| {
            matches!(event, SourceAsyncEvent::Awaited { future: id, .. } if id == future)
                || (!root_cancelled
                    && matches!(event, SourceAsyncEvent::Suspended { future: id, .. } if id == future))
        })
    };
    // An actual Main cancellation interrupts a pending await without receiving
    // its value. Successful await observations and direct child control remain
    // exact, as do the parent outcome, cleanup errors and per-task trace.
    // A parent failure can race physical file completion before the owner
    // receives that child. Only this unobserved, implicit scope-close race
    // may have different terminal winners on two independent executions.
    let unobserved_file_close = |future: &u64| {
        (ordinary_failure || root_cancelled)
            && jit.scope_cancellation_requests.contains(future)
            && !jit.control_attempts.contains(future)
            && jit.events.iter().any(|event| {
                matches!(event, SourceAsyncEvent::Submitted { future: id, operation }
                    if id == future && operation == "file-read-size")
            })
            && !observed_await(future)
    };
    // Admission failure or actual Main cancellation closes the parent without
    // awaiting independent Workers. A received successful return can precede close
    // on one execution, while implicit cancellation wins on the other.
    // Explicit observations/control, ordinary Worker errors and unknown IDs
    // remain exact; packet/heap/native/physical-return checks still follow.
    let unobserved_worker_close = |future: &u64| {
        (admission_failure || root_cancelled)
            && !jit.scope_cancellation_requests.is_empty()
            && (jit.scope_cancellation_requests.contains(future)
                || jit.events.iter().any(|event| {
                    matches!(event, SourceAsyncEvent::WorkerCompleted { future: id, .. }
                        if id == future)
                }))
            && !jit.control_attempts.contains(future)
            && jit.events.iter().any(|event| {
                matches!(event, SourceAsyncEvent::Submitted { future: id, operation }
                    if id == future && operation == "spawn")
            })
            && !observed_await(future)
    };
    let races = actual
        .symmetric_difference(&expected)
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    // A closed native-completion service returns Unit only. Under an already
    // cancelled task, completion can beat the recursive cancellation request.
    // Admit that physical race only inside the same cancelled subtree: aliases
    // observed/control-tested outside it remain exact, as do every value,
    // failure, cleanup, admission tree, await trace and resource proof.
    let below = |mut scope: u64, ancestor: u64| {
        for _ in 0..=jit.future_admissions.len() {
            if ancestor.checked_add(1) == Some(scope) {
                return true;
            }
            let Some(parent) = scope
                .checked_sub(1)
                .and_then(|id| jit.future_admissions.iter().find(|a| a.future == id))
            else {
                return false;
            };
            scope = parent.parent_scope;
        }
        false
    };
    let internal_native_close = |future: &u64| {
        jit.resources_returned && jit.scope_cancellation_requests.contains(future)
            && !jit.control_attempts.contains(future)
            && jit.events.iter().any(|event|matches!(event,SourceAsyncEvent::Submitted {future:id,operation} if id==future && operation=="native-completion"))
            && jit.cancelled_children.iter().any(|ancestor| {
                actual.contains(ancestor) && jit.control_attempts.contains(ancestor)
                    && jit.future_admissions.iter().any(|a|a.future==*future && below(a.parent_scope,*ancestor))
                    && jit.events.iter().all(|event|match event {
                        SourceAsyncEvent::Awaited {future:id,scope} | SourceAsyncEvent::Suspended {future:id,scope,..} if id==future=>below(*scope,*ancestor),
                        _=>true,
                    })
            })
    };
    if races.iter().any(|future| {
        !unobserved_file_close(future)
            && !unobserved_worker_close(future)
            && !internal_native_close(future)
    }) {
        return Err(format!(
            "async JIT/AOT child cancellation outcomes differ: JIT={expected:?}, AOT={actual:?}; Main cancel={}, scope requests={:?}, explicit control={:?}, differing child events={:?}",
            jit.root_cancellation_requested,
            jit.scope_cancellation_requests,
            jit.control_attempts,
            jit.events
                .iter()
                .filter(|event| match event {
                    SourceAsyncEvent::Submitted { future, .. }
                    | SourceAsyncEvent::Awaited { future, .. }
                    | SourceAsyncEvent::Suspended { future, .. }
                    | SourceAsyncEvent::WorkerCompleted { future, .. } => races.contains(future),
                    _ => false,
                })
                .collect::<Vec<_>>()
        ));
    }
    Ok(races)
}

pub(crate) fn compare_source_async_values_and_transport(
    jit: &rinzu_codegen::SourceAsyncRun,
    stdout: &str,
) -> Result<(), String> {
    compare_native_call_evidence(jit.native_calls, stdout)?;
    compare_userdata_evidence(jit.heap.userdata, stdout)?;
    compare_awaited_values(jit, stdout)?;
    let transport = parse_line(stdout, "RINZU_TRANSPORT=")?;
    let snapshot = jit.transport;
    let expected = [
        snapshot.worker_domains,
        snapshot.peak_worker_domains,
        snapshot.in_flight_for_origin,
        snapshot.peak_in_flight_per_origin,
        snapshot.transport_bytes,
        snapshot.peak_transport_bytes,
    ];
    let actual = parse_resource_numbers(transport)?;
    if actual.len() != 6 || actual[..5] != expected[..5] || !jit.resources_returned {
        return Err(format!(
            "async JIT/AOT transport resources differ: JIT={expected:?}, AOT={transport}, JIT baseline={}",
            jit.resources_returned
        ));
    }
    let aot_credits = parse_transport_credits(parse_line(stdout, "RINZU_CREDITS=")?)?;
    let jit_packets = validate_transport_credits(&jit.transport_evidence, expected[5])?;
    let aot_packets = validate_transport_credits(&aot_credits, actual[5])?;
    let mut cancelled = parse_resource_numbers(parse_line(stdout, "RINZU_CANCELLED_CHILDREN=")?)?
        .into_iter()
        .map(|value| value as u64)
        .collect::<std::collections::BTreeSet<_>>();
    // Only validated terminal races add a JIT-side cancellation to the AOT set.
    // Either direction still proves identical inputs, physical returns and full
    // release of every actual packet/heap/native resource.
    cancelled.extend(compare_cancelled_children(jit, &cancelled)?);
    compare_source_worker_heaps(jit, stdout, &cancelled)?;
    compare_worker_userdata_evidence(jit, stdout, &cancelled)?;
    compare_userdata_process(stdout)?;
    if jit_packets.keys().ne(aot_packets.keys()) {
        return Err("async JIT/AOT admitted transport operations differ".into());
    }
    for (operation, (input, results)) in &jit_packets {
        let (aot_input, aot_results) = &aot_packets[operation];
        // A cancelled job can be dropped before user-body entry, or reach a
        // poll and return a real failure packet. Both paths must still prove
        // every actual reservation and release. Other children stay exact.
        if input != aot_input || (!cancelled.contains(operation) && results != aot_results) {
            return Err(format!(
                "async JIT/AOT transport packet widths differ for child {operation}"
            ));
        }
    }
    let started = parse_resource_numbers(parse_line(stdout, "RINZU_STARTED_CHILDREN=")?)?;
    let submitted = ["spawn", "file-read-size"].map(|kind| jit.events.iter().filter(|event| {
        matches!(event, rinzu_codegen::SourceAsyncEvent::Submitted { operation, .. } if operation == kind)
    }).count());
    if started != submitted {
        return Err("async JIT/AOT actual child starts differ".into());
    }
    let submitted_timers = jit.events.iter().filter(|event| {
        matches!(event, rinzu_codegen::SourceAsyncEvent::Submitted { operation, .. } if operation == "timer")
    }).count();
    if parse_resource_numbers(parse_line(stdout, "RINZU_STARTED_TIMERS=")?)? != [submitted_timers] {
        return Err("async JIT/AOT actual timer starts differ".into());
    }
    let submitted_completions = jit.events.iter().filter(|event| {
        matches!(event, rinzu_codegen::SourceAsyncEvent::Submitted { operation, .. } if matches!(operation.as_str(), "native-completion" | "native-transfer"))
    }).count();
    if stdout
        .lines()
        .any(|line| line.starts_with("RINZU_NATIVE_COMPLETIONS="))
    {
        if parse_resource_numbers(parse_line(stdout, "RINZU_NATIVE_COMPLETIONS=")?)?
            != [submitted_completions]
        {
            return Err("async JIT/AOT native completion starts differ".into());
        }
    } else if submitted_completions != 0 {
        return Err("async AOT native completion evidence is missing".into());
    }
    Ok(())
}

fn userdata_numbers_match(
    expected: rinzu_runtime::data::NativeUserDataEvidence,
    actual: &[usize],
    cancelled: bool,
) -> bool {
    expected.is_baseline()
        && actual.len() == 6
        && actual[..4] == [0; 4]
        && actual[4] == actual[5]
        && (cancelled || (actual[4] == expected.created && actual[5] == expected.released))
}

fn compare_userdata_evidence(
    expected: rinzu_runtime::data::NativeUserDataEvidence,
    stdout: &str,
) -> Result<(), String> {
    if expected.is_baseline()
        && expected.created == 0
        && !stdout
            .lines()
            .any(|line| line.starts_with("RINZU_NATIVE_USERDATA="))
    {
        return Ok(());
    }
    let actual = parse_resource_numbers(parse_line(stdout, "RINZU_NATIVE_USERDATA=")?)?;
    if !userdata_numbers_match(expected, &actual, false) {
        return Err(format!(
            "JIT/AOT Main userdata differs: JIT={expected:?}, AOT={actual:?}"
        ));
    }
    Ok(())
}

fn compare_worker_userdata_evidence(
    jit: &rinzu_codegen::SourceAsyncRun,
    stdout: &str,
    cancelled: &std::collections::BTreeSet<u64>,
) -> Result<(), String> {
    if jit
        .worker_heaps
        .iter()
        .all(|worker| worker.heap.userdata.is_baseline() && worker.heap.userdata.created == 0)
        && !stdout
            .lines()
            .any(|line| line.starts_with("RINZU_WORKER_USERDATA="))
    {
        return Ok(());
    }
    let expected = jit
        .events
        .iter()
        .filter_map(|event| match event {
            rinzu_codegen::SourceAsyncEvent::WorkerCompleted { future, .. } => Some(*future),
            _ => None,
        })
        .zip(&jit.worker_heaps)
        .map(|(future, worker)| (future, worker.heap.userdata))
        .collect::<std::collections::BTreeMap<_, _>>();
    let heap_ids = parse_line(stdout, "RINZU_WORKER_HEAPS=")?
        .split(';')
        .filter(|record| !record.is_empty())
        .map(|record| {
            record
                .split(':')
                .next()
                .unwrap()
                .parse::<u64>()
                .map_err(|_| "invalid Worker heap identity".to_owned())
        })
        .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
    let mut seen = std::collections::BTreeSet::new();
    for record in parse_line(stdout, "RINZU_WORKER_USERDATA=")?
        .split(';')
        .filter(|record| !record.is_empty())
    {
        let fields = record
            .split(':')
            .map(|value| {
                value
                    .parse::<usize>()
                    .map_err(|_| "invalid Worker userdata evidence".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        if fields.len() != 7 {
            return Err("invalid Worker userdata field count".into());
        }
        let future = fields[0] as u64;
        if !seen.insert(future) || !heap_ids.contains(&future) {
            return Err("duplicate or unknown Worker userdata identity".into());
        }
        let expected = expected.get(&future).copied().unwrap_or_default();
        if !userdata_numbers_match(expected, &fields[1..], cancelled.contains(&future)) {
            return Err(format!(
                "JIT/AOT Worker {future} userdata differs: JIT={expected:?}, AOT={fields:?}"
            ));
        }
    }
    if seen != heap_ids {
        return Err("missing Worker userdata evidence".into());
    }
    Ok(())
}

fn compare_userdata_process(stdout: &str) -> Result<(), String> {
    let mut created = 0usize;
    if stdout
        .lines()
        .any(|line| line.starts_with("RINZU_NATIVE_USERDATA="))
    {
        let owner = parse_resource_numbers(parse_line(stdout, "RINZU_NATIVE_USERDATA=")?)?;
        if owner.len() != 6 {
            return Err("invalid Main userdata field count".into());
        }
        created = owner[4];
    }
    if stdout
        .lines()
        .any(|line| line.starts_with("RINZU_WORKER_USERDATA="))
    {
        for record in parse_line(stdout, "RINZU_WORKER_USERDATA=")?
            .split(';')
            .filter(|record| !record.is_empty())
        {
            let owner = record
                .split(':')
                .map(|value| {
                    value
                        .parse::<usize>()
                        .map_err(|_| "invalid Worker userdata evidence".to_owned())
                })
                .collect::<Result<Vec<_>, _>>()?;
            if owner.len() != 7 {
                return Err("invalid Worker userdata field count".into());
            }
            created = created
                .checked_add(owner[5])
                .ok_or("userdata process count overflow")?;
        }
    }
    if created == 0
        && !stdout
            .lines()
            .any(|line| line.starts_with("RINZU_USERDATA_PROCESS="))
    {
        return Ok(());
    }
    let actual = parse_resource_numbers(parse_line(stdout, "RINZU_USERDATA_PROCESS=")?)?;
    // A completed call leaves neither tokens nor closed identity metadata.
    if actual != [0, 0, 0] {
        return Err(format!(
            "AOT process userdata metadata differs: created={created}, process={actual:?}"
        ));
    }
    Ok(())
}

fn compare_source_worker_heaps(
    jit: &rinzu_codegen::SourceAsyncRun,
    stdout: &str,
    cancelled: &std::collections::BTreeSet<u64>,
) -> Result<(), String> {
    let completed = jit
        .events
        .iter()
        .filter_map(|event| match event {
            rinzu_codegen::SourceAsyncEvent::WorkerCompleted { future, domain, .. } => {
                Some((*future, *domain))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if completed.len() != jit.worker_heaps.len() {
        return Err("JIT Worker completion/heap evidence counts differ".into());
    }
    let mut expected = std::collections::BTreeMap::new();
    for ((future, domain), worker) in completed.into_iter().zip(&jit.worker_heaps) {
        if domain != worker.domain
            || worker.heap.objects != 0
            || worker.heap.roots != 0
            || expected.insert(future, worker).is_some()
        {
            return Err("JIT Worker heap ownership or baseline differs".into());
        }
    }
    let mut actual = std::collections::BTreeMap::new();
    let mut seen = std::collections::BTreeSet::new();
    let admitted = jit
        .transport_evidence
        .iter()
        .filter_map(|credit| {
            (credit.direction == rinzu_runtime::transport::TransportDirection::Input)
                .then_some(credit.operation)
                .flatten()
        })
        .collect::<std::collections::BTreeSet<_>>();
    let encoded = parse_line(stdout, "RINZU_WORKER_HEAPS=")?;
    for record in encoded.split(';').filter(|record| !record.is_empty()) {
        let fields = record
            .split(':')
            .map(|field| {
                field
                    .parse::<usize>()
                    .map_err(|_| "invalid AOT Worker heap counter".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        if fields.len() != 7
            || fields[1] != 0
            || fields[2] != 0
            || fields[3] == 0
            || fields[5] < 1
            || fields[6] > 1
            || (fields[6] == 1 && fields[5] < 2)
            || !seen.insert(fields[0] as u64)
            || !admitted.contains(&(fields[0] as u64))
        {
            return Err(
                "AOT Worker heap evidence has live resources or no prepared/body frames".into(),
            );
        }
        if fields[6] == 1 {
            actual.insert(fields[0] as u64, fields[4]);
        }
    }
    for future in expected.keys().chain(actual.keys()) {
        if !cancelled.contains(future)
            && expected.contains_key(future) != actual.contains_key(future)
        {
            return Err("JIT/AOT executed Worker heap evidence differs".into());
        }
    }
    for (future, worker) in expected {
        if worker.heap.peak_objects != 0 && actual.get(&future).is_some_and(|peak| *peak == 0) {
            return Err("AOT Worker heap evidence omitted actual managed objects".into());
        }
    }
    Ok(())
}

fn parse_resource_numbers(value: &str) -> Result<Vec<usize>, String> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    value
        .split(',')
        .map(|number| {
            number
                .parse()
                .map_err(|_| "invalid resource evidence".into())
        })
        .collect()
}

fn parse_transport_credits(
    value: &str,
) -> Result<Vec<rinzu_runtime::transport::TransportCreditEvidence>, String> {
    use rinzu_runtime::transport::{TransportCreditEvidence, TransportDirection};
    if value.is_empty() {
        return Ok(Vec::new());
    }
    value
        .split(';')
        .map(|record| {
            let numbers = record
                .split(':')
                .map(|n| {
                    n.parse::<u64>()
                        .map_err(|_| "invalid credit evidence".to_owned())
                })
                .collect::<Result<Vec<_>, _>>()?;
            if numbers.len() != 6 {
                return Err("invalid credit evidence fields".into());
            }
            Ok(TransportCreditEvidence {
                reservation: numbers[0],
                operation: Some(numbers[1]),
                direction: match numbers[2] {
                    0 => TransportDirection::Input,
                    1 => TransportDirection::Result,
                    _ => return Err("invalid credit direction".into()),
                },
                bytes: usize::try_from(numbers[3]).map_err(|_| "credit size overflow")?,
                reserved_at: numbers[4],
                released_at: Some(numbers[5]),
            })
        })
        .collect()
}

type TransportPackets = std::collections::BTreeMap<u64, (usize, Vec<usize>)>;

fn validate_transport_credits(
    credits: &[rinzu_runtime::transport::TransportCreditEvidence],
    reported_peak: usize,
) -> Result<TransportPackets, String> {
    use rinzu_runtime::transport::TransportDirection;
    let mut reservations = std::collections::HashSet::new();
    let mut events = std::collections::BTreeMap::new();
    let mut inputs = std::collections::BTreeMap::new();
    let mut results = std::collections::BTreeMap::<u64, Vec<(usize, u64)>>::new();
    for credit in credits {
        let operation = credit
            .operation
            .ok_or("transport credit has no child identity")?;
        let release = credit
            .released_at
            .ok_or("transport credit was not released")?;
        if !reservations.insert(credit.reservation)
            || credit.reserved_at == 0
            || release <= credit.reserved_at
            || credit.bytes > 128 * 1024 * 1024
        {
            return Err("invalid transport reservation lifetime".into());
        }
        if events
            .insert(credit.reserved_at, (true, credit.bytes))
            .is_some()
            || events.insert(release, (false, credit.bytes)).is_some()
        {
            return Err("duplicate transport credit sequence".into());
        }
        match credit.direction {
            TransportDirection::Input => {
                if inputs.insert(operation, (credit.bytes, release)).is_some() {
                    return Err("duplicate child input reservation".into());
                }
            }
            TransportDirection::Result => results
                .entry(operation)
                .or_default()
                .push((credit.bytes, credit.reserved_at)),
        }
    }
    let mut used = 0usize;
    let mut peak = 0;
    for (index, (&sequence, &(acquire, bytes))) in events.iter().enumerate() {
        if sequence != index as u64 + 1 {
            return Err("missing transport credit sequence".into());
        }
        used = if acquire {
            used.checked_add(bytes)
        } else {
            used.checked_sub(bytes)
        }
        .ok_or("transport credit imbalance")?;
        if used > 256 * 1024 * 1024 {
            return Err("transport process limit exceeded".into());
        }
        peak = peak.max(used);
    }
    if used != 0 || peak != reported_peak {
        return Err(format!(
            "transport credit evidence does not prove baseline/peak: {used}/{peak}, reported {reported_peak}"
        ));
    }
    let mut packets = TransportPackets::new();
    for (operation, (input, input_release)) in inputs {
        let result = results.remove(&operation).unwrap_or_default();
        if result.iter().any(|(_, reserve)| *reserve <= input_release) {
            return Err("result credit acquired before input settlement".into());
        }
        let mut widths = result
            .into_iter()
            .map(|(width, _)| width)
            .collect::<Vec<_>>();
        widths.sort_unstable();
        packets.insert(operation, (input, widths));
    }
    if !results.is_empty() {
        return Err("result credit has no admitted input".into());
    }
    Ok(packets)
}

fn parse_line<'a>(output: &'a str, prefix: &str) -> Result<&'a str, String> {
    output
        .lines()
        .find_map(|line| line.strip_prefix(prefix))
        .ok_or_else(|| format!("AOT output is missing {prefix}"))
}

fn parse_i64_line(output: &str, prefix: &str) -> Result<i64, String> {
    parse_line(output, prefix)?
        .parse()
        .map_err(|error| format!("AOT result is invalid: {error}"))
}

fn diagnostic_signature(failure: Option<&rinzu_runtime::failure::Failure>) -> String {
    use rinzu_runtime::failure::{ErrorKind, FailureCause};
    let code = |cause| match cause {
        FailureCause::ArithmeticTrap => 1,
        FailureCause::Cancelled => 2,
        FailureCause::Overloaded => 3,
        FailureCause::Shutdown => 4,
        FailureCause::Error(ErrorKind::Conversion) => 5,
        FailureCause::Unsupported => 6,
        FailureCause::RuntimePanic => 7,
        FailureCause::Panic => 16,
        FailureCause::Error(ErrorKind::NullNativeValue) => 8,
        FailureCause::Error(ErrorKind::InvalidUtf8) => 9,
        FailureCause::Error(ErrorKind::NativeRecordBusy) => 10,
        FailureCause::Error(ErrorKind::UnobservedChildFailure) => 11,
        FailureCause::Error(ErrorKind::Io) => 12,
        FailureCause::Error(ErrorKind::SupplantBusy) => 13,
        FailureCause::Error(ErrorKind::UnboundValue) => 14,
        FailureCause::Error(ErrorKind::User) => 15,
    };
    let Some(failure) = failure else {
        return "0".into();
    };
    std::iter::once(&failure.primary)
        .chain(&failure.cleanup)
        .map(|diagnostic| {
            let cause = code(diagnostic.cause);
            match diagnostic.source {
                Some(source) => format!(
                    "{cause}@{}:{}:{}",
                    source.file, source.start_byte, source.end_byte
                ),
                None => format!("{cause}@-"),
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn error_name(kind: rinzu_runtime::failure::ErrorKind) -> &'static str {
    use rinzu_runtime::failure::ErrorKind;
    match kind {
        ErrorKind::User => "user",
        ErrorKind::Conversion => "conversion",
        ErrorKind::NullNativeValue => "null-native-value",
        ErrorKind::InvalidUtf8 => "invalid-utf8",
        ErrorKind::NativeRecordBusy => "native-record-busy",
        ErrorKind::UnobservedChildFailure => "unobserved-child",
        ErrorKind::Io => "io",
        ErrorKind::SupplantBusy => "supplant-busy",
        ErrorKind::UnboundValue => "unbound-value",
    }
}

fn failure_name(result: &Result<i64, BaselineFailure>) -> &'static str {
    match result {
        Ok(_) => "none",
        Err(BaselineFailure::Arithmetic) => "arithmetic",
        Err(BaselineFailure::Conversion) => "conversion",
        Err(BaselineFailure::Error(kind)) => error_name(*kind),
        Err(BaselineFailure::Cancelled) => "cancelled",
        Err(BaselineFailure::Overloaded) => "overloaded",
        Err(BaselineFailure::Shutdown) => "shutdown",
        Err(BaselineFailure::Unsupported) => "unsupported",
        Err(BaselineFailure::RuntimePanic) => "runtime-panic",
        Err(BaselineFailure::Panic) => "panic",
    }
}

pub fn build_package(package: &Path, output: Option<PathBuf>) -> Result<AotBuild, String> {
    build_package_with_native_inputs(package, output, &rinzu_compiler::NativeCliInputs::default())
}

pub fn build_package_with_native_inputs(
    package: &Path,
    output: Option<PathBuf>,
    inputs: &rinzu_compiler::NativeCliInputs,
) -> Result<AotBuild, String> {
    let context = crate::input::check(package, &crate::input::Options::default(), inputs)?;
    build_checked_input(&context, output)
}

pub(crate) fn build_checked_input(
    context: &rinzu_compiler::CompilationContext,
    output: Option<PathBuf>,
) -> Result<AotBuild, String> {
    build_checked_layout(context, output, crate::app_layout::Layout::Sidecars)
}

pub(crate) fn build_checked_app(
    context: &rinzu_compiler::CompilationContext,
    output: PathBuf,
) -> Result<AotBuild, String> {
    build_checked_layout(context, Some(output), crate::app_layout::Layout::MacosApp)
}

fn build_checked_layout(
    context: &rinzu_compiler::CompilationContext,
    output: Option<PathBuf>,
    layout: crate::app_layout::Layout,
) -> Result<AotBuild, String> {
    let manifest = &context.metadata;
    let artifact = &context.artifact;
    let native_bindings = artifact
        .sdk_packages
        .iter()
        .flat_map(|package| &package.native_imports)
        .map(|import| (import.export_name.clone(), import.import_key))
        .collect::<HashMap<_, _>>();
    let native_imports = artifact
        .sdk_packages
        .iter()
        .flat_map(|package| &package.native_imports)
        .filter(|import| import.runtime_service.is_none())
        .cloned()
        .collect::<Vec<_>>();
    let mut native_build = crate::input::build_native(context)?;
    let _native_scratch = crate::verify_capture::TemporaryDirectory(native_build.directory.clone());
    native_build.validate_source_packages()?;
    native_build.source_packages = crate::native_deployment::prepare(
        &native_build.source_packages,
        &native_build.directory.join("deployment"),
    )?;
    let uses_source_async = manifest.language != "0.1" && main_uses_source_async(artifact);
    let aot = if uses_source_async {
        emit_source_async_aot_object(&artifact.ir, "main", &native_bindings)
    } else {
        emit_baseline_aot_object_with_native_bindings(&artifact.ir, "main", &native_bindings)
    }
    .map_err(|error| {
        crate::cli_diagnostics::stage_failure(
            Some(&context.entry),
            "AOT code generation",
            None,
            &format!("{error:#}"),
        )
    })?;
    let async_plan = if uses_source_async {
        async_aot_plan(artifact, &aot)?
    } else {
        None
    };
    let executable = std::path::absolute(output.unwrap_or_else(|| context.default_output()))
        .map_err(|error| error.to_string())?;
    crate::input::protect_outputs(
        context,
        &std::iter::once(executable.clone())
            .chain(
                [
                    ".o",
                    ".runtime.c",
                    ".metadata.json",
                    ".source-packages",
                    ".resources",
                ]
                .into_iter()
                .map(|suffix| with_suffix(&executable, suffix)),
            )
            .collect::<Vec<_>>(),
    )?;
    let parent = executable
        .parent()
        .ok_or_else(|| "AOT output has no parent directory".to_owned())?;
    crate::input::protect_outputs(
        context,
        &native_build
            .artifacts
            .iter()
            .map(|artifact| {
                artifact
                    .library
                    .file_name()
                    .map(|name| parent.join(name))
                    .ok_or_else(|| "native artifact has no filename".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?,
    )?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("create AOT output directory {}: {error}", parent.display()))?;
    let object = with_suffix(&executable, ".o");
    let shim = with_suffix(&executable, ".runtime.c");
    let metadata = with_suffix(&executable, ".metadata.json");
    fs::write(&object, &aot.object)
        .map_err(|error| format!("write AOT object {}: {error}", object.display()))?;
    fs::write(
        &shim,
        runtime_shim_with_layout(
            &aot,
            &native_imports,
            async_plan.as_ref(),
            &native_build.source_packages,
            &with_suffix(&executable, ".source-packages"),
            layout,
        )?,
    )
    .map_err(|error| format!("write AOT runtime shim {}: {error}", shim.display()))?;
    write_metadata_with_native_inputs(
        &metadata,
        &manifest.name,
        &aot,
        &artifact.native_inputs,
        uses_source_async,
    )?;
    let mut saved: serde_json::Value =
        serde_json::from_slice(&fs::read(&metadata).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    saved["compilation"] = json!({"mode":context.mode,"entry":context.entry,"language":manifest.language,"target":context.target,"semantics":rinzu_compiler::LANGUAGE_SEMANTICS_REVISION,"inputs":context.inputs});
    saved["compilation"]["inputFingerprint"] = json!(
        native_build
            .context_inputs
            .get(rinzu_bindgen::compilation::DIGEST_KEY)
    );
    saved["compilation"]["inputRecord"] = serde_json::from_str(
        native_build
            .context_inputs
            .get(rinzu_bindgen::compilation::KEY)
            .ok_or("compilation receipt missing")?,
    )
    .map_err(|error| error.to_string())?;
    fs::write(
        &metadata,
        serde_json::to_vec_pretty(&saved).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if !native_build.source_packages.is_empty() {
        let bundle = crate::source_package_bundle::save(
            &native_build.source_packages,
            &with_suffix(&executable, ".source-packages"),
        )?;
        let mut saved: serde_json::Value =
            serde_json::from_slice(&fs::read(&metadata).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?;
        saved["sourcePackages"] = json!({ "version": 2, "manifest": bundle.strip_prefix(parent).map_err(|error| error.to_string())? });
        fs::write(
            &metadata,
            serde_json::to_vec_pretty(&saved).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
    }

    let runtime_resources = rinzu_compiler::standard_library::resource_root()?;
    let link_toolchain = crate::native_clang::TargetClang::resolve()?;
    let mut linker = link_toolchain.command();
    linker
        .arg("-std=c11")
        .arg("-O2")
        .arg("-I")
        .arg(runtime_resources.join("include"))
        .arg(&object)
        .arg(&shim);
    if !aot.native_call_plans.is_empty() {
        linker
            .arg(runtime_resources.join("runtime/callback_entry.c"))
            .arg(runtime_resources.join("runtime/callback_template.S"));
    }
    if aot
        .native_call_plans
        .iter()
        .any(|plan| plan.signature.is_supplant)
    {
        linker
            .arg(runtime_resources.join("runtime/owner_dispatcher.c"))
            .args(["-framework", "CoreFoundation"]);
    }
    for artifact in &native_build.artifacts {
        linker.arg(&artifact.library);
    }
    // CLI native inputs have already been linked into their verified provider.
    // Linking them into the executable again lets dyld coalesce a weak symbol
    // to a second copy outside the binding plan's selected image.
    if async_plan.is_some() {
        linker.arg("-pthread");
        if async_plan
            .as_ref()
            .is_some_and(|plan| plan.source_file_read || !plan.file_read_keys.is_empty())
        {
            let data = manifest.root.join("data");
            let destination = with_suffix(&executable, ".resources").join("data");
            if data.exists() {
                copy_data_directory(&data, &destination)?;
            } else {
                // Absence is a service Error when read, rather than a build failure.
                fs::create_dir_all(&destination)
                    .map_err(|error| format!("create AOT resource directory: {error}"))?;
            }
        }
    }
    if !native_build.artifacts.is_empty() {
        linker.arg("-Wl,-rpath,@executable_path");
    }
    crate::cli_diagnostics::tool_output(
        linker.arg("-o").arg(&executable).output(),
        Some(&executable),
        "AOT linking",
        "clang",
    )?;
    link_toolchain.verify()?;
    native_build.verify_inputs()?;
    let mut saved: serde_json::Value =
        serde_json::from_slice(&fs::read(&metadata).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    saved["aotToolchain"] = json!(link_toolchain.fingerprint);
    saved["nativeDeployments"] = json!(native_build.source_packages.iter().map(|package| {
        let descriptor = rinzu_bindgen::source_package::validate_source_descriptor(&package.artifact.expected_wire)?;
        Ok(json!({"package": descriptor.interface.package.package, "inputFingerprint": descriptor.interface.package.build_inputs.get(rinzu_bindgen::compilation::DIGEST_KEY), "descriptorSha256": rinzu_bindgen::source_package::sha256(&package.artifact.expected_wire)}))
    }).collect::<Result<Vec<_>, String>>()?);
    fs::write(
        &metadata,
        serde_json::to_vec_pretty(&saved).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    for artifact in &native_build.artifacts {
        let destination = parent.join(
            artifact
                .library
                .file_name()
                .ok_or_else(|| "native package artifact has no file name".to_owned())?,
        );
        fs::copy(&artifact.library, &destination).map_err(|error| {
            format!(
                "copy native package {} to {}: {error}",
                artifact.library.display(),
                destination.display()
            )
        })?;
    }
    let _ = fs::remove_dir_all(native_build.directory);
    Ok(AotBuild {
        executable,
        object,
        metadata,
    })
}

pub(super) fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(suffix);
    PathBuf::from(value)
}

pub(crate) fn main_uses_source_async(artifact: &rinzu_compiler::FrontendArtifact) -> bool {
    // A synchronous entry may start asynchronous work. Runtime selection must
    // follow executable task operations, not the spelling of Main's signature.
    artifact
        .ir
        .source_native_signatures
        .iter()
        .any(|signature| signature.is_async || signature.is_supplant)
        || artifact.ir.functions.iter().any(|function| {
            function.is_async
                || function
                    .blocks
                    .iter()
                    .flat_map(|block| &block.operations)
                    .any(|op| {
                        matches!(
                            op.kind,
                            rinzu_compiler::OperationKind::TaskControl { .. }
                                | rinzu_compiler::OperationKind::Spawn { .. }
                        )
                    })
        })
}

fn async_scalar_kind(ty: &rinzu_compiler::Type) -> Result<u32, String> {
    if *ty == rinzu_compiler::Type::Unit {
        return Ok(11);
    }
    ty.scalar()
        .map(|scalar| scalar as u32)
        .ok_or_else(|| format!("AOT async transport for {ty:?} requires the R3 data backend"))
}

pub(crate) fn async_aot_plan(
    artifact: &rinzu_compiler::FrontendArtifact,
    aot: &BaselineAotObject,
) -> Result<Option<AsyncAotPlan>, String> {
    if !main_uses_source_async(artifact) {
        return Ok(None);
    }
    let mut workers = std::collections::BTreeMap::new();
    for operation in artifact
        .ir
        .functions
        .iter()
        .flat_map(|function| &function.blocks)
        .flat_map(|block| &block.operations)
    {
        if let rinzu_compiler::OperationKind::Spawn { target, .. } = &operation.kind {
            let function = artifact
                .ir
                .functions
                .iter()
                .find(|function| function.name == *target)
                .or_else(|| {
                    artifact
                        .ir
                        .functions
                        .iter()
                        .find(|function| function.name == target.rsplit("::").next().unwrap())
                })
                .ok_or_else(|| format!("async AOT Worker `{target}` is missing"))?;
            let metadata = aot
                .functions
                .iter()
                .find(|item| item.name == function.name)
                .ok_or_else(|| format!("async AOT Worker `{target}` has no compiled entry"))?;
            workers.insert(
                metadata.index,
                AsyncAotWorker {
                    index: metadata.index,
                    symbol: metadata.symbol.clone(),
                    parameters: function
                        .parameter_types
                        .iter()
                        .map(|ty| {
                            aot.data_type(ty).map(|item| item.index).ok_or_else(|| {
                                format!("AOT Worker parameter schema missing for {ty:?}")
                            })
                        })
                        .collect::<Result<_, _>>()?,
                    result: if aot
                        .data_type(&function.result_type)
                        .is_some_and(|item| item.managed)
                    {
                        12
                    } else {
                        async_scalar_kind(&function.result_type)?
                    },
                    result_type: aot
                        .data_type(&function.result_type)
                        .ok_or_else(|| "AOT Worker result schema missing".to_owned())?
                        .index,
                },
            );
        }
    }
    let mut callees = Vec::new();
    for function in artifact
        .ir
        .functions
        .iter()
        .filter(|function| function.is_async)
    {
        let metadata = aot
            .functions
            .iter()
            .find(|item| item.name == function.name)
            .ok_or_else(|| format!("async AOT callee `{}` has no compiled entry", function.name))?;
        let file = artifact
            .ir
            .source_files
            .get(&function.range.file_id.0)
            .and_then(|name| aot.source_files.iter().find(|(_, value)| *value == name))
            .map_or(u64::from(function.range.file_id.0), |(identity, _)| {
                *identity
            });
        callees.push(AsyncAotCallee {
            index: metadata.index,
            symbol: metadata.symbol.clone(),
            parameter_types: function
                .parameter_types
                .iter()
                .map(|ty| {
                    aot.data_type(ty)
                        .map(|entry| entry.index)
                        .ok_or_else(|| format!("missing compiled parameter type {ty}"))
                })
                .collect::<Result<_, _>>()?,
            managed_parameters: function
                .parameter_types
                .iter()
                .map(|ty| {
                    aot.data_type(ty)
                        .map(|entry| entry.managed)
                        .ok_or_else(|| format!("missing compiled parameter type {ty}"))
                })
                .collect::<Result<_, _>>()?,
            rooted_parameters: function
                .parameter_types
                .iter()
                .map(|ty| {
                    aot.data_type(ty)
                        .map(|entry| entry.managed || matches!(ty, rinzu_compiler::Type::Future(_)))
                        .ok_or_else(|| format!("missing compiled parameter type {ty}"))
                })
                .collect::<Result<_, _>>()?,
            result: if aot
                .data_type(&function.result_type)
                .is_some_and(|entry| entry.managed)
            {
                12
            } else if matches!(function.result_type, rinzu_compiler::Type::Future(_)) {
                13
            } else {
                async_scalar_kind(&function.result_type)?
            },
            result_type: aot
                .data_type(&function.result_type)
                .ok_or_else(|| format!("missing compiled result type {}", function.result_type))?
                .index,
            source: rinzu_runtime::failure::SourcePosition {
                file,
                start_byte: function.range.start_byte,
                end_byte: function.range.end_byte,
            },
        });
    }
    let mut file_read_keys = Vec::new();
    for import in artifact
        .sdk_packages
        .iter()
        .flat_map(|package| &package.native_imports)
    {
        match import.runtime_service.as_deref() {
            Some("reactor.file-read-size") => file_read_keys.push(import.import_key),
            Some(service) => {
                return Err(format!(
                    "async AOT runtime service `{service}` is not implemented"
                ));
            }
            None => {}
        }
    }
    file_read_keys.sort_unstable();
    let source_file_read = artifact
        .ir
        .source_native_signatures
        .iter()
        .any(|signature| {
            matches!(
                &signature.binding,
                rinzu_compiler::ast::NativeBinding::Runtime(
                    rinzu_compiler::RuntimeFunction::ReadResourceSize
                )
            )
        });
    let source_timer = artifact
        .ir
        .source_native_signatures
        .iter()
        .any(|signature| {
            matches!(
                &signature.binding,
                rinzu_compiler::ast::NativeBinding::Runtime(rinzu_compiler::RuntimeFunction::Sleep)
            )
        });
    Ok(Some(AsyncAotPlan {
        workers: workers.into_values().collect(),
        callees,
        file_read_keys,
        source_file_read,
        source_timer,
    }))
}

fn copy_data_directory(source: &Path, destination: &Path) -> Result<(), String> {
    if !source.is_dir() {
        return Err(format!(
            "async AOT package data directory is missing: {}",
            source.display()
        ));
    }
    fs::create_dir_all(destination).map_err(|error| {
        format!(
            "create AOT resource directory {}: {error}",
            destination.display()
        )
    })?;
    for entry in fs::read_dir(source)
        .map_err(|error| format!("read package data directory {}: {error}", source.display()))?
    {
        let entry = entry.map_err(|error| format!("read package data entry: {error}"))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("read package data file type: {error}"))?;
        let target = destination.join(entry.file_name());
        if file_type.is_symlink() {
            return Err(format!(
                "AOT package data cannot contain symbolic links: {}",
                entry.path().display()
            ));
        }
        if file_type.is_dir() {
            copy_data_directory(&entry.path(), &target)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), &target).map_err(|error| {
                format!(
                    "copy AOT resource {} to {}: {error}",
                    entry.path().display(),
                    target.display()
                )
            })?;
        } else {
            return Err(format!(
                "unsupported AOT package data entry: {}",
                entry.path().display()
            ));
        }
    }
    Ok(())
}

fn write_metadata_with_native_inputs(
    path: &Path,
    package: &str,
    aot: &BaselineAotObject,
    inputs: &rinzu_compiler::NativeCliInputs,
    source_async: bool,
) -> Result<(), String> {
    let functions = aot
        .functions
        .iter()
        .map(|function| {
            json!({
                "index": function.index,
                "name": function.name,
                "symbol": function.symbol,
                "parameterCount": function.parameter_count,
            })
        })
        .collect::<Vec<_>>();
    let mut metadata = json!({
        "schema": "rinzu.aot-runtime-metadata",
        "version": 1,
        "package": package,
        "nativeInputs": { "searchPaths": inputs.search_paths, "libraries": inputs.libraries, "objects": inputs.objects },
        "target": rinzu_runtime::V1_TARGET,
        "entryIndex": aot.entry_index,
        "entrySymbol": aot.entry_symbol,
        "functions": functions,
        "cLowering": {
            "functions": aot.c_functions,
            "fallbacks": aot.c_fallbacks,
            "sourceBytes": aot.c_function_source.len(),
            "codeBytesScope": "compile.codeBytes counts Cranelift managed code only; C functions are in the runtime translation unit",
        },
        "functionReferences": aot.reference_targets.iter().enumerate().map(|(index,target)| json!({"handle": index + 1, "target": target})).collect::<Vec<_>>(),
        "nativeCallPlans": aot.native_call_plans.iter().enumerate().map(|(index,plan)| json!({"identity": index, "function": plan.function, "call": plan.call.0, "declaration": plan.signature.name, "signature": plan.signature.canonical_type(), "callbacks": plan.callbacks.iter().map(|site| json!({"parameter": site.index, "targets": site.targets})).collect::<Vec<_>>() })).collect::<Vec<_>>(),
        "sourceFiles": aot.source_files,
        "compile": {
            "codeBytes": aot.compile.code_size,
            "stackMaps": aot.compile.stack_map_count,
            "stackMapEntries": aot.compile.stack_map_entry_count,
        },
        "runtimeSymbols": [
            "rinzu_baseline_poll",
            "rinzu_baseline_set_source",
            "rinzu_baseline_set_source_packed",
            "rinzu_baseline_observe_source",
            "rinzu_baseline_has_failure",
            "rinzu_baseline_function_entry",
            "rinzu_baseline_publish_roots",
            "rinzu_baseline_prepare_roots",
            "rinzu_baseline_commit_roots",
            "rinzu_baseline_publish_roots_poll",
            "rinzu_baseline_commit_roots_poll",
            "rinzu_baseline_commit_proven_roots_poll",
            "rinzu_baseline_commit_proven_roots_poll_owner_proven",
            "rinzu_baseline_commit_partial_roots_poll",
            "rinzu_baseline_commit_partial_roots_poll_owner_proven",
            "rinzu_baseline_commit_roots_poll_owner_proven",
            "rinzu_baseline_publish_roots_poll_owner_proven",
            "rinzu_baseline_project_owner_proven",
            "rinzu_baseline_construct_owner_proven",
            "rinzu_baseline_data_operation_owner_proven",
            "rinzu_baseline_bytes_slice_payload_owner_proven",
            "rinzu_baseline_bytes_int_length_owner_proven",
            "rinzu_baseline_string_scalar_length_owner_proven",
            "rinzu_baseline_collection_length_owner_proven",
            "rinzu_baseline_construct",
            "rinzu_baseline_data_operation",
            "rinzu_baseline_bytes_slice_payload",
            "rinzu_baseline_bytes_int_length",
            "rinzu_baseline_string_scalar_length",
            "rinzu_baseline_collection_length",
            "rinzu_baseline_project",
            "rinzu_baseline_store_field",
            "rinzu_baseline_bytes",
            "rinzu_baseline_bytes_pad",
            "rinzu_baseline_bytes_length",
            "rinzu_baseline_enum_variant",
            "rinzu_baseline_value_compare",
            "rinzu_baseline_checked_arithmetic",
            "rinzu_baseline_scalar_arithmetic",
            "rinzu_baseline_scalar_compare",
            "rinzu_baseline_scalar_convert",
            "rinzu_baseline_has_error",
            "rinzu_baseline_catch_error",
            "rinzu_baseline_string",
            "rinzu_baseline_error_message",
            "rinzu_baseline_error_data",
            "rinzu_baseline_stdout",
            "rinzu_baseline_format_string",
            "rinzu_baseline_utf8_length",
            "rinzu_baseline_string_compare",
            "rinzu_baseline_begin_cleanup",
            "rinzu_baseline_end_cleanup",
            "rinzu_baseline_resolve_dispatch",
            "rinzu_baseline_leave_dispatch",
            "rinzu_baseline_retain_return",
            "rinzu_baseline_native_invoke"
        ]
    });
    if source_async {
        metadata["sourceAsync"] =
            json!({"continuation": "main-thread-native-stack", "transport": "typed-scalar-v1"});
        metadata["runtimeSymbols"].as_array_mut().unwrap().extend(
            [
                "rinzu_baseline_async_spawn",
                "rinzu_baseline_async_await",
                "rinzu_baseline_async_call",
                "rinzu_baseline_async_service",
                "rinzu_baseline_supplant_reserve",
                "rinzu_baseline_supplant_start",
                "rinzu_baseline_supplant_settle",
            ]
            .into_iter()
            .map(|symbol| json!(symbol)),
        );
    }
    let bytes = serde_json::to_vec_pretty(&metadata)
        .map_err(|error| format!("serialize AOT metadata: {error}"))?;
    fs::write(path, bytes)
        .map_err(|error| format!("write AOT metadata {}: {error}", path.display()))
}

pub(crate) fn runtime_shim_with_source_packages(
    aot: &BaselineAotObject,
    imports: &[rinzu_compiler::NativeImport],
    async_plan: Option<&AsyncAotPlan>,
    packages: &[crate::source_native_package::BuiltSourcePackage],
    bundle: &Path,
) -> Result<String, String> {
    runtime_shim_with_layout(
        aot,
        imports,
        async_plan,
        packages,
        bundle,
        crate::app_layout::Layout::Sidecars,
    )
}

fn runtime_shim_with_layout(
    aot: &BaselineAotObject,
    imports: &[rinzu_compiler::NativeImport],
    async_plan: Option<&AsyncAotPlan>,
    packages: &[crate::source_native_package::BuiltSourcePackage],
    bundle: &Path,
    layout: crate::app_layout::Layout,
) -> Result<String, String> {
    crate::aot_source_native::plans(aot, packages)?;
    let record_layouts = crate::aot_source_native::record_layouts(aot, packages)?;
    let source = runtime_shim(aot, imports, async_plan, packages);
    let source = source.replace("/* SOURCE_NATIVE_RECORD_LAYOUTS */", &record_layouts);
    if aot.native_call_plans.is_empty() {
        return Ok(source);
    }
    let bundle = Path::new(bundle.file_name().ok_or("source bundle has no filename")?);
    let startup = crate::aot_source_native::startup(aot, packages, bundle, layout)?;
    Ok(source
        .replace("/* SOURCE_NATIVE_STARTUP */", &startup)
        .replace("/* SOURCE_NATIVE_RECORD_LAYOUTS */", &record_layouts))
}

fn runtime_shim(
    aot: &BaselineAotObject,
    native_imports: &[rinzu_compiler::NativeImport],
    async_plan: Option<&AsyncAotPlan>,
    packages: &[crate::source_native_package::BuiltSourcePackage],
) -> String {
    let mut source = String::from(
        "#include <stdint.h>\n#include <stdio.h>\n#ifndef printf\nstatic int rinzu_report_printf(const char *,...);\n#define printf rinzu_report_printf\n#define RINZU_AOT_BUFFERED_REPORT 1\n#endif\n#include <limits.h>\n#include <stdlib.h>\n#include <string.h>\n#include <pthread.h>\n#include \"rinzu/standalone_interrupt.h\"\n#include \"rinzu/runtime.h\"\n#include \"rinzu/failure.h\"\n#include \"rinzu/error_groups.h\"\n#include \"rinzu/managed_composite.h\"\n#include \"rinzu/native_record.h\"\n#include \"rinzu/native_userdata.h\"\n#include \"rinzu/owner_transition.h\"\n",
    );
    source.push_str(&aot.data_schema_c);
    source.push_str(&aot.source_locations_c);
    source.push_str(include_str!("aot_error_trace.c"));
    source.push_str("static const char *rinzu_error_trace_function_name(int64_t function) { switch(function) {\n");
    for function in &aot.functions {
        source.push_str(&format!(
            "case {}: return {};\n",
            function.index,
            crate::resource_report::c_string(&function.name)
        ));
    }
    source.push_str("default: rinzu_failure_fatal(); return NULL;} }\n");
    source.push_str(
        "static rinzu_value_schema_t rinzu_program_schema;\n\
         static pthread_once_t rinzu_program_schema_once = PTHREAD_ONCE_INIT;\n\
         static void rinzu_init_program_schema(void) { if (rinzu_value_schema_init(&rinzu_program_schema, rinzu_program_types, sizeof rinzu_program_types / sizeof *rinzu_program_types)) rinzu_failure_fatal(); }\n\
         typedef struct RinzuAsyncState RinzuAsyncState;\n\
         typedef struct RinzuAsyncTask RinzuAsyncTask;\n\
         typedef struct { RinzuAsyncState *async; RinzuAsyncTask *task; rinzu_owner_transition_t transitions; rinzu_owner_transition_t *shared_transitions; struct rinzu_foreign_queue *foreign_queue; struct rinzu_async_future *worker; struct RinzuNativeCall *native_calls; struct RinzuReturnRoot *return_roots; struct RinzuTaskScope *task_scopes; rinzu_userdata_registry_t userdata; rinzu_userdata_registry_t *shared_userdata; rinzu_failure_state_t pending; rinzu_error_groups_t error_groups; rinzu_error_trace_stack_t error_trace_stack; rinzu_managed_roots_t roots; rinzu_managed_data_t data; rinzu_managed_data_t *shared_data; rinzu_managed_root_set_t domain_roots; rinzu_source_position_t source; uint64_t failure_origin; uint64_t polls; uint64_t *domain_polls; uint64_t allocations; uint64_t cancel_after; uint64_t max_allocations; uint64_t shutdown_after; uint64_t callback_dispatches; uint64_t callback_depth; int cancel_requested; int shutdown_requested; int cancel_masked; } RinzuContext;\n\
         static __attribute__((noinline, cold)) void rinzu_context_configure_data(rinzu_managed_data_t *data) { if (pthread_once(&rinzu_program_schema_once, rinzu_init_program_schema) || rinzu_managed_configure_values(data, &rinzu_program_schema)) rinzu_failure_fatal(); rinzu_configure_program_sources(data); }\n\
         static inline rinzu_managed_data_t *rinzu_context_data(RinzuContext *context) { rinzu_managed_data_t *data = context->shared_data ? context->shared_data : &context->data; if (!data->schema) rinzu_context_configure_data(data); return data; }\n",
    );
    source.push_str(rinzu_runtime::stdio::C_SOURCE);
    source.push_str(include_str!("aot_report.c"));
    source.push_str(rinzu_runtime::data::display::C_SOURCE);
    source.push_str(include_str!("aot_transition.c"));
    source.push_str(include_str!("aot_return.c"));
    source.push_str(include_str!("aot_userdata.c"));
    if aot
        .native_call_plans
        .iter()
        .any(|plan| plan.signature.is_supplant)
    {
        source.push_str("#define RINZU_AOT_HAS_SUPPLANT 1\n#include \"rinzu/supplant.h\"\n#include \"rinzu/owner_dispatcher.h\"\n#include <sched.h>\n");
    }
    let records = include_str!("aot_native_record.c");
    if aot.native_record_schemas.is_empty() {
        source.push_str(&records.replace("/* SOURCE_NATIVE_RECORD_LAYOUTS */", "static const rinzu_native_record_layout_t rinzu_native_record_layouts[] = {{0,0,0,0,NULL}};\nstatic const size_t rinzu_native_record_layout_count = 0;\n"));
    } else {
        source.push_str(records);
    }
    if aot.native_call_plans.is_empty() {
        source.push_str("static void rinzu_aot_source_native_startup(void) {}\nstatic int rinzu_aot_source_native_baseline(void) { return 1; }\nstatic void rinzu_aot_source_native_evidence(void) {}\n");
    } else {
        source.push_str("#include \"rinzu/native_input.h\"\n");
        source.push_str("#define RINZU_AOT_HAS_SOURCE_NATIVE 1\nstatic void *rinzu_aot_callback_parent_scope(RinzuContext *);\nstatic void *rinzu_aot_callback_enter_scope(RinzuContext *,void *);\nstatic void rinzu_aot_callback_leave_scope(RinzuContext *,void *);\n");
        if async_plan.is_none() {
            source.push_str("static void *rinzu_aot_callback_parent_scope(RinzuContext *context) {(void)context;return NULL;}\nstatic void *rinzu_aot_callback_enter_scope(RinzuContext *context,void *scope) {(void)context;if(scope)_Exit(86);return NULL;}\nstatic void rinzu_aot_callback_leave_scope(RinzuContext *context,void *scope) {(void)context;if(scope)_Exit(86);}\n");
        }
        source.push_str(&include_str!("aot_source_native.c").replace(
            "/* SOURCE_NATIVE_PLANS */",
            &crate::aot_source_native::plans(aot, packages).expect("validated source native plans"),
        ));
    }
    for function in &aot.functions {
        let mut parameters = vec!["void *context".to_owned()];
        parameters
            .extend((0..function.parameter_count).map(|index| format!("int64_t argument_{index}")));
        source.push_str(&format!(
            "extern int64_t {}({});\n",
            function.symbol,
            parameters.join(", ")
        ));
    }
    for import in native_imports {
        source.push_str(&format!(
            "extern rinzu_status_t {}(const rinzu_native_invocation_context_v1_t *, const int64_t *, size_t, int64_t *);\n",
            import.symbol
        ));
    }
    if let Some(plan) = async_plan {
        if plan.source_file_read || !plan.file_read_keys.is_empty() {
            source.push_str("#define RINZU_AOT_HAS_FILE_READ 1\n");
        }
        if plan.source_timer {
            source.push_str("#define RINZU_AOT_HAS_TIMER 1\n");
        }
        source.push_str(include_str!("aot_source_async.c"));
        source.push_str(&format!(
            "_Static_assert(RINZU_ASYNC_HANDLE_BASE == UINT64_C({}), \"private Future handle base\");\n",
            rinzu_codegen::C_ASYNC_HANDLE_BASE
        ));
        if aot
            .native_call_plans
            .iter()
            .any(|plan| plan.signature.is_supplant)
        {
            source.push_str(include_str!("aot_supplant.c"));
        }
    } else {
        source.push_str("#define ASYNC_EVENT(raw, kind, future) ((void)0)\n#define rinzu_aot_async_observe(context, failure) ((void)0)\n#define rinzu_aot_async_cancelled(context) ((context)->cancel_requested)\n#define rinzu_aot_async_root_shutdown(context) 0\nint64_t rinzu_baseline_task_control(void *raw,int64_t kind,int64_t future) {(void)raw;(void)kind;(void)future;rinzu_failure_fatal();return 0;}\n");
    }
    source.push_str(&format!(
        "_Static_assert(offsetof(RinzuContext, pending) + offsetof(rinzu_failure_state_t, current) + offsetof(rinzu_failure_t, primary) == {}, \"private pending primary offset\");\n\
         _Static_assert(sizeof(((RinzuContext *)0)->pending.current.primary) == 8, \"private pending primary width\");\n\
         _Static_assert(_Alignof(RinzuContext) >= 8, \"private Context alignment\");\n",
        rinzu_codegen::C_CONTEXT_PENDING_PRIMARY_OFFSET
    ));
    source.push_str(&format!(
        "_Static_assert(offsetof(RinzuContext, failure_origin) == {}, \"private failure origin offset\");\n\
         _Static_assert(sizeof(((RinzuContext *)0)->failure_origin) == 8, \"private failure origin width\");\n",
        rinzu_codegen::C_CONTEXT_FAILURE_ORIGIN_OFFSET
    ));
    source.push_str(include_str!("aot_safepoint.c"));
    source.push_str(
        "\nvoid rinzu_baseline_begin_cleanup(void *raw, int64_t region) { RinzuContext *context = raw; rinzu_failure_begin_scoped(&context->pending, context->roots.depth, (uint64_t)region); }\n\
         void rinzu_baseline_end_cleanup(void *raw, int64_t region) { RinzuContext *context = raw; rinzu_failure_end_scoped(&context->pending, context->roots.depth, (uint64_t)region); }\n\
         void rinzu_baseline_leave_dispatch(void *raw) { RinzuContext *context = raw; rinzu_baseline_release_return(context); rinzu_error_trace_stack_leave(&context->error_trace_stack); rinzu_managed_leave(&context->roots); }\n\
         int64_t rinzu_baseline_checked_arithmetic(void *raw, int64_t operation, int64_t left, int64_t right) {\n\
           RinzuContext *context = raw; int64_t result = 0; int failed = 0;\n\
           switch (operation) {\n\
             case 0: failed = __builtin_sub_overflow((int64_t)0, left, &result); break;\n\
             case 1: failed = __builtin_add_overflow(left, right, &result); break;\n\
             case 2: failed = __builtin_sub_overflow(left, right, &result); break;\n\
             case 3: failed = __builtin_mul_overflow(left, right, &result); break;\n\
             case 4: if (right == 0 || (left == LLONG_MIN && right == -1)) failed = 1; else result = left / right; break;\n\
             case 5: if (right == 0 || (left == LLONG_MIN && right == -1)) failed = 1; else result = left % right; break;\n\
             default: failed = 1; break;\n\
           }\n\
           if (failed && context->pending.current.primary == 0) rinzu_failure_fail(&context->pending, 1, context->source); return result;\n\
         }\n\
         int64_t rinzu_baseline_resolve_dispatch(void *raw, int64_t function) {\n\
           (void)raw; switch (function) {\n",
    );
    append_dispatch_cases(&mut source, &aot.functions);
    source.push_str("    default: return 0;\n  }\n}\n\n");
    source.push_str(include_str!("aot_collections.c"));
    source.push_str(include_str!("aot_arguments.c"));
    if aot.uses_json {
        source.push_str(include_str!("aot_json.c"));
    }
    source.push_str(include_str!("aot_data.c"));
    source.push_str(include_str!("aot_stdio.c"));
    source.push_str(include_str!("aot_errors.c"));
    source.push_str("#include \"rinzu/scalar.h\"\n");
    source.push_str(&format!(
        "#define RINZU_RESOURCE_SOURCES {}\n#include \"rinzu/resource_report.h\"\n",
        crate::resource_report::c_string(&serde_json::to_string(&aot.source_files).unwrap())
    ));
    source.push_str("int64_t rinzu_baseline_scalar_arithmetic(void *raw, int64_t kind, int64_t operation, int64_t left, int64_t right) { RinzuContext *context = raw; rinzu_scalar_result_t result = rinzu_scalar_arithmetic((unsigned)kind, (unsigned)operation, (uint64_t)left, (uint64_t)right); if (result.status && context->pending.current.primary == 0) rinzu_failure_fail(&context->pending, 1, context->source); return (int64_t)result.bits; }\n");
    source.push_str("int64_t rinzu_baseline_scalar_compare(void *raw, int64_t kind, int64_t operation, int64_t left, int64_t right) { RinzuContext *context = raw; int result = rinzu_scalar_compare((unsigned)kind, (unsigned)operation, (uint64_t)left, (uint64_t)right); if (result < 0 && context->pending.current.primary == 0) rinzu_failure_fail(&context->pending, 1, context->source); return result < 0 ? 0 : result; }\n");
    source.push_str("int64_t rinzu_baseline_has_error(void *raw) { RinzuContext *context = raw; return rinzu_failure_recoverable(&context->pending); }\n");
    source.push_str("int64_t rinzu_baseline_catch_error(void *raw) { RinzuContext *context = raw; (void)rinzu_failure_observe(&context->pending); rinzu_aot_async_observe(context, &context->pending.observed->failure); uint64_t value = rinzu_managed_error(rinzu_context_data(context), &context->pending.observed->failure); context->allocations += 2; rinzu_failure_forget_managed(&context->pending.observed->failure); return (int64_t)value; }\n");
    source.push_str("int64_t rinzu_baseline_string(void *raw, const unsigned char *bytes, int64_t length) { RinzuContext *context = raw; if (length < 0) rinzu_failure_fatal(); uint64_t value = rinzu_managed_string_literal(rinzu_context_data(context), bytes, (size_t)length); context->allocations += 1; return (int64_t)value; }\n");
    source.push_str("int64_t rinzu_baseline_error_message(void *raw, int64_t error) { RinzuContext *context=raw; rinzu_managed_data_t *data=rinzu_context_data(raw); if (rinzu_managed_find(data,(uint64_t)error)->kind==RINZU_MANAGED_RICH_ERROR) context->allocations++; return (int64_t)rinzu_managed_message(data, (uint64_t)error); }\n");
    source.push_str("int64_t rinzu_baseline_utf8_length(void *raw, int64_t string) { return (int64_t)rinzu_managed_string_value(rinzu_context_data(raw), (uint64_t)string)->length; }\n");
    source.push_str("int64_t rinzu_baseline_string_compare(void *raw, int64_t operation, int64_t left, int64_t right) { if (operation != 0 && operation != 1) rinzu_failure_fatal(); return rinzu_managed_string_equal(rinzu_context_data(raw), (uint64_t)left, (uint64_t)right) ^ (operation == 1); }\n");
    source.push_str("int64_t rinzu_baseline_scalar_convert(void *raw, int64_t source, int64_t target, int64_t bits) { RinzuContext *context = raw; rinzu_scalar_result_t result = rinzu_scalar_convert((unsigned)source, (unsigned)target, (uint64_t)bits); if (result.status && context->pending.current.primary == 0) rinzu_failure_fail(&context->pending, 5, context->source); return (int64_t)result.bits; }\n");

    if !native_imports.is_empty() {
        source.push_str(
            "static int64_t rinzu_aot_invoke_handler(void *raw, uint64_t handler) {\n\
           ((RinzuContext *)raw)->callback_dispatches += 1; ASYNC_EVENT(raw, RINZU_EVENT_CALLBACK_ENTER, 0);\n\
           int64_t address = rinzu_baseline_resolve_dispatch(raw, (int64_t)handler);\n\
           if (address == 0) return 0;\n\
           int64_t (*callable)(void *) = (int64_t (*)(void *))(intptr_t)address;\n\
           int64_t result = callable(raw); rinzu_baseline_leave_dispatch(raw); ASYNC_EVENT(raw, RINZU_EVENT_CALLBACK_LEAVE, 0); return result;\n\
         }\n",
        );
    }
    source.push_str(
        "int64_t rinzu_baseline_native_invoke(void *raw, int64_t key, int64_t count, int64_t a0, int64_t a1, int64_t a2, int64_t a3, int64_t a4, int64_t a5, int64_t a6, int64_t a7) {\n\
           RinzuContext *context = raw;\n\
           if (count < 0 || count > 8) { rinzu_failure_fail(&context->pending, 3, context->source); return 0; }\n\
           int64_t arguments[8] = {a0, a1, a2, a3, a4, a5, a6, a7}; (void)arguments;\n",
    );
    if !native_imports.is_empty() {
        source.push_str("rinzu_native_invocation_context_v1_t invocation = {sizeof(invocation), RINZU_ABI_VERSION, raw, rinzu_aot_invoke_handler};\n");
    }
    source.push_str(
        "int64_t result = 0; rinzu_status_t status = RINZU_STATUS_UNSUPPORTED;\n\
           switch ((uint64_t)key) {\n",
    );
    for import in native_imports {
        source.push_str(&format!(
            "    case UINT64_C(0x{:016x}): ASYNC_EVENT(raw, RINZU_EVENT_NATIVE_ENTER, 0); status = {}(&invocation, arguments, (size_t)count, &result); ASYNC_EVENT(raw, RINZU_EVENT_NATIVE_LEAVE, 0); break;\n",
            import.import_key, import.symbol
        ));
    }
    if let Some(plan) = async_plan {
        for key in &plan.file_read_keys {
            source.push_str(&format!("    case UINT64_C(0x{key:016x}): if (count != 1) rinzu_failure_fatal(); return rinzu_baseline_async_file(raw, a0);\n"));
        }
    }
    source.push_str(
        "    default: break;\n  }\n  if (status != RINZU_STATUS_OK) { rinzu_failure_fail(&context->pending, 3, context->source); return 0; }\n  return result;\n}\n\n",
    );
    if let Some(plan) = async_plan {
        append_async_main(&mut source, aot, plan);
    } else {
        source.push_str(&format!(
        "int main(void) {{\n  rinzu_report_begin();\n  RinzuContext context = {{0}};\n  const char *cancel = getenv(\"RINZU_CANCEL_AFTER_POLLS\");\n  const char *maximum = getenv(\"RINZU_MAX_ALLOCATIONS\");\n  const char *shutdown = getenv(\"RINZU_SHUTDOWN_AFTER_POLLS\");\n  if (cancel) context.cancel_after = strtoull(cancel, NULL, 10);\n  if (maximum) context.max_allocations = strtoull(maximum, NULL, 10);\n  if (shutdown) context.shutdown_after = strtoull(shutdown, NULL, 10);\n  rinzu_activity_t *main_activity=rinzu_owner_activity_prepare(rinzu_context_transitions(&context),RINZU_ACTIVITY_MANAGED,{main_label},UINT64_C({main_file}),{main_start},{main_end},0);\n  rinzu_activity_start(main_activity);\n  int64_t result = {}(&context);\n  rinzu_activity_end(main_activity);\n  rinzu_baseline_leave_dispatch(&context);\n  rinzu_failure_forget_managed(&context.pending.current); rinzu_managed_collect(&context.data, &context.roots);\n  if (context.data.objects != 0) rinzu_failure_fatal(); rinzu_owner_transition_finish(rinzu_context_transitions(&context));\n  rinzu_failure_print(&context.pending); rinzu_failure_print_traces(&context.pending);\n  const char *outcome = context.pending.current.primary == 2 ? \"cancelled\" : ((context.pending.current.primary && context.pending.current.primary != 4) ? \"failure\" : \"success\");\n  const char *failure = rinzu_failure_name(context.pending.current.primary);\n  printf(\"RINZU_RESULT=%lld\\n\", (long long)result);\n  printf(\"RINZU_FAILURE=%s\\n\", failure);\n  printf(\"RINZU_TRACE={{\\\"schema\\\":\\\"rinzu.event-trace\\\",\\\"version\\\":1,\\\"producer\\\":\\\"rinzu-aot\\\",\\\"context\\\":{{\\\"engine\\\":\\\"baseline-aot\\\",\\\"resume-domain\\\":\\\"main\\\",\\\"resource-baseline\\\":\\\"true\\\",\\\"shutdown\\\":\\\"complete\\\"}},\\\"events\\\":[{{\\\"sequence\\\":0,\\\"domain\\\":\\\"main\\\",\\\"task\\\":\\\"main\\\",\\\"kind\\\":\\\"task-start\\\"}},{{\\\"sequence\\\":1,\\\"domain\\\":\\\"main\\\",\\\"task\\\":\\\"main\\\",\\\"kind\\\":\\\"task-complete\\\",\\\"outcome\\\":\\\"%s\\\"}}]}}\\n\", outcome);\n  int exit_code = (context.pending.current.primary == 0 || context.pending.current.primary == 4) ? 0 : 1; rinzu_failure_dispose(&context.pending); rinzu_managed_dispose(&context.roots); rinzu_managed_data_dispose(&context.data); if (rinzu_report_flush()) exit_code = 1; return exit_code;\n}}\n",
        aot.entry_symbol,
        main_label = serde_json::to_string(&aot.functions.iter().find(|function|function.index==aot.entry_index).unwrap().name).unwrap(),
        main_file = aot.functions.iter().find(|function|function.index==aot.entry_index).unwrap().source.file,
        main_start = aot.functions.iter().find(|function|function.index==aot.entry_index).unwrap().source.start_byte,
        main_end = aot.functions.iter().find(|function|function.index==aot.entry_index).unwrap().source.end_byte,
        ));
    }
    source = source.replace("int main(void) {", "int main(int argc, char **argv) {");
    if async_plan.is_none() {
        source = source.replace(
            "RinzuContext context = {0};",
            "RinzuContext context = {0}; context.data.program_argc = argc; context.data.program_argv = argv;",
        );
        source = source.replace(
            "rinzu_baseline_leave_dispatch(&context);",
            "rinzu_baseline_leave_dispatch(&context); context.data.program_arguments = 0;",
        );
    }
    source = source.replace(
        "int main(int argc, char **argv) {",
        "int main(int argc, char **argv) {\n  rinzu_standalone_interrupt_normalize();\n  if (rinzu_resource_report_from_env()) return 1;\n  rinzu_aot_source_native_startup();",
    );
    source = source.replace("rinzu_failure_print(&context.pending); rinzu_failure_print_traces(&context.pending);", "if (context.native_calls || !rinzu_aot_source_native_baseline()) rinzu_failure_fatal();\n  rinzu_aot_source_native_evidence();\n  if (context.return_roots) rinzu_failure_fatal();\n  rinzu_aot_userdata_evidence(&context);\n  rinzu_failure_print(&context.pending); rinzu_failure_print_traces(&context.pending);");
    source = source.replace(
        "rinzu_managed_data_dispose(&context.data);",
        "rinzu_aot_userdata_finish(&context); if (context.return_roots) rinzu_failure_fatal(); rinzu_managed_data_dispose(&context.data);",
    );
    source = source.replace(
        "return exit_code;",
        "if (rinzu_resource_report_finish()) exit_code = 1; return exit_code;",
    );
    source.push_str(&aot.c_function_source);
    source
}

fn append_dispatch_cases(source: &mut String, functions: &[AotFunctionMetadata]) {
    for function in functions {
        source.push_str(&format!(
            "    case {}: return (int64_t)(intptr_t)&{};\n",
            function.index, function.symbol
        ));
    }
}

fn append_async_main(source: &mut String, aot: &BaselineAotObject, plan: &AsyncAotPlan) {
    if !plan.workers.is_empty() {
        source.push_str("#define RINZU_AOT_HAS_CPU_WORKERS 1\n");
    }
    source.push_str(include_str!("aot_typed_worker.c"));
    source.push_str(
        "static void rinzu_aot_async_prepare(RinzuAsyncTask *task) {\n  switch (task->index) {\n",
    );
    for callee in &plan.callees {
        let validation = callee.parameter_types.iter().enumerate().map(|(index, ty)| format!("if (rinzu_managed_slot_valid(rinzu_context_data(&task->context), {ty}, (rinzu_managed_slot_t){{task->arguments[{index}], {reference}}})) rinzu_failure_fatal();", reference = u8::from(callee.managed_parameters[index]))).collect::<Vec<_>>().join(" ");
        let roots = callee
            .rooted_parameters
            .iter()
            .enumerate()
            .filter(|(_, managed)| **managed)
            .map(|(index, _)| format!("task->arguments[{index}]"))
            .collect::<Vec<_>>();
        let publish = if roots.is_empty() {
            "rinzu_managed_publish(&task->context.roots, NULL, 0);".to_owned()
        } else {
            format!(
                "uint64_t roots[] = {{{}}}; rinzu_managed_publish(&task->context.roots, roots, {});",
                roots.join(", "),
                roots.len()
            )
        };
        source.push_str(&format!(
            "    case {index}: {{ if (task->count != {count} || task->future->result_kind != {result}) rinzu_failure_fatal(); task->result_type = {result_type}; task->function_source = (rinzu_source_position_t){{UINT64_C({file}), {start}, {end}, 1}}; task->function_label = {label}; task->context.source = task->function_source; {validation} {publish} return; }}\n",
            index = callee.index, count = callee.managed_parameters.len(), result = callee.result,
            result_type = callee.result_type,
            file = callee.source.file, start = callee.source.start_byte, end = callee.source.end_byte,
            label = serde_json::to_string(&aot.functions.iter().find(|function| function.index == callee.index).unwrap().name).unwrap(),
        ));
    }
    source.push_str("    default: rinzu_failure_fatal();\n  }\n}\nstatic int64_t rinzu_aot_async_entry(RinzuAsyncTask *task) {\n  switch (task->index) {\n");
    for callee in &plan.callees {
        let arguments = (0..callee.managed_parameters.len())
            .map(|index| format!(", (int64_t)task->arguments[{index}]"))
            .collect::<String>();
        source.push_str(&format!(
            "    case {index}: return {symbol}(&task->context{arguments});\n",
            index = callee.index,
            symbol = callee.symbol
        ));
    }
    source.push_str("    default: rinzu_failure_fatal(); return 0;\n  }\n}\n");
    for worker in &plan.workers {
        if !worker.parameters.is_empty() {
            let kinds = worker
                .parameters
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            source.push_str(&format!(
                "static const size_t rinzu_worker_types_{}[] = {{{kinds}}};\n",
                worker.index
            ));
        }
        let arguments = (0..worker.parameters.len())
            .map(|index| format!(", (int64_t)future->arguments[{index}]"))
            .collect::<String>();
        source.push_str(&format!(r#"
static void rinzu_worker_run_{index}(rinzu_async_future_t *future) {{
  RinzuContext context = {{0}}; context.source = future->source; context.worker = future;
  const char *cancel = getenv("RINZU_CANCEL_WORKER_AFTER_POLLS"); if (cancel) context.cancel_after = strtoull(cancel, NULL, 10);
  const char *shutdown = getenv("RINZU_SHUTDOWN_WORKER_AFTER_POLLS"); if (shutdown) context.shutdown_after = strtoull(shutdown, NULL, 10);
  if (rinzu_aot_typed_prepare_worker(&context, future)) {{
    future->worker_body_entered = 1;
    rinzu_activity_t *activity=rinzu_owner_activity_prepare(rinzu_context_transitions(&context),RINZU_ACTIVITY_MANAGED,{label},future->source.file,future->source.start_byte,future->source.end_byte,context.roots.depth);
    rinzu_async_attach_native_board(future,activity->board);
    rinzu_activity_start(activity);
    future->result_slot = (uint64_t){symbol}(&context{arguments});
    rinzu_activity_end(activity);
    future->failure = context.pending.current; context.pending.current = (rinzu_failure_t){{0}};
    future->observed = context.pending.observed; context.pending.observed = NULL; context.pending.observed_count = 0;
    if (!future->failure.primary) rinzu_aot_typed_encode_result(&context, future);
    rinzu_baseline_leave_dispatch(&context);
  }}
  rinzu_aot_typed_finish_worker(&context, future);
}}
"#, index = worker.index, symbol = worker.symbol, label = serde_json::to_string(&aot.functions.iter().find(|function|function.index==worker.index).unwrap().name).unwrap()));
    }
    source.push_str("static rinzu_async_future_t *rinzu_aot_worker_request(RinzuContext *context, int64_t index, const uint64_t *arguments, int64_t count) {\n  (void)context; (void)arguments; (void)count;\n  switch (index) {\n");
    for worker in &plan.workers {
        let kinds = if worker.parameters.is_empty() {
            "NULL".into()
        } else {
            format!("rinzu_worker_types_{}", worker.index)
        };
        source.push_str(&format!("    case {index}: if (count != {count}) rinzu_failure_fatal(); return rinzu_aot_typed_worker(context, {kinds}, arguments, {count}, {result}, {result_type}, rinzu_worker_run_{index}, {label});\n", index = worker.index, count = worker.parameters.len(), result = worker.result, result_type = worker.result_type, label = serde_json::to_string(&aot.functions.iter().find(|function|function.index==worker.index).unwrap().name).unwrap()));
    }
    source.push_str("    default: rinzu_failure_fatal(); return NULL;\n  }\n}\n");
    source.push_str(&format!(r#"
int main(int argc, char **argv) {{
  rinzu_report_begin();
  RinzuContext context = {{0}}; RinzuAsyncState async = {{0}};
  context.data.program_argc = argc; context.data.program_argv = argv;
  rinzu_async_init(&async.runtime); async.executable = argv[0]; context.async = &async; async.root = &context;
  async.runtime.admission_hook = rinzu_aot_async_admission; async.runtime.admission_context = &async;
  async.runtime.record_evidence = getenv("RINZU_TRANSPORT_EVIDENCE") != NULL;
#ifdef RINZU_AOT_HAS_SUPPLANT
  rinzu_aot_supplant_init(&async);
#endif
  context.domain_polls = &async.polls; rinzu_continuation_init_root(&async.root_continuation);
  context.shared_data = &context.data; rinzu_managed_attach_roots(context.shared_data, &context.domain_roots, &context.roots);
  context.data.future_context=&async;context.data.future_begin=rinzu_aot_future_begin;
  context.data.future_mark=rinzu_aot_future_mark;context.data.future_sweep=rinzu_aot_future_sweep;
  context.data.future_due=rinzu_aot_future_due;
  rinzu_child_init(&async.children); async.runtime.scope = &async.children;
  const char *cancel = getenv("RINZU_CANCEL_AFTER_POLLS"); if (cancel) context.cancel_after = strtoull(cancel, NULL, 10);
  const char *shutdown = getenv("RINZU_SHUTDOWN_AFTER_POLLS"); if (shutdown) context.shutdown_after = strtoull(shutdown, NULL, 10);
  ASYNC_EVENT(&context, RINZU_EVENT_TASK_START, 0);
  async.root_source=(rinzu_source_position_t){{UINT64_C({root_file}),{root_start},{root_end},1}};
  async.root_label={root_label};
  async.root_activity=rinzu_owner_activity_prepare(rinzu_context_transitions(&context),RINZU_ACTIVITY_MANAGED,async.root_label,async.root_source.file,async.root_source.start_byte,async.root_source.end_byte,0);
  rinzu_activity_start(async.root_activity);
  int64_t result = {entry}(&context);
  if(async.root_activity){{rinzu_activity_end(async.root_activity);async.root_activity=NULL;}}
  rinzu_baseline_leave_dispatch(&context);
  rinzu_managed_collect(&context.data, &context.roots);
  rinzu_aot_async_close(&context);
  rinzu_managed_collect(&context.data, &context.roots);
  if (context.pending.current.primary) result = 0;
  rinzu_aot_async_credit_evidence(&async);
  rinzu_aot_typed_heap_evidence(&async);
#ifdef RINZU_AOT_HAS_SUPPLANT
  rinzu_aot_supplant_finish(&async);
#endif
  context.data.future_context=NULL;context.data.future_begin=NULL;context.data.future_mark=NULL;context.data.future_sweep=NULL;context.data.future_due=NULL;
  rinzu_async_dispose(&async.runtime);
  rinzu_child_dispose(&async.children);
  context.data.program_arguments = 0;
  rinzu_failure_forget_managed(&context.pending.current);
  rinzu_managed_collect(&context.data, &context.roots);
  rinzu_owner_transition_finish(rinzu_context_transitions(&context));
  int baseline = !async.runtime.records && !async.runtime.domains && !async.runtime.in_flight && !async.runtime.transport_bytes && !context.roots.depth && !context.data.objects && !async.tasks && !async.continuations.live && !async.continuations.mapped_bytes && context.data.root_sets == &context.domain_roots && !context.domain_roots.next;
  ASYNC_EVENT(&context, context.pending.current.primary == 2 ? RINZU_EVENT_TASK_CANCELLED : (context.pending.current.primary ? RINZU_EVENT_TASK_FAILURE : RINZU_EVENT_TASK_SUCCESS), 0);
  rinzu_failure_print(&context.pending); rinzu_failure_print_traces(&context.pending);
  printf("RINZU_RESULT=%lld\n", (long long)result);
  printf("RINZU_FAILURE=%s\n", rinzu_failure_name(context.pending.current.primary));
  printf("RINZU_FAILURE_MESSAGE=%s\n", rinzu_failure_is_error(context.pending.current.primary) ? rinzu_managed_error_text(context.pending.current.primary) : rinzu_failure_name(context.pending.current.primary));
  printf("RINZU_TRANSPORT=%zu,%zu,%zu,%zu,%zu,%zu\n", async.runtime.domains, async.runtime.peak_domains, async.runtime.in_flight, async.runtime.peak_in_flight, async.runtime.transport_bytes, async.runtime.peak_transport_bytes);
  printf("RINZU_CONTINUATIONS=%zu,%zu,%zu,%zu\n", async.continuations.live, async.continuations.peak_live, async.continuations.mapped_bytes, async.continuations.peak_mapped_bytes);
  printf("RINZU_FUTURE_GC=%zu,%zu,%zu\n",async.peak_future_records,async.gc_retired_futures,async.runtime.records);
  printf("RINZU_STARTED_CHILDREN=%zu,%zu\n", async.started_workers, async.started_files);
  printf("RINZU_STARTED_TIMERS=%zu\n", async.started_timers);
  printf("RINZU_NATIVE_COMPLETIONS=%zu\n", async.started_native_completions);
  printf("RINZU_STARTED_BEFORE_FIRST_AWAIT=%zu,%zu\n", async.first_await_workers, async.first_await_files);
  rinzu_aot_async_print(&async, baseline);
  int exit_code = ((context.pending.current.primary && !(context.pending.current.primary == 4 && async.explicit_shutdown && !context.pending.current.head)) || !baseline) ? 1 : async.exit_status;
  rinzu_managed_detach_roots(context.shared_data, &context.domain_roots);
  rinzu_failure_dispose(&context.pending); rinzu_managed_dispose(&context.roots); rinzu_managed_data_dispose(&context.data);
  if (rinzu_report_flush()) exit_code = 1;
  return exit_code;
}}
"#, root_file = aot.functions.iter().find(|function|function.index==aot.entry_index).unwrap().source.file,
        root_start = aot.functions.iter().find(|function|function.index==aot.entry_index).unwrap().source.start_byte,
        root_end = aot.functions.iter().find(|function|function.index==aot.entry_index).unwrap().source.end_byte,
        root_label = serde_json::to_string(&aot.functions.iter().find(|function|function.index==aot.entry_index).unwrap().name).unwrap(),
        entry = aot.entry_symbol));
}

#[cfg(test)]
#[path = "aot_userdata_tests.rs"]
mod userdata_tests;

#[cfg(test)]
#[path = "aot_callback_tests.rs"]
mod callback_tests;

#[cfg(test)]
#[path = "aot_error_tests.rs"]
mod error_tests;

#[cfg(test)]
#[path = "aot_native_record_tests.rs"]
mod native_record_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn revised_try_all_control_cancel_shutdown_and_fatal_match_generated_c() {
        let root = std::env::temp_dir().join(format!("rinzu-revised-all-{}", std::process::id()));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("rinzu.toml"),
            "[package]\nname=\"all_controls\"\nlanguage=\"0.5\"\n",
        )
        .unwrap();
        let source = r#"import rinzu.stdio;
fn line(text:String){try{rinzu.stdio.writeLine(text);}catch unexpected{panic("failed");}}
fn work()->Int {var n=0;while n<100000{n=n+1;}return n;}
async fn child() {try {try {let done=await spawn work();} finally {line("child-cleanup");}} catch unexpected {panic("failed");}}
async fn main() {
    try all {let pending=child();var n=0;while n<100000{n=n+1;}}
    catch errors {line("catch-unreachable");}
    finally {line("finally");}
}"#;
        fs::write(root.join("src/main.rinzu"), source).unwrap();
        verify_equivalence_with_options(
            &root,
            BaselineOptions {
                cancel_after_polls: Some(40),
                ..Default::default()
            },
        )
        .unwrap();
        verify_equivalence_with_options(
            &root,
            BaselineOptions {
                shutdown_after_polls: Some(40),
                ..Default::default()
            },
        )
        .unwrap();
        fs::write(
            root.join("src/main.rinzu"),
            source.replace(
                "let pending=child();var n=0;while n<100000{n=n+1;}",
                "let pending=child();let fatal=1/0;",
            ),
        )
        .unwrap();
        verify_equivalence(&root, None).unwrap();
        fs::write(
            root.join("src/main.rinzu"),
            include_str!("../../../tests/fixtures/programs/try-all-control/src/main.rinzu"),
        )
        .unwrap();
        verify_equivalence(&root, None).unwrap();
        let shutdown_source = r#"import rinzu.stdio;
fn line(text:String){try{rinzu.stdio.writeLine(text);}catch unexpected{panic("failed");}}
fn work()->Int {var n=0;while n<100000{n=n+1;}return n;}
async fn child(){try {try {let done=await spawn work();}finally{line("child");}}catch unexpected {panic("failed");}}
async fn window(){try all {let pending=child();let shutdown=requestShutdown();}finally{line("window");}}
async fn main(){try all {await window();}finally{line("main");}}
"#;
        fs::write(root.join("src/main.rinzu"), shutdown_source).unwrap();
        let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
        let built = build_package(&root, None).unwrap();
        let options = rinzu_codegen::SourceAsyncOptions {
            capture_stdout: true,
            record_transport_evidence: true,
            ..rinzu_codegen::SourceAsyncOptions::from_sdk_packages(
                &manifest.root,
                &artifact.sdk_packages,
            )
            .unwrap()
        };
        let mut reference =
            rinzu_codegen::run_source_async_report(&artifact.ir, "main", options).unwrap();
        let output = Command::new(&built.executable)
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .output()
            .unwrap();
        assert_eq!(output.stdout, reference.stdout);
        assert_eq!(output.status.code(), Some(i32::from(reference.exit_status)));
        let evidence =
            String::from_utf8(crate::verify_capture::evidence(&output).unwrap()).unwrap();
        let evidence = crate::future_identity::normalize(&reference, &evidence).unwrap();
        // Queued window and its waiting parent both receive the shutdown. The
        // first control poll can be at either site; ordinary Error locations and
        // every cleanup diagnostic remain exact in the shared comparisons.
        let failure = reference.failure_details.as_mut().unwrap();
        assert_eq!(
            failure.primary.cause,
            rinzu_runtime::failure::FailureCause::Shutdown
        );
        let source = failure.primary.source.unwrap();
        let allowed = ["requestShutdown()", "await window()"].map(|text| {
            let start = shutdown_source.find(text).unwrap() as u32;
            rinzu_runtime::failure::SourcePosition {
                file: source.file,
                start_byte: start,
                end_byte: start + text.len() as u32,
            }
        });
        assert!(allowed.contains(&source));
        let actual = parse_line(&evidence, "RINZU_DIAGNOSTICS=").unwrap();
        failure.primary.source = Some(
            *allowed
                .iter()
                .find(|source| {
                    actual
                        == format!(
                            "4@{}:{}:{}",
                            source.file, source.start_byte, source.end_byte
                        )
                })
                .unwrap_or_else(|| panic!("unexpected shutdown diagnostic: {actual}")),
        );
        compare_source_async_failures(&reference, &evidence).unwrap();
        compare_source_async_values_and_transport(&reference, &evidence).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    mod native_length_fixture {
        include!("../tests/fixtures/native_length_sdk/setup.rs");
    }
    mod native_opaque_fixture {
        include!("../tests/fixtures/native_opaque_sdk/setup.rs");
    }
    mod native_data_fixture {
        include!("../tests/fixtures/native_data_sdk/setup.rs");
    }
    mod native_record_fixture {
        include!("../tests/fixtures/native_record_sdk/setup.rs");
    }
    #[test]
    fn compiled_return_cleanup_roots_and_loop_routes_match_actual_aot() {
        let root =
            std::env::temp_dir().join(format!("rinzu-aot-return-finally-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("rinzu.toml"),
            "[package]\nname=\"return_finally\"\nlanguage=\"0.2\"\n",
        )
        .unwrap();
        std::fs::write(root.join("src/main.rinzu"), r#"
fn returning()->String {
    try {try {return "retained";}finally {try {let badInner=CInt8(300);}catch [inner: Error, ...innerRest] {let copiedInner=inner.message;}}}
    finally {try {let badOuter=CInt8(400);}catch [outer: Error, ...outerRest] {let copiedOuter=outer.message;}}
}
fn recursive(depth:Int)->String {try {if depth<4 {return recursive(depth+1);}else {return "retained";}}finally {let moved=["moving","cleanup","roots"];}}
fn unpublished()->String {try {return "unpublished";}finally {let badCleanup=CInt8(200);}}
async fn ready()->Int {return 4;}
fn launch()->Future<Int> {try {return ready();}finally {let movedFuture=["opaque","Future","handle"];}}
async fn main() {
    let ordinary=returning();let nested=recursive(0);let worker=await spawn returning();
    if ordinary!="retained"||nested!=ordinary||worker!=ordinary {panic("lostRoot");}
    try {let ignored=unpublished();}catch [cleanupError: Error, ...cleanupErrorRest] {let copied=cleanupError.message;}
    let future=launch();let readyValue=await future;if readyValue!=4 {panic("wrongFuture");}
    var count=0;while count<3 {try {continue;}finally {count=count+1;}}
    while true {try {break;}finally {count=count+1;}}
    if count!=4 {panic("lostCleanup");}
}
"#).unwrap();
        let run = crate::aot::verify_equivalence(&root, None).unwrap();
        assert_eq!(run.outcome, "success");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn source_native_data_aot_matches_values_errors_roots_signed_and_sanitized() {
        let root =
            std::env::temp_dir().join(format!("rinzu-aot-native-data-{}", std::process::id()));
        native_data_fixture::create(&root);
        let app = root.join("app");
        let (_, artifact) = rinzu_compiler::check_package(&app).unwrap();
        let built = build_package(&app, None).unwrap();
        let bundle = with_suffix(&built.executable, ".source-packages");
        let source = rinzu_codegen::run_source_async_report(
            &artifact.ir,
            "main",
            rinzu_codegen::SourceAsyncOptions {
                gc_stress: true,
                source_packages: Some(
                    crate::source_package_bundle::load(&bundle.join("manifest.json")).unwrap(),
                ),
                record_transport_evidence: true,
                ..Default::default()
            },
        )
        .unwrap();
        let check =
            |binary: &Path, expected: &rinzu_codegen::SourceAsyncRun, cancel: Option<usize>| {
                let mut command = Command::new(binary);
                command
                    .env("RINZU_TRANSPORT_EVIDENCE", "1")
                    .env("ASAN_OPTIONS", "detect_leaks=0");
                if let Some(limit) = cancel {
                    command.env("RINZU_CANCEL_WORKER_AFTER_POLLS", limit.to_string());
                }
                let actual = command.output().unwrap();
                assert_eq!(
                    actual.status.success(),
                    expected.failure_details.is_none(),
                    "{}",
                    String::from_utf8_lossy(&actual.stderr)
                );
                let stdout = std::str::from_utf8(&actual.stderr).unwrap().to_owned();
                compare_source_async_values_and_transport(expected, &stdout).unwrap();
            };
        check(&built.executable, &source, None);
        assert_eq!(source.native_calls.prepared, 166);
        assert_eq!(source.caught_errors.len(), 68);
        let entitlements = root.join("adhoc-sdk-library.plist");
        fs::write(&entitlements, "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>com.apple.security.cs.disable-library-validation</key><true/></dict></plist>").unwrap();
        let signed = Command::new("codesign")
            .args([
                "--force",
                "--sign",
                "-",
                "--options",
                "runtime",
                "--entitlements",
            ])
            .arg(&entitlements)
            .arg(&built.executable)
            .output()
            .unwrap();
        assert!(
            signed.status.success(),
            "{}",
            String::from_utf8_lossy(&signed.stderr)
        );
        assert!(
            Command::new("codesign")
                .args(["--verify", "--strict"])
                .arg(&built.executable)
                .status()
                .unwrap()
                .success()
        );
        check(&built.executable, &source, None);
        let sanitize = |built: &AotBuild| {
            let binary = with_suffix(&built.executable, ".sanitized");
            let output = Command::new("clang")
                .args([
                    "-std=c11",
                    "-O1",
                    "-g",
                    "-fsanitize=address,undefined",
                    "-fno-omit-frame-pointer",
                    "-pthread",
                ])
                .arg(&built.object)
                .arg(with_suffix(&built.executable, ".runtime.c"))
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime/src/callback_entry.c"))
                .arg(
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("../runtime/src/callback_template.S"),
                )
                .arg("-I")
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
                .arg("-o")
                .arg(&binary)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            binary
        };
        check(&sanitize(&built), &source, None);
        fs::write(app.join("src/main.rinzu"), "import buffers; async fn main() { let pending = spawn buffers.leaks(); let returned = await pending; }").unwrap();
        let (_, cancel_artifact) = rinzu_compiler::check_package(&app).unwrap();
        let cancelled = build_package(&app, Some(root.join("cancelled"))).unwrap();
        let cancelled_bundle = with_suffix(&cancelled.executable, ".source-packages");
        let sanitized_cancel = sanitize(&cancelled);
        for limit in [1, 3, 5, 10, 20, 100, 1000] {
            let expected = rinzu_codegen::run_source_async_report(
                &cancel_artifact.ir,
                "main",
                rinzu_codegen::SourceAsyncOptions {
                    gc_stress: true,
                    source_packages: Some(
                        crate::source_package_bundle::load(&cancelled_bundle.join("manifest.json"))
                            .unwrap(),
                    ),
                    cancel_worker_after_polls: Some(limit),
                    record_transport_evidence: true,
                    ..Default::default()
                },
            )
            .unwrap();
            assert!(expected.resources_returned && expected.native_calls.is_baseline());
            check(&cancelled.executable, &expected, Some(limit));
            check(&sanitized_cancel, &expected, Some(limit));
        }
        let loaded = crate::source_package_bundle::load(&bundle.join("manifest.json")).unwrap();
        let signature = artifact
            .ir
            .source_native_signatures
            .iter()
            .find(|signature| {
                signature.binding
                    == rinzu_compiler::ast::NativeBinding::Symbol("buffer_fixture_cleanup".into())
            })
            .unwrap();
        let import = loaded
            .imports()
            .values()
            .find(|import| import.identity == signature.name)
            .unwrap();
        // SAFETY: this validated Unit() fixture function runs after all physical
        // Workers and managed heaps returned, and releases only its private page.
        let cleanup: unsafe extern "C" fn() = unsafe { std::mem::transmute(import.glue_address()) };
        // SAFETY: no subsequent native read or borrowed view uses the page.
        unsafe { cleanup() };
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn source_native_record_aot_matches_layout_values_errors_roots_signed_and_sanitized() {
        let root =
            std::env::temp_dir().join(format!("rinzu-aot-native-record-{}", std::process::id()));
        native_record_fixture::create(&root);
        let app = root.join("app");
        let (_, artifact) = rinzu_compiler::check_package(&app).unwrap();
        let built = build_package(&app, None).unwrap();
        let bundle = with_suffix(&built.executable, ".source-packages");
        let source = rinzu_codegen::run_source_async_report(
            &artifact.ir,
            "main",
            rinzu_codegen::SourceAsyncOptions {
                gc_stress: true,
                source_packages: Some(
                    crate::source_package_bundle::load(&bundle.join("manifest.json")).unwrap(),
                ),
                record_transport_evidence: true,
                ..Default::default()
            },
        )
        .unwrap();
        let check =
            |binary: &Path, expected: &rinzu_codegen::SourceAsyncRun, cancel: Option<usize>| {
                let mut command = Command::new(binary);
                command
                    .env("RINZU_TRANSPORT_EVIDENCE", "1")
                    .env("ASAN_OPTIONS", "detect_leaks=0");
                if let Some(limit) = cancel {
                    command.env("RINZU_CANCEL_WORKER_AFTER_POLLS", limit.to_string());
                }
                let actual = command.output().unwrap();
                assert_eq!(
                    actual.status.success(),
                    expected.failure_details.is_none(),
                    "{}",
                    String::from_utf8_lossy(&actual.stderr)
                );
                let stdout = std::str::from_utf8(&actual.stderr).unwrap().to_owned();
                compare_source_async_values_and_transport(expected, &stdout).unwrap();
            };
        check(&built.executable, &source, None);
        assert_eq!(source.native_calls.prepared, 102);
        assert_eq!(source.caught_errors.len(), 22);
        let entitlements = root.join("adhoc-sdk-library.plist");
        fs::write(&entitlements, "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>com.apple.security.cs.disable-library-validation</key><true/></dict></plist>").unwrap();
        let signed = Command::new("codesign")
            .args([
                "--force",
                "--sign",
                "-",
                "--options",
                "runtime",
                "--entitlements",
            ])
            .arg(&entitlements)
            .arg(&built.executable)
            .output()
            .unwrap();
        assert!(
            signed.status.success(),
            "{}",
            String::from_utf8_lossy(&signed.stderr)
        );
        assert!(
            Command::new("codesign")
                .args(["--verify", "--strict"])
                .arg(&built.executable)
                .status()
                .unwrap()
                .success()
        );
        check(&built.executable, &source, None);
        let sanitize = |built: &AotBuild| {
            let binary = with_suffix(&built.executable, ".sanitized");
            let output = Command::new("clang")
                .args([
                    "-std=c11",
                    "-O1",
                    "-g",
                    "-fsanitize=address,undefined",
                    "-fno-omit-frame-pointer",
                    "-pthread",
                ])
                .arg(&built.object)
                .arg(with_suffix(&built.executable, ".runtime.c"))
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime/src/callback_entry.c"))
                .arg(
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("../runtime/src/callback_template.S"),
                )
                .arg("-I")
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
                .arg("-o")
                .arg(&binary)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            binary
        };
        check(&sanitize(&built), &source, None);
        fs::write(app.join("src/main.rinzu"), "import records; async fn main() { let pending = spawn records.leaks(); let returned = await pending; }").unwrap();
        let (_, cancel_artifact) = rinzu_compiler::check_package(&app).unwrap();
        let cancelled = build_package(&app, Some(root.join("cancelled"))).unwrap();
        let cancelled_bundle = with_suffix(&cancelled.executable, ".source-packages");
        let sanitized_cancel = sanitize(&cancelled);
        for limit in [1, 3, 5, 10, 20, 100, 1000] {
            let expected = rinzu_codegen::run_source_async_report(
                &cancel_artifact.ir,
                "main",
                rinzu_codegen::SourceAsyncOptions {
                    gc_stress: true,
                    source_packages: Some(
                        crate::source_package_bundle::load(&cancelled_bundle.join("manifest.json"))
                            .unwrap(),
                    ),
                    cancel_worker_after_polls: Some(limit),
                    record_transport_evidence: true,
                    ..Default::default()
                },
            )
            .unwrap();
            assert!(expected.resources_returned && expected.native_calls.is_baseline());
            check(&cancelled.executable, &expected, Some(limit));
            check(&sanitized_cancel, &expected, Some(limit));
        }
        let loaded = crate::source_package_bundle::load(&bundle.join("manifest.json")).unwrap();
        let signature = artifact
            .ir
            .source_native_signatures
            .iter()
            .find(|signature| {
                signature.binding
                    == rinzu_compiler::ast::NativeBinding::Symbol("record_fixture_cleanup".into())
            })
            .unwrap();
        let import = loaded
            .imports()
            .values()
            .find(|import| import.identity == signature.name)
            .unwrap();
        // SAFETY: this validated Unit() fixture function runs after all physical
        // Workers and managed heaps returned, and releases only its private page.
        let cleanup: unsafe extern "C" fn() = unsafe { std::mem::transmute(import.glue_address()) };
        // SAFETY: no subsequent native read or borrowed view uses the page.
        unsafe { cleanup() };
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn source_native_opaque_aot_matches_worker_values_error_cleanup_and_sanitizers() {
        let root =
            std::env::temp_dir().join(format!("rinzu-aot-native-opaque-{}", std::process::id()));
        native_opaque_fixture::create(&root);
        let app = root.join("app");
        let (_, artifact) = rinzu_compiler::check_package(&app).unwrap();
        let built = build_package(&app, None).unwrap();
        let bundle = with_suffix(&built.executable, ".source-packages");
        let source = rinzu_codegen::run_source_async_report(
            &artifact.ir,
            "main",
            rinzu_codegen::SourceAsyncOptions {
                gc_stress: true,
                source_packages: Some(
                    crate::source_package_bundle::load(&bundle.join("manifest.json")).unwrap(),
                ),
                record_transport_evidence: true,
                ..Default::default()
            },
        )
        .unwrap();
        let check = |binary: &Path, expected: &rinzu_codegen::SourceAsyncRun| {
            let output = Command::new(binary)
                .env("RINZU_TRANSPORT_EVIDENCE", "1")
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            );
            let stdout = std::str::from_utf8(&output.stderr).unwrap().to_owned();
            compare_source_async_values_and_transport(expected, &stdout).unwrap();
            assert_eq!(parse_line(&stdout, "RINZU_FAILURE=").unwrap(), "none");
        };
        check(&built.executable, &source);
        assert_eq!(source.native_calls.prepared, 351);
        assert_eq!(source.caught_errors.len(), 4);
        let entitlements = root.join("adhoc-sdk-library.plist");
        fs::write(&entitlements, "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>com.apple.security.cs.disable-library-validation</key><true/></dict></plist>").unwrap();
        let signed = Command::new("codesign")
            .args([
                "--force",
                "--sign",
                "-",
                "--options",
                "runtime",
                "--entitlements",
            ])
            .arg(&entitlements)
            .arg(&built.executable)
            .output()
            .unwrap();
        assert!(
            signed.status.success(),
            "{}",
            String::from_utf8_lossy(&signed.stderr)
        );
        let verified = Command::new("codesign")
            .args(["--verify", "--strict"])
            .arg(&built.executable)
            .output()
            .unwrap();
        assert!(
            verified.status.success(),
            "{}",
            String::from_utf8_lossy(&verified.stderr)
        );
        check(&built.executable, &source);

        let sanitize = |built: &AotBuild| {
            let binary = with_suffix(&built.executable, ".sanitized");
            let output = Command::new("clang")
                .args([
                    "-O1",
                    "-g",
                    "-fsanitize=address,undefined",
                    "-fno-omit-frame-pointer",
                    "-pthread",
                ])
                .arg(&built.object)
                .arg(with_suffix(&built.executable, ".runtime.c"))
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime/src/callback_entry.c"))
                .arg(
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("../runtime/src/callback_template.S"),
                )
                .arg("-I")
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
                .arg("-o")
                .arg(&binary)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            // Startup finds the saved bundle relative to the original executable
            // name compiled into the runtime, so both binaries share this bundle.
            binary
        };
        check(&sanitize(&built), &source);

        fs::write(app.join("src/main.rinzu"), "import objects; async fn main() { if objects.nullableLookup(42) != 1 { panic(\"wrongMain\"); } let a = spawn objects.nullableLookup(1); let b = spawn objects.nullableLookup(2); if await a != 1 || await b != 1 { panic(\"wrongWorker\"); } }").unwrap();
        let (_, null_artifact) = rinzu_compiler::check_package(&app).unwrap();
        let null_build = build_package(&app, Some(root.join("nullable"))).unwrap();
        let null_bundle = with_suffix(&null_build.executable, ".source-packages");
        let null_source = rinzu_codegen::run_source_async_report(
            &null_artifact.ir,
            "main",
            rinzu_codegen::SourceAsyncOptions {
                gc_stress: true,
                source_packages: Some(
                    crate::source_package_bundle::load(&null_bundle.join("manifest.json")).unwrap(),
                ),
                record_transport_evidence: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(null_source.native_calls.prepared, 12);
        assert_eq!(null_source.caught_errors.len(), 3);
        assert_eq!(null_source.native_calls.callback_entries, 96);
        check(&null_build.executable, &null_source);
        check(&sanitize(&null_build), &null_source);

        fs::write(app.join("src/main.rinzu"), "import objects; async fn main() { let pending = spawn objects.leaks(42); let returned = await pending; }").unwrap();
        let (_, cancel_artifact) = rinzu_compiler::check_package(&app).unwrap();
        let cancel_build = build_package(&app, Some(root.join("cancelled"))).unwrap();
        let cancel_bundle = with_suffix(&cancel_build.executable, ".source-packages");
        let mut returned_opaque = false;
        for limit in [1, 3, 5, 10, 20, 100, 1000] {
            let expected = rinzu_codegen::run_source_async_report(
                &cancel_artifact.ir,
                "main",
                rinzu_codegen::SourceAsyncOptions {
                    gc_stress: true,
                    source_packages: Some(
                        crate::source_package_bundle::load(&cancel_bundle.join("manifest.json"))
                            .unwrap(),
                    ),
                    cancel_worker_after_polls: Some(limit),
                    record_transport_evidence: true,
                    ..Default::default()
                },
            )
            .unwrap();
            assert!(expected.resources_returned);
            assert!(expected.native_calls.is_baseline());
            assert_eq!(
                expected.native_calls.prepared,
                expected.native_calls.released
            );
            returned_opaque |= expected.native_calls.returned > 0;
            let actual = Command::new(&cancel_build.executable)
                .env("RINZU_CANCEL_WORKER_AFTER_POLLS", limit.to_string())
                .env("RINZU_TRANSPORT_EVIDENCE", "1")
                .output()
                .unwrap();
            let stdout = std::str::from_utf8(&actual.stderr).unwrap().to_owned();
            compare_source_async_values_and_transport(&expected, &stdout)
                .unwrap_or_else(|error| panic!("limit={limit}: {error}\n{stdout}"));
            assert_eq!(
                parse_line(&stdout, "RINZU_FAILURE=").unwrap(),
                if expected.failure_details.is_some() {
                    "cancelled"
                } else {
                    "none"
                }
            );
        }
        assert!(returned_opaque);
        let cleanup = |module: &rinzu_compiler::IrModule, bundle: &Path| {
            let loaded = crate::source_package_bundle::load(&bundle.join("manifest.json")).unwrap();
            let signature = module
                .source_native_signatures
                .iter()
                .find(|signature| {
                    signature.binding
                        == rinzu_compiler::ast::NativeBinding::Symbol(
                            "object_fixture_cleanup".into(),
                        )
                })
                .unwrap();
            let import = loaded
                .imports()
                .values()
                .find(|import| import.identity == signature.name)
                .unwrap();
            // SAFETY: this exact validated Unit() binding runs only after every
            // Source Worker returned and all managed wrappers were collected.
            let cleanup: unsafe extern "C" fn() =
                unsafe { std::mem::transmute(import.glue_address()) };
            // SAFETY: all admitted Workers and native calls have returned.
            unsafe { cleanup() };
        };
        cleanup(&artifact.ir, &bundle);
        cleanup(&null_artifact.ir, &null_bundle);
        cleanup(&cancel_artifact.ir, &cancel_bundle);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn source_native_opaque_receives_prepared_results_and_recovers_null_on_main_workers() {
        let root =
            std::env::temp_dir().join(format!("rinzu-source-native-opaque-{}", std::process::id()));
        native_opaque_fixture::create(&root);
        let app = root.join("app");
        let (manifest, artifact) = rinzu_compiler::check_package(&app).unwrap();
        crate::native_contract::check_artifact(&manifest, &artifact).unwrap();
        let built = crate::native_package::build_with_native_inputs(
            &artifact.sdk_packages,
            &manifest.root,
            &artifact.native_inputs,
        )
        .unwrap();
        let loaded = built.load_source_packages().unwrap();
        let address = |symbol: &str| {
            let signature = artifact
                .ir
                .source_native_signatures
                .iter()
                .find(|signature| {
                    signature.binding == rinzu_compiler::ast::NativeBinding::Symbol(symbol.into())
                })
                .unwrap();
            loaded
                .imports()
                .values()
                .find(|import| import.identity == signature.name)
                .unwrap()
                .glue_address() as usize
        };
        // SAFETY: these process-pinned v2 bindings were checked against the exact
        // ordinary CSize(CInt) and Unit() source/C prototypes above.
        let stats: unsafe extern "C" fn(i32) -> u64 =
            unsafe { std::mem::transmute(address("object_stats")) };
        // SAFETY: the cleanup binding has a checked physical Unit() signature.
        let cleanup: unsafe extern "C" fn() =
            unsafe { std::mem::transmute(address("object_fixture_cleanup")) };
        let normal = rinzu_codegen::run_source_async_report(
            &artifact.ir,
            "main",
            rinzu_codegen::SourceAsyncOptions {
                gc_stress: true,
                source_packages: Some(loaded),
                record_transport_evidence: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(normal.failure_details, None);
        assert_eq!(normal.caught_errors.len(), 4);
        assert_eq!(
            normal.caught_errors[0].primary.cause,
            rinzu_runtime::failure::FailureCause::Error(
                rinzu_runtime::failure::ErrorKind::Conversion
            )
        );
        assert!(
            normal.caught_errors[1..]
                .iter()
                .all(|failure| failure.primary.cause
                    == rinzu_runtime::failure::FailureCause::Error(
                        rinzu_runtime::failure::ErrorKind::NullNativeValue
                    ))
        );
        assert!(normal.native_calls.is_baseline());
        assert_eq!(
            [
                normal.native_calls.prepared,
                normal.native_calls.returned,
                normal.native_calls.closed,
                normal.native_calls.settled,
                normal.native_calls.read,
                normal.native_calls.released,
            ],
            [351; 6]
        );
        assert_eq!(normal.native_calls.abandoned, 0);
        assert_eq!(normal.native_calls.callback_entries, 288);
        assert_eq!(normal.native_calls.copied_bytes, 1152);
        assert_eq!(normal.native_calls.prepared, normal.native_calls.released);
        assert!(normal.heap.collections > 100);
        assert_eq!(normal.worker_heaps.len(), 9);
        assert!(
            normal
                .worker_heaps
                .iter()
                .all(|worker| worker.heap.objects == 0 && worker.heap.roots == 0)
        );
        assert!(normal.resources_returned);
        // SAFETY: all physical calls/Workers have returned; fixture cleanup runs
        // explicitly only after managed wrapper teardown, never as a GC hook.
        unsafe {
            assert_eq!(stats(0), 9);
            assert_eq!(stats(1), 9);
            assert_eq!(stats(2), 9);
            assert_eq!(stats(3), 0);
            cleanup();
        }

        fs::write(app.join("src/main.rinzu"), "import objects; async fn main() { let child = spawn objects.leaks(42); try { let result = await child; } finally { let cleanupMarker = 7; } }").unwrap();
        let (_, cancelled_artifact) = rinzu_compiler::check_package(&app).unwrap();
        for limit in [1, 3, 5, 10, 20, 100, 1000] {
            let run = rinzu_codegen::run_source_async_report(
                &cancelled_artifact.ir,
                "main",
                rinzu_codegen::SourceAsyncOptions {
                    gc_stress: true,
                    source_packages: Some(built.load_source_packages().unwrap()),
                    cancel_worker_after_polls: Some(limit),
                    record_transport_evidence: true,
                    ..Default::default()
                },
            )
            .unwrap();
            assert!(
                run.native_calls.is_baseline(),
                "{limit}: {:?}",
                run.native_calls
            );
            assert_eq!(run.native_calls.prepared, run.native_calls.returned);
            assert_eq!(run.native_calls.prepared, run.native_calls.released);
            assert_eq!(
                [
                    run.native_calls.closed,
                    run.native_calls.settled,
                    run.native_calls.read
                ],
                [run.native_calls.prepared; 3]
            );
            assert_eq!(run.native_calls.abandoned, 0);
            assert!(run.resources_returned, "{limit}");
            // SAFETY: the cancelled Worker physically joined. Its wrapper GC
            // must not synthesize native retain/release or close the entity.
            unsafe {
                assert_eq!(stats(1), 0);
                assert_eq!(stats(2), 0);
                assert_eq!(stats(0), stats(3));
                cleanup();
            }
        }
        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&built.directory).unwrap();
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn source_native_length_plans_match_real_aot_workers_signed_images_and_sanitizers() {
        let root =
            std::env::temp_dir().join(format!("rinzu-aot-native-length-{}", std::process::id()));
        native_length_fixture::create(&root);
        let app = root.join("app");
        let (_, artifact) = rinzu_compiler::check_package(&app).unwrap();
        let built = build_package(&app, None).unwrap();
        let bundle = with_suffix(&built.executable, ".source-packages");
        let loaded = crate::source_package_bundle::load(&bundle.join("manifest.json")).unwrap();
        let jit = rinzu_codegen::run_source_async_report(
            &artifact.ir,
            "main",
            rinzu_codegen::SourceAsyncOptions {
                gc_stress: true,
                source_packages: Some(loaded),
                record_transport_evidence: true,
                ..Default::default()
            },
        )
        .unwrap();
        let normal = Command::new(&built.executable)
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .output()
            .unwrap();
        assert!(
            normal.status.success(),
            "{}",
            String::from_utf8_lossy(&normal.stderr)
        );
        let stdout = std::str::from_utf8(&normal.stderr).unwrap().to_owned();
        compare_source_async_values_and_transport(&jit, &stdout).unwrap();
        assert_eq!(parse_line(&stdout, "RINZU_FAILURE=").unwrap(), "none");
        for arguments in [
            vec!["--force", "--sign", "-", "--options", "runtime"],
            vec!["--verify", "--strict"],
        ] {
            let signed = Command::new("codesign")
                .args(arguments)
                .arg(&built.executable)
                .output()
                .unwrap();
            assert!(
                signed.status.success(),
                "{}",
                String::from_utf8_lossy(&signed.stderr)
            );
        }
        let strict = Command::new(&built.executable).output().unwrap();
        assert_eq!(strict.status.code(), Some(78));
        assert!(String::from_utf8_lossy(&strict.stderr).contains("Team IDs"));
        // Ad-hoc SDK facades have no distribution Team ID. This local fixture
        // grants only the library-loading exception; no JIT/mapping entitlement.
        let local_entitlements = root.join("adhoc-library.plist");
        fs::write(&local_entitlements, "<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>com.apple.security.cs.disable-library-validation</key><true/></dict></plist>").unwrap();
        let signed = Command::new("codesign")
            .args([
                "--force",
                "--sign",
                "-",
                "--options",
                "runtime",
                "--entitlements",
            ])
            .arg(&local_entitlements)
            .arg(&built.executable)
            .output()
            .unwrap();
        assert!(
            signed.status.success(),
            "{}",
            String::from_utf8_lossy(&signed.stderr)
        );
        let entitlements = Command::new("codesign")
            .args(["-d", "--entitlements", "-"])
            .arg(&built.executable)
            .output()
            .unwrap();
        let entitlement_text = String::from_utf8_lossy(&entitlements.stdout);
        assert!(
            entitlements.status.success()
                && entitlement_text.contains("com.apple.security.cs.disable-library-validation")
        );
        assert!(
            !entitlement_text.contains("allow-jit")
                && !entitlement_text.contains("allow-unsigned-executable-memory")
        );
        let signed = Command::new(&built.executable)
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .output()
            .unwrap();
        assert!(
            signed.status.success(),
            "{}",
            String::from_utf8_lossy(&signed.stderr)
        );
        compare_source_async_values_and_transport(
            &jit,
            std::str::from_utf8(&signed.stderr).unwrap(),
        )
        .unwrap();
        // Every saved input is verified before the native address transaction publishes.
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(bundle.join("manifest.json")).unwrap()).unwrap();
        for name in ["descriptor", "library"] {
            let path = bundle.join(manifest["packages"][0][name].as_str().unwrap());
            let original = fs::read(&path).unwrap();
            let mut changed = original.clone();
            changed[0] ^= 1;
            let saved = path.with_extension("untampered");
            fs::rename(&path, &saved).unwrap();
            fs::write(&path, changed).unwrap();
            let rejected = Command::new(&built.executable).output().unwrap();
            assert_eq!(rejected.status.code(), Some(78));
            assert!(!String::from_utf8_lossy(&rejected.stderr).contains("RINZU_RESULT="));
            fs::remove_file(&path).unwrap();
            fs::rename(&saved, &path).unwrap();
        }
        let sanitized = built.executable.parent().unwrap().join("sanitized");
        let native = Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime/src");
        let linked = Command::new("clang")
            .args([
                "-std=c11",
                "-O1",
                "-g",
                "-fsanitize=address,undefined",
                "-fno-omit-frame-pointer",
                "-pthread",
                "-I",
            ])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
            .arg(&built.object)
            .arg(with_suffix(&built.executable, ".runtime.c"))
            .arg(native.join("callback_entry.c"))
            .arg(native.join("callback_template.S"))
            .arg("-o")
            .arg(&sanitized)
            .output()
            .unwrap();
        assert!(
            linked.status.success(),
            "{}",
            String::from_utf8_lossy(&linked.stderr)
        );
        let sanitized = Command::new(&sanitized)
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .output()
            .unwrap();
        assert!(
            sanitized.status.success(),
            "{}",
            String::from_utf8_lossy(&sanitized.stderr)
        );
        assert!(!String::from_utf8_lossy(&sanitized.stderr).contains("runtime error:"));
        compare_source_async_values_and_transport(
            &jit,
            std::str::from_utf8(&sanitized.stderr).unwrap(),
        )
        .unwrap();
        // Cancellation is observed at managed polls after a complete physical native loop.
        fs::write(app.join("src/main.rinzu"), r#"import measure;
            fn work() -> Int { let measured = measure.measure(b"A\xff", "日本語\u{0}🙂"); var index = 0; while index < 1000 { index = index + 1; } return measured; }
            async fn main() { let returned = await spawn work(); }
        "#).unwrap();
        let (_, artifact) = rinzu_compiler::check_package(&app).unwrap();
        let cancelled_build = build_package(&app, Some(app.join("build/cancelled"))).unwrap();
        let cancelled_bundle = with_suffix(&cancelled_build.executable, ".source-packages");
        let mut after_return = false;
        for limit in [1, 3, 5, 10, 20, 100] {
            let jit = rinzu_codegen::run_source_async_report(
                &artifact.ir,
                "main",
                rinzu_codegen::SourceAsyncOptions {
                    gc_stress: true,
                    source_packages: Some(
                        crate::source_package_bundle::load(&cancelled_bundle.join("manifest.json"))
                            .unwrap(),
                    ),
                    cancel_worker_after_polls: Some(limit),
                    record_transport_evidence: true,
                    ..Default::default()
                },
            )
            .unwrap();
            let aot = Command::new(&cancelled_build.executable)
                .env("RINZU_CANCEL_WORKER_AFTER_POLLS", limit.to_string())
                .env("RINZU_TRANSPORT_EVIDENCE", "1")
                .output()
                .unwrap();
            assert!(!aot.status.success());
            let stdout = std::str::from_utf8(&aot.stderr).unwrap().to_owned();
            compare_source_async_values_and_transport(&jit, &stdout)
                .unwrap_or_else(|error| panic!("limit={limit}: {error}\n{stdout}"));
            assert_eq!(parse_line(&stdout, "RINZU_FAILURE=").unwrap(), "cancelled");
            after_return |= jit.native_calls.callback_entries > 0;
        }
        assert!(after_return);
        fs::write(app.join("src/main.rinzu"), r#"import measure;
            async fn main() {
                let before = measure.calls(); var round = 0;
                while round < 180 { if measure.measure(b"A\xff", "日本語\u{0}🙂") != 13 { panic("wrong"); } round = round + 1; }
                if measure.calls() != before + 180 { panic("countFailure"); }
            }
        "#).unwrap();
        let (_, growth_artifact) = rinzu_compiler::check_package(&app).unwrap();
        let growth = build_package(&app, Some(app.join("build/growth"))).unwrap();
        let growth_bundle = with_suffix(&growth.executable, ".source-packages");
        let jit = rinzu_codegen::run_source_async_report(
            &growth_artifact.ir,
            "main",
            rinzu_codegen::SourceAsyncOptions {
                gc_stress: true,
                source_packages: Some(
                    crate::source_package_bundle::load(&growth_bundle.join("manifest.json"))
                        .unwrap(),
                ),
                record_transport_evidence: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(jit.native_calls.callback_entries, 180 * 96);
        let signed = Command::new("codesign")
            .args([
                "--force",
                "--sign",
                "-",
                "--options",
                "runtime",
                "--entitlements",
            ])
            .arg(&local_entitlements)
            .arg(&growth.executable)
            .output()
            .unwrap();
        assert!(signed.status.success());
        let output = Command::new(&growth.executable)
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = std::str::from_utf8(&output.stderr).unwrap().to_owned();
        compare_source_async_values_and_transport(&jit, &stdout).unwrap();
        let entries =
            parse_resource_numbers(parse_line(&stdout, "RINZU_NATIVE_ENTRIES=").unwrap()).unwrap();
        assert_eq!(
            (entries[0], entries[1], entries[4], entries[5]),
            (540, 2, 0, 0)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn seeded_composite_workers_repeat_on_real_jit_aot_heaps_and_under_sanitizers() {
        const SEEDS: [u64; 4] = [1, 0x5eed, 0xdecafbad, 0x7fff_ffff_ffff_ffff];
        const AWAITS: [&str; 7] = [
            "let nan_copy = await received_nan;",
            "let byte_copy = await received_bytes;",
            "let text_copy = await received_text;",
            "let map_copy = await received_map;",
            "let list_copy = await received_list;",
            "let record_copy = await copy;",
            "let payload_copy = await first;",
        ];
        let fixture = include_str!("../../codegen/tests/fixtures/worker_composite.rinzu");
        let original_awaits = format!(
            "{}\n    if nan_copy == nan_copy {{\n        panic(\"bad_nan\");\n    }}\n    {}",
            AWAITS[0],
            AWAITS[1..].join("\n    ")
        );
        assert!(fixture.contains(&original_awaits));
        let directory = std::env::temp_dir().join(format!(
            "rinzu-composite-worker-seeds-{}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        for seed in SEEDS {
            let mut random = seed;
            let mut awaits = AWAITS;
            for index in (1..awaits.len()).rev() {
                random = random
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                awaits.swap(index, (random % (index as u64 + 1)) as usize);
            }
            let shuffled = fixture.replace(
                &original_awaits,
                &format!(
                    "{}\n    if nan_copy == nan_copy {{ panic(\"bad_nan\"); }}",
                    awaits.join("\n    ")
                ),
            );
            let successful = format!(
                "{}\nasync fn main() {{ var iteration=0; while iteration<8 {{ let round=await repeated(); iteration=iteration+1; }} }}",
                shuffled.replace("async fn main()", "async fn repeated()")
            );
            let cancelled = format!(
                "{}\n{}",
                shuffled.replace("async fn main()", "async fn successful_main()"),
                include_str!("../../codegen/tests/fixtures/worker_composite_cancel.rinzu")
            );
            for (source, cancel) in [
                (successful, None),
                (cancelled, Some(100 + (seed % 53) as usize)),
            ] {
                let case = format!(
                    "{seed:016x}-{}",
                    if cancel.is_some() {
                        "cancel"
                    } else {
                        "success"
                    }
                );
                fs::write(directory.join(format!("{case}.rinzu")), &source).unwrap();
                let artifact = rinzu_compiler::compile_source(&rinzu_compiler::SourceFile::new(
                    rinzu_compiler::FileId(1),
                    "worker-composite-seeded.rinzu",
                    source,
                ))
                .unwrap();
                let object =
                    emit_source_async_aot_object(&artifact.ir, "main", &HashMap::new()).unwrap();
                let plan = async_aot_plan(&artifact, &object).unwrap().unwrap();
                fs::write(directory.join("program.o"), &object.object).unwrap();
                fs::write(
                    directory.join("runtime.c"),
                    runtime_shim(&object, &[], Some(&plan), &[]),
                )
                .unwrap();
                let compile = Command::new("clang")
                    .args([
                        "-std=c11",
                        "-O1",
                        "-g",
                        "-fsanitize=address,undefined",
                        "-pthread",
                        "-I",
                    ])
                    .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
                    .arg(directory.join("runtime.c"))
                    .arg(directory.join("program.o"))
                    .arg("-o")
                    .arg(directory.join("program"))
                    .output()
                    .unwrap();
                assert!(
                    compile.status.success(),
                    "seed={seed:x} {}",
                    String::from_utf8_lossy(&compile.stderr)
                );
                // Three fresh schedules for each source retain the same seed/source;
                // actual completion interleavings are checked by the credit ledger.
                for repetition in 0..3 {
                    let jit = rinzu_codegen::run_source_async_report(
                        &artifact.ir,
                        "main",
                        rinzu_codegen::SourceAsyncOptions {
                            gc_stress: true,
                            cancel_worker_after_polls: cancel,
                            record_transport_evidence: true,
                            ..Default::default()
                        },
                    )
                    .unwrap();
                    assert!(
                        jit.resources_returned && jit.heap.objects == 0 && jit.heap.roots == 0,
                        "seed={seed:x} repetition={repetition} {jit:?}"
                    );
                    assert_eq!(
                        jit.worker_heaps.len(),
                        if cancel.is_some() { 1 } else { 72 }
                    );
                    let mut command = Command::new(directory.join("program"));
                    command
                        .env("RINZU_TRANSPORT_EVIDENCE", "1")
                        .env_remove("RINZU_CANCEL_AFTER_POLLS")
                        .env_remove("RINZU_CANCEL_WORKER_AFTER_POLLS");
                    if let Some(cancel) = cancel {
                        command.env("RINZU_CANCEL_WORKER_AFTER_POLLS", cancel.to_string());
                    }
                    let output = command.output().unwrap();
                    let stdout = std::str::from_utf8(&output.stderr).unwrap().to_owned();
                    fs::write(
                        directory.join(format!("{case}-{repetition}.ledger")),
                        &stdout,
                    )
                    .unwrap();
                    assert_eq!(
                        output.status.success(),
                        jit.failure_details.is_none(),
                        "seed={seed:x} repetition={repetition} {stdout}\n{}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                    assert!(
                        !String::from_utf8_lossy(&output.stderr).contains("Sanitizer")
                            && !String::from_utf8_lossy(&output.stderr).contains("runtime error:")
                    );
                    compare_source_async_values_and_transport(&jit, &stdout).unwrap_or_else(
                        |error| panic!("seed={seed:x} repetition={repetition} {error}"),
                    );
                    assert_eq!(
                        parse_line(&stdout, "RINZU_DIAGNOSTICS=").unwrap(),
                        diagnostic_signature(jit.failure_details.as_ref()),
                        "seed={seed:x} repetition={repetition}"
                    );
                }
            }
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn async_wait_cycle_diagnoses_actual_continuation_dependencies_before_resume_or_cleanup() {
        // The appended C fixture supplies every task and the actual main. Keep
        // the unused generated entry empty so unrelated worker cleanup blocks
        // do not introduce unreferenced labels under this fixture's -Werror.
        let artifact = rinzu_compiler::compile_source(&rinzu_compiler::SourceFile::new(
            rinzu_compiler::FileId(1),
            "wait-cycle.rinzu",
            "async fn main(){}",
        ))
        .unwrap();
        let object = emit_source_async_aot_object(&artifact.ir, "main", &HashMap::new()).unwrap();
        let plan = async_aot_plan(&artifact, &object).unwrap().unwrap();
        let mut source = runtime_shim(&object, &[], Some(&plan), &[])
            .replace("int main(", "int unused_source_main(");
        source.push_str(include_str!("../tests/fixtures/async_wait_cycle.c"));
        let root =
            std::env::temp_dir().join(format!("rinzu-async-wait-model-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("program.o"), &object.object).unwrap();
        fs::write(root.join("runtime.c"), source).unwrap();
        for sanitized in [false, true] {
            let binary = root.join(if sanitized { "sanitized" } else { "signed" });
            let mut clang = Command::new("clang");
            clang
                .args([
                    "-std=c11", "-O1", "-g", "-Wall", "-Wextra", "-Werror", "-pthread", "-I",
                ])
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
                .arg(root.join("runtime.c"))
                .arg(root.join("program.o"));
            if sanitized {
                clang.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
            }
            let compiled = clang.arg("-o").arg(&binary).output().unwrap();
            assert!(compiled.status.success(), "{compiled:?}");
            if !sanitized {
                for args in [
                    ["--force", "--sign", "-", "--options", "runtime"].as_slice(),
                    ["--verify", "--strict"].as_slice(),
                ] {
                    let signed = Command::new("codesign")
                        .args(args)
                        .arg(&binary)
                        .output()
                        .unwrap();
                    assert!(signed.status.success(), "{signed:?}");
                }
                let entitlements = Command::new("codesign")
                    .args(["-d", "--entitlements", "-"])
                    .arg(&binary)
                    .output()
                    .unwrap();
                assert!(entitlements.status.success() && entitlements.stdout.is_empty());
            }
            let run = |mode| {
                Command::new(&binary)
                    .arg(mode)
                    .env("ASAN_OPTIONS", "detect_leaks=0")
                    .env("UBSAN_OPTIONS", "halt_on_error=1")
                    .output()
                    .unwrap()
            };
            for mode in ["normal", "pump"] {
                let normal = run(mode);
                assert!(normal.status.success(), "{mode} {normal:?}");
                assert_eq!(
                    String::from_utf8_lossy(&normal.stdout).trim(),
                    "ASYNC_WAIT_MODEL=baseline"
                );
            }
            for mode in ["self", "cycle"] {
                let negative = run(mode);
                assert_eq!(negative.status.code(), Some(86), "{negative:?}");
                assert!(negative.stdout.is_empty(), "{negative:?}");
                let diagnostic = String::from_utf8_lossy(&negative.stderr);
                assert!(
                    diagnostic.contains("observable-wait-cycle")
                        && diagnostic.contains("source=17:31:49 owner=0x"),
                    "{diagnostic}"
                );
                assert_eq!(
                    diagnostic
                        .lines()
                        .filter(|line| line.starts_with("rinzu wait: "))
                        .count(),
                    if mode == "self" { 1 } else { 3 }
                );
                assert!(!diagnostic.contains("runtime error:"), "{diagnostic}");
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn unrelated_result_pin_survives_future_zero_gc_and_consuming_await() {
        let artifact = rinzu_compiler::compile_source(&rinzu_compiler::SourceFile::new(
            rinzu_compiler::FileId(1),
            "async-result-pin-identity.rinzu",
            "async fn main() {}",
        ))
        .unwrap();
        let object = emit_source_async_aot_object(&artifact.ir, "main", &HashMap::new()).unwrap();
        let plan = async_aot_plan(&artifact, &object).unwrap().unwrap();
        let mut source = runtime_shim(&object, &[], Some(&plan), &[])
            .replace("int main(", "int unused_source_main(");
        source.push_str(include_str!(
            "../tests/fixtures/async_result_pin_identity.c"
        ));
        let directory = std::env::temp_dir().join(format!(
            "rinzu-async-result-pin-identity-{}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("program.o"), &object.object).unwrap();
        fs::write(directory.join("runtime.c"), source).unwrap();
        for flags in [
            vec!["-O2", "-Wall", "-Wextra", "-Werror"],
            vec!["-O1", "-g", "-fsanitize=address,undefined"],
        ] {
            let compile = Command::new("clang")
                .args(["-std=c11", "-pthread", "-I"])
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
                .args(flags)
                .arg(directory.join("runtime.c"))
                .arg(directory.join("program.o"))
                .arg("-o")
                .arg(directory.join("program"))
                .output()
                .unwrap();
            assert!(compile.status.success(), "{compile:?}");
            let output = Command::new(directory.join("program"))
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .env("UBSAN_OPTIONS", "halt_on_error=1")
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            assert_eq!(
                String::from_utf8_lossy(&output.stdout).trim(),
                "ASYNC_RESULT_PIN_IDENTITY=4:baseline"
            );
            assert!(
                !String::from_utf8_lossy(&output.stderr).contains("Sanitizer")
                    && !String::from_utf8_lossy(&output.stderr).contains("runtime error:")
            );
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn typed_worker_faults_and_late_returns_settle_actual_heaps_and_native_allocations() {
        let artifact = rinzu_compiler::compile_source(&rinzu_compiler::SourceFile::new(
            rinzu_compiler::FileId(1),
            "typed-worker-resources.rinzu",
            include_str!("../tests/fixtures/typed_worker_resources.rinzu"),
        ))
        .unwrap();
        let object = emit_source_async_aot_object(&artifact.ir, "main", &HashMap::new()).unwrap();
        let plan = async_aot_plan(&artifact, &object).unwrap().unwrap();
        let worker = |name: &str| {
            let index = object
                .functions
                .iter()
                .find(|entry| entry.name == name)
                .unwrap()
                .index;
            plan.workers
                .iter()
                .find(|entry| entry.index == index)
                .unwrap()
        };
        let mut source = String::from(
            r#"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdatomic.h>
static _Atomic size_t model_native_live;
static void *model_calloc(size_t count, size_t size) {
  void *memory = calloc(count,size); if (memory) atomic_fetch_add(&model_native_live,1); return memory;
}
static inline void *model_malloc(size_t size) {
  void *memory = malloc(size); if (memory) atomic_fetch_add(&model_native_live,1); return memory;
}
static void model_free(void *memory) {
  if (memory) { assert(atomic_fetch_sub(&model_native_live,1)); free(memory); }
}
#define calloc model_calloc
#define malloc model_malloc
#define free model_free
"#,
        );
        source.push_str(
            &runtime_shim(&object, &[], Some(&plan), &[])
                .replace("int main(", "int unused_source_main("),
        );
        source.push_str(&format!(
            "\n#define MODEL_ECHO {}\n#define MODEL_ECHO_RUN rinzu_worker_run_{}\n#define MODEL_EXPANDED {}\n#define MODEL_OBSERVED {}\n#define MODEL_PACKET_TYPE {}\n",
            worker("echo").index, worker("echo").index, worker("expanded").index,
            worker("observed").index, worker("echo").parameters[0]
        ));
        source.push_str(include_str!("../tests/fixtures/typed_worker_resources.c"));
        let directory = std::env::temp_dir().join(format!(
            "rinzu-typed-worker-resources-{}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("program.o"), &object.object).unwrap();
        fs::write(directory.join("runtime.c"), source).unwrap();
        for flags in [
            vec!["-O2", "-Wall", "-Wextra", "-Werror"],
            vec!["-O1", "-g", "-fsanitize=address,undefined"],
        ] {
            let compile = Command::new("clang")
                .args(["-std=c11", "-pthread", "-I"])
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
                .args(flags)
                .arg(directory.join("runtime.c"))
                .arg(directory.join("program.o"))
                .arg("-o")
                .arg(directory.join("program"))
                .output()
                .unwrap();
            assert!(
                compile.status.success(),
                "{}",
                String::from_utf8_lossy(&compile.stderr)
            );
            let output = Command::new(directory.join("program")).output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                std::str::from_utf8(&output.stdout).unwrap().trim(),
                "TYPED_WORKER_RESOURCES=96:baseline"
            );
            assert!(
                !String::from_utf8_lossy(&output.stderr).contains("Sanitizer")
                    && !String::from_utf8_lossy(&output.stderr).contains("runtime error:")
            );
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn typed_worker_heap_transport_runs_under_address_and_undefined_sanitizers() {
        let fixture = include_str!("../../codegen/tests/fixtures/worker_composite.rinzu");
        let cancelled = format!(
            "{}\n{}",
            fixture.replace("async fn main()", "async fn successful_main()"),
            include_str!("../../codegen/tests/fixtures/worker_composite_cancel.rinzu")
        );
        let references = format!(
            "{}\nfn verified() -> Int {{ try {{ return verify(); }} catch unexpected {{ panic(\"unexpected failure\"); return 0; }} }} async fn forwarded() -> Int {{ return await spawn verified(); }} async fn main() {{ try {{ let on_main=verify(); let reference=forwarded; let pending=reference(); let on_worker=await pending; if on_main!=42 || on_worker!=42 {{ panic(\"bad_result\"); }} }} catch unexpected {{ panic(\"failed\"); }} }}",
            include_str!("../../codegen/tests/fixtures/function_reference.rinzu")
                .replace("fn main()", "fn unused_main()")
        );
        let directory = std::env::temp_dir().join(format!(
            "rinzu-typed-worker-sanitized-{}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        for (source, cancel) in [
            (fixture.to_owned(), None),
            (cancelled, Some(100)),
            (references, None),
        ] {
            let artifact = rinzu_compiler::compile_source(&rinzu_compiler::SourceFile::new(
                rinzu_compiler::FileId(1),
                "worker-composite-sanitized.rinzu",
                source,
            ))
            .unwrap();
            let jit = rinzu_codegen::run_source_async_report(
                &artifact.ir,
                "main",
                rinzu_codegen::SourceAsyncOptions {
                    gc_stress: true,
                    cancel_worker_after_polls: cancel,
                    record_transport_evidence: true,
                    ..Default::default()
                },
            )
            .unwrap();
            let object =
                emit_source_async_aot_object(&artifact.ir, "main", &HashMap::new()).unwrap();
            let plan = async_aot_plan(&artifact, &object).unwrap().unwrap();
            fs::write(directory.join("program.o"), &object.object).unwrap();
            fs::write(
                directory.join("runtime.c"),
                runtime_shim(&object, &[], Some(&plan), &[]),
            )
            .unwrap();
            let compile = Command::new("clang")
                .args([
                    "-std=c11",
                    "-O1",
                    "-g",
                    "-fsanitize=address,undefined",
                    "-pthread",
                    "-I",
                ])
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
                .arg(directory.join("runtime.c"))
                .arg(directory.join("program.o"))
                .arg("-o")
                .arg(directory.join("program"))
                .output()
                .unwrap();
            assert!(
                compile.status.success(),
                "{}",
                String::from_utf8_lossy(&compile.stderr)
            );
            let mut command = Command::new(directory.join("program"));
            command
                .env("RINZU_TRANSPORT_EVIDENCE", "1")
                .env_remove("RINZU_CANCEL_AFTER_POLLS")
                .env_remove("RINZU_CANCEL_WORKER_AFTER_POLLS");
            if let Some(cancel) = cancel {
                command.env("RINZU_CANCEL_WORKER_AFTER_POLLS", cancel.to_string());
            }
            let output = command.output().unwrap();
            let stdout = std::str::from_utf8(&output.stderr).unwrap().to_owned();
            assert_eq!(
                output.status.success(),
                jit.failure_details.is_none(),
                "{stdout}\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                !String::from_utf8_lossy(&output.stderr).contains("Sanitizer")
                    && !String::from_utf8_lossy(&output.stderr).contains("runtime error:"),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            compare_source_async_values_and_transport(&jit, &stdout).unwrap();
            assert_eq!(
                parse_line(&stdout, "RINZU_DIAGNOSTICS=").unwrap(),
                diagnostic_signature(jit.failure_details.as_ref())
            );
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn async_trace_preserves_task_future_and_native_order_across_interleavings() {
        use rinzu_trace::{EventKind, NativeTransition, TraceEvent};
        let expected = TraceDocument {
            schema: rinzu_trace::TRACE_SCHEMA.into(),
            version: rinzu_trace::TRACE_VERSION,
            producer: "test".into(),
            context: BTreeMap::new(),
            events: [
                (
                    "main",
                    EventKind::NativeEnter {
                        transition: NativeTransition::NativeSafe,
                    },
                ),
                (
                    "main:1",
                    EventKind::AwaitPending {
                        operation: "file-read-size".into(),
                    },
                ),
                (
                    "main:1",
                    EventKind::AwaitReady {
                        operation: "future:3".into(),
                    },
                ),
                (
                    "main",
                    EventKind::NativeLeave {
                        transition: NativeTransition::NativeSafe,
                    },
                ),
            ]
            .into_iter()
            .enumerate()
            .map(|(sequence, (task, event))| TraceEvent {
                sequence: sequence as u64,
                domain: "main".into(),
                task: Some(task.into()),
                source: None,
                event,
                observation: None,
            })
            .collect(),
        };
        let mut actual = expected.clone();
        actual.events.swap(2, 3);
        for (sequence, event) in actual.events.iter_mut().enumerate() {
            event.sequence = sequence as u64;
        }
        actual.validate().unwrap();
        compare_async_task_traces(&expected, &actual).unwrap();
        let mut already_ready = actual.clone();
        already_ready
            .events
            .retain(|event| !matches!(event.event, EventKind::AwaitPending { .. }));
        compare_async_task_traces(&expected, &already_ready).unwrap();
        let mut changed = actual.clone();
        changed.events[3].event = EventKind::AwaitReady {
            operation: "future:4".into(),
        };
        assert!(compare_async_task_traces(&expected, &changed).is_err());
        let mut changed = actual.clone();
        changed.events.swap(0, 2);
        assert!(compare_async_task_traces(&expected, &changed).is_err());
        let mut changed = actual.clone();
        changed.events[3].task = Some("main:2".into());
        assert!(compare_async_task_traces(&expected, &changed).is_err());
        let mut changed = actual.clone();
        changed.events[3].domain = "worker".into();
        assert!(compare_async_task_traces(&expected, &changed).is_err());
        actual.events.pop();
        assert!(compare_async_task_traces(&expected, &actual).is_err());
    }

    #[test]
    fn await_evidence_allows_task_interleaving_but_keeps_each_task_future_and_order_exact() {
        let source = rinzu_compiler::SourceFile::new(
            rinzu_compiler::FileId(1),
            "tasks.rinzu",
            "async fn ready(value:CInt)->CInt{return value;} async fn child(value:CInt)->CInt{let result=await ready(value);return result;} async fn main(){let first=child(7);let second=child(9);let a=await first;let b=await second;}",
        );
        let artifact = rinzu_compiler::compile_source(&source).unwrap();
        let run = rinzu_codegen::run_source_async_report(
            &artifact.ir,
            "main",
            rinzu_codegen::SourceAsyncOptions::default(),
        )
        .unwrap();
        let report = |tasks: &BTreeMap<u64, Vec<(u64, rinzu_runtime::scalar::ScalarValue)>>| {
            let values = tasks
                .iter()
                .flat_map(|(scope, values)| {
                    values.iter().map(move |(future, value)| {
                        (
                            *scope,
                            *future,
                            format!("{}:{:016x}", value.kind() as u32, value.bits()),
                        )
                    })
                })
                .collect::<Vec<_>>();
            format!(
                "RINZU_AWAITED_VALUES={}\nRINZU_AWAITED_TASK_VALUES={}\n",
                values
                    .iter()
                    .map(|(_, _, value)| value.clone())
                    .collect::<Vec<_>>()
                    .join(","),
                values
                    .iter()
                    .map(|(scope, future, value)| format!("{scope}:{future}:{value}"))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        };
        let regrouped = report(&run.awaited_tasks);
        compare_awaited_values(&run, &regrouped).unwrap();
        let mut reordered = run.awaited_tasks.clone();
        reordered.get_mut(&0).unwrap().reverse();
        assert!(compare_awaited_values(&run, &report(&reordered)).is_err());
        let mut wrong_future = run.awaited_tasks.clone();
        wrong_future.get_mut(&0).unwrap()[0].0 += 1;
        assert!(compare_awaited_values(&run, &report(&wrong_future)).is_err());
        let mut omitted = run.awaited_tasks.clone();
        omitted.remove(&0);
        assert!(compare_awaited_values(&run, &report(&omitted)).is_err());
    }

    #[test]
    fn file_close_race_keeps_explicit_observation_and_other_cancellation_exact() {
        use rinzu_codegen::SourceAsyncEvent;
        use rinzu_runtime::failure::FailureCause;
        let source = rinzu_compiler::SourceFile::new(
            rinzu_compiler::FileId(1),
            "file-close-evidence.rinzu",
            "async fn main() { panic(\"failure\"); }",
        );
        let artifact = rinzu_compiler::compile_source(&source).unwrap();
        let mut run = rinzu_codegen::run_source_async_report(
            &artifact.ir,
            "main",
            rinzu_codegen::SourceAsyncOptions::default(),
        )
        .unwrap();
        run.events.push(SourceAsyncEvent::Submitted {
            future: 0,
            operation: "file-read-size".into(),
        });
        run.scope_cancellation_requests.push(0);
        let completed = std::collections::BTreeSet::new();
        let cancelled = std::collections::BTreeSet::from([0]);
        compare_cancelled_children(&run, &cancelled).unwrap();
        run.cancelled_children.push(0);
        compare_cancelled_children(&run, &completed).unwrap();
        assert!(compare_cancelled_children(&run, &std::collections::BTreeSet::from([1])).is_err());

        for operation in ["spawn", "timer"] {
            let mut changed = run.clone();
            changed.events = vec![SourceAsyncEvent::Submitted {
                future: 0,
                operation: operation.into(),
            }];
            assert!(compare_cancelled_children(&changed, &completed).is_err());
        }
        let mut changed = run.clone();
        changed.scope_cancellation_requests.clear();
        assert!(compare_cancelled_children(&changed, &completed).is_err());
        let mut changed = run.clone();
        changed.control_attempts.push(0);
        assert!(compare_cancelled_children(&changed, &completed).is_err());
        let mut changed = run.clone();
        changed.events.push(SourceAsyncEvent::Awaited {
            scope: 0,
            future: 0,
        });
        assert!(compare_cancelled_children(&changed, &completed).is_err());
        let mut changed = run.clone();
        changed.events.push(SourceAsyncEvent::Suspended {
            scope: 0,
            future: 0,
            operation: "file-read-size".into(),
            owner_thread: std::thread::current().id(),
        });
        assert!(compare_cancelled_children(&changed, &completed).is_err());
        let mut changed = run.clone();
        changed.failure_details = None;
        assert!(compare_cancelled_children(&changed, &completed).is_err());
        let mut changed = run.clone();
        changed.failure_details.as_mut().unwrap().primary.cause = FailureCause::Cancelled;
        assert!(compare_cancelled_children(&changed, &completed).is_err());
    }
    #[test]
    fn native_unit_race_requires_cancelled_ancestor_and_no_outside_observer() {
        use rinzu_codegen::{FutureAdmission, SourceAsyncEvent};
        let artifact = rinzu_compiler::compile_source(&rinzu_compiler::SourceFile::new(
            rinzu_compiler::FileId(1),
            "native-race.rinzu",
            "async fn main(){} ",
        ))
        .unwrap();
        let mut run =
            rinzu_codegen::run_source_async_report(&artifact.ir, "main", Default::default())
                .unwrap();
        run.future_admissions = vec![
            FutureAdmission {
                future: 10,
                parent_scope: 0,
                ordinal: 0,
            },
            FutureAdmission {
                future: 11,
                parent_scope: 11,
                ordinal: 0,
            },
        ];
        run.cancelled_children = vec![10];
        run.control_attempts = vec![10];
        run.scope_cancellation_requests = vec![11];
        run.events = vec![
            SourceAsyncEvent::Submitted {
                future: 11,
                operation: "native-completion".into(),
            },
            SourceAsyncEvent::Suspended {
                scope: 11,
                future: 11,
                operation: "native-completion".into(),
                owner_thread: std::thread::current().id(),
            },
        ];
        let actual = std::collections::BTreeSet::from([10, 11]);
        assert_eq!(
            compare_cancelled_children(&run, &actual).unwrap(),
            std::collections::BTreeSet::from([11])
        );
        let mut changed = run.clone();
        changed.events.push(SourceAsyncEvent::Awaited {
            scope: 0,
            future: 11,
        });
        assert!(compare_cancelled_children(&changed, &actual).is_err());
        changed = run.clone();
        changed.control_attempts.push(11);
        assert!(compare_cancelled_children(&changed, &actual).is_err());
        changed = run.clone();
        changed.scope_cancellation_requests.clear();
        assert!(compare_cancelled_children(&changed, &actual).is_err());
        changed = run.clone();
        changed.resources_returned = false;
        assert!(compare_cancelled_children(&changed, &actual).is_err());
        changed = run.clone();
        changed.future_admissions[1].parent_scope = 0;
        assert!(compare_cancelled_children(&changed, &actual).is_err());
        assert!(compare_cancelled_children(&run, &std::collections::BTreeSet::from([11])).is_err());
    }

    #[test]
    fn admission_close_race_requires_unobserved_workers_and_real_scope_cancellation() {
        use rinzu_codegen::SourceAsyncEvent;
        use rinzu_runtime::failure::FailureCause;
        let source = rinzu_compiler::SourceFile::new(
            rinzu_compiler::FileId(1),
            "admission-close-evidence.rinzu",
            "async fn main() { panic(\"failure\"); }",
        );
        let artifact = rinzu_compiler::compile_source(&source).unwrap();
        let mut run = rinzu_codegen::run_source_async_report(
            &artifact.ir,
            "main",
            rinzu_codegen::SourceAsyncOptions::default(),
        )
        .unwrap();
        run.failure_details.as_mut().unwrap().primary.cause = FailureCause::Overloaded;
        run.events = vec![SourceAsyncEvent::Submitted {
            future: 0,
            operation: "spawn".into(),
        }];
        run.scope_cancellation_requests.push(0);
        let completed = std::collections::BTreeSet::new();
        let cancelled = std::collections::BTreeSet::from([0]);
        compare_cancelled_children(&run, &cancelled).unwrap();
        run.cancelled_children.push(0);
        compare_cancelled_children(&run, &completed).unwrap();
        assert!(compare_cancelled_children(&run, &std::collections::BTreeSet::from([1])).is_err());
        let mut received = run.clone();
        received.scope_cancellation_requests = vec![1];
        assert!(compare_cancelled_children(&received, &completed).is_err());
        received.events.push(SourceAsyncEvent::WorkerCompleted {
            future: 0,
            domain: 1,
            worker_thread: std::thread::current().id(),
        });
        compare_cancelled_children(&received, &completed).unwrap();
        for cause in [FailureCause::Cancelled, FailureCause::ArithmeticTrap] {
            let mut changed = run.clone();
            changed.failure_details.as_mut().unwrap().primary.cause = cause;
            assert!(compare_cancelled_children(&changed, &completed).is_err());
        }
        let mut changed = run.clone();
        changed.scope_cancellation_requests.clear();
        assert!(compare_cancelled_children(&changed, &completed).is_err());
        let mut changed = run.clone();
        changed.control_attempts.push(0);
        assert!(compare_cancelled_children(&changed, &completed).is_err());
        for event in [
            SourceAsyncEvent::Awaited {
                scope: 0,
                future: 0,
            },
            SourceAsyncEvent::Suspended {
                scope: 0,
                future: 0,
                operation: "spawn".into(),
                owner_thread: std::thread::current().id(),
            },
        ] {
            let mut changed = run.clone();
            changed.events.push(event);
            assert!(compare_cancelled_children(&changed, &completed).is_err());
        }
    }

    #[test]
    fn implicit_close_races_validate_transport_and_heaps_in_both_directions() {
        use rinzu_codegen::SourceAsyncEvent;
        use rinzu_runtime::failure::{Failure, FailureCause};
        use rinzu_runtime::transport::TransportDirection;
        let source = rinzu_compiler::SourceFile::new(
            rinzu_compiler::FileId(1),
            "close-packet-evidence.rinzu",
            "fn echo(value:CUInt64)->CUInt64{return value;} async fn main(){let child=spawn echo(7);}",
        );
        let artifact = rinzu_compiler::compile_source(&source).unwrap();
        let mut run = rinzu_codegen::run_source_async_report(
            &artifact.ir,
            "main",
            rinzu_codegen::SourceAsyncOptions {
                gc_stress: true,
                record_transport_evidence: true,
                ..Default::default()
            },
        )
        .unwrap();
        run.failure_details = Some(Failure::new(FailureCause::Overloaded));
        run.scope_cancellation_requests.push(0);
        let completed = "RINZU_AWAITED_VALUES=\nRINZU_AWAITED_TASK_VALUES=\nRINZU_TRANSPORT=0,1,0,1,0,8\nRINZU_CREDITS=1:0:0:8:1:2;2:0:1:8:3:4\nRINZU_CANCELLED_CHILDREN=\nRINZU_STARTED_CHILDREN=1,0\nRINZU_STARTED_TIMERS=0\nRINZU_WORKER_HEAPS=0:0:0:2:0:2:1\n";
        let cancelled = completed
            .replace(";2:0:1:8:3:4", "")
            .replace(
                "RINZU_CANCELLED_CHILDREN=\n",
                "RINZU_CANCELLED_CHILDREN=0\n",
            )
            .replace("0:0:0:2:0:2:1", "0:0:0:1:0:1:0");
        compare_source_async_values_and_transport(&run, completed).unwrap();
        compare_source_async_values_and_transport(&run, &cancelled).unwrap();
        run.cancelled_children.push(0);
        run.events
            .retain(|event| !matches!(event, SourceAsyncEvent::WorkerCompleted { .. }));
        run.worker_heaps.clear();
        run.transport_evidence
            .retain(|credit| credit.direction == TransportDirection::Input);
        compare_source_async_values_and_transport(&run, completed).unwrap();
        compare_source_async_values_and_transport(&run, &cancelled).unwrap();
        run.failure_details = Some(Failure::new(FailureCause::Cancelled));
        assert!(compare_source_async_values_and_transport(&run, completed).is_err());
        run.root_cancellation_requested = true;
        compare_source_async_values_and_transport(&run, completed).unwrap();
        for corrupt in [
            completed.replace("1:0:0:8:1:2", "1:0:0:4:1:2"),
            completed.replace("1:0:0:8:1:2", "1:0:0:8:1:0"),
            completed.replace("0,1,0,1,0,8", "0,1,0,1,8,8"),
            completed.replace("0:0:0:2:0:2:1", "0:1:0:2:0:2:1"),
            completed.replace("RINZU_STARTED_CHILDREN=1,0", "RINZU_STARTED_CHILDREN=0,0"),
            completed.replace(
                "RINZU_CANCELLED_CHILDREN=\n",
                "RINZU_CANCELLED_CHILDREN=9\n",
            ),
        ] {
            assert!(
                compare_source_async_values_and_transport(&run, &corrupt).is_err(),
                "accepted corrupt evidence: {corrupt}"
            );
        }
        run.control_attempts.push(0);
        assert!(compare_source_async_values_and_transport(&run, completed).is_err());
        run.control_attempts.clear();
        run.events.push(SourceAsyncEvent::Suspended {
            scope: 0,
            future: 0,
            operation: "spawn".into(),
            owner_thread: std::thread::current().id(),
        });
        compare_source_async_values_and_transport(&run, completed).unwrap();
        run.events.push(SourceAsyncEvent::Awaited {
            scope: 0,
            future: 0,
        });
        assert!(compare_source_async_values_and_transport(&run, completed).is_err());
    }

    #[test]
    fn async_equivalence_rejects_changed_type_bits_order_and_resource_evidence() {
        let source = rinzu_compiler::SourceFile::new(
            rinzu_compiler::FileId(1),
            "evidence.rinzu",
            "fn echo(value: CFloat) -> CFloat { return value; } async fn ready() -> CUInt64 { return 18446744073709551615; } async fn main() { let high = await ready(); let zero = await spawn echo(-0.0); }",
        );
        let artifact = rinzu_compiler::compile_source(&source).unwrap();
        let jit = rinzu_codegen::run_source_async(
            &artifact.ir,
            "main",
            rinzu_codegen::SourceAsyncOptions {
                gc_stress: true,
                record_transport_evidence: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(jit.result, 0);
        let correct = "RINZU_AWAITED_VALUES=7:ffffffffffffffff,8:0000000080000000\nRINZU_AWAITED_TASK_VALUES=0:0:7:ffffffffffffffff,0:1:8:0000000080000000\nRINZU_TRANSPORT=0,1,0,1,0,4\nRINZU_CREDITS=1:1:0:4:1:2;2:1:1:4:3:4\nRINZU_CANCELLED_CHILDREN=\nRINZU_STARTED_CHILDREN=1,0\nRINZU_STARTED_TIMERS=0\nRINZU_WORKER_HEAPS=1:0:0:2:0:2:1\n";
        compare_source_async_values_and_transport(&jit, correct).unwrap();
        for corrupt in [
            correct.replace(
                "RINZU_AWAITED_TASK_VALUES=0:0:7:ffffffffffffffff,0:1:8:0000000080000000\n",
                "",
            ),
            correct.replace("0:1:8:0000000080000000", "1:1:8:0000000080000000"),
            correct.replace("0:1:8:0000000080000000", "0:2:8:0000000080000000"),
            correct.replace("8:0000000080000000", "9:0000000080000000"),
            correct.replace("8:0000000080000000", "8:0000000000000000"),
            correct.replace(
                "7:ffffffffffffffff,8:0000000080000000",
                "8:0000000080000000,7:ffffffffffffffff",
            ),
            correct.replace("0,1,0,1,0,4", "0,1,0,1,4,4"),
            correct.replace("0,1,0,1,0,4", "0,1,0,1,0,8"),
            correct.replace("1:1:0:4:1:2", "1:1:0:4:1:0"),
            correct.replace("2:1:1:4:3:4", "1:1:1:4:3:4"),
            correct.replace("RINZU_STARTED_CHILDREN=1,0", "RINZU_STARTED_CHILDREN=0,0"),
            correct.replace("RINZU_STARTED_TIMERS=0\n", ""),
            correct.replace("RINZU_STARTED_TIMERS=0", "RINZU_STARTED_TIMERS=1"),
            correct.replace(
                "RINZU_WORKER_HEAPS=1:0:0:2:0:2:1",
                "RINZU_WORKER_HEAPS=1:1:0:2:0:2:1",
            ),
            correct.replace(
                "RINZU_WORKER_HEAPS=1:0:0:2:0:2:1",
                "RINZU_WORKER_HEAPS=1:0:1:2:0:2:1",
            ),
            correct.replace("RINZU_WORKER_HEAPS=1:0:0:2:0:2:1", "RINZU_WORKER_HEAPS="),
            correct.replace(
                "RINZU_WORKER_HEAPS=1:0:0:2:0:2:1",
                "RINZU_WORKER_HEAPS=1:0:0:2:0:1:1",
            ),
        ] {
            assert!(
                compare_source_async_values_and_transport(&jit, &corrupt).is_err(),
                "accepted corrupt evidence: {corrupt}"
            );
        }
        let mut jit = jit;
        jit.heap.userdata = rinzu_runtime::data::NativeUserDataEvidence {
            created: 2,
            released: 2,
            ..Default::default()
        };
        jit.worker_heaps[0].heap.userdata = rinzu_runtime::data::NativeUserDataEvidence {
            created: 1,
            released: 1,
            ..Default::default()
        };
        let userdata = format!(
            "{correct}RINZU_NATIVE_USERDATA=0,0,0,0,2,2\nRINZU_WORKER_USERDATA=1:0:0:0:0:1:1\nRINZU_USERDATA_PROCESS=0,0,0\n"
        );
        compare_source_async_values_and_transport(&jit, &userdata).unwrap();
        for corrupt in [
            userdata.replace("0,0,0,0,2,2", "1,0,0,0,2,2"),
            userdata.replace("0,0,0,0,2,2", "0,1,0,0,2,2"),
            userdata.replace("0,0,0,0,2,2", "0,0,1,0,2,2"),
            userdata.replace("0,0,0,0,2,2", "0,0,0,1,2,2"),
            userdata.replace("0,0,0,0,2,2", "0,0,0,0,2,1"),
            userdata.replace("0,0,0,0,2,2", "0,0,0,0,1,1"),
            userdata.replace("RINZU_NATIVE_USERDATA=0,0,0,0,2,2\n", ""),
            userdata.replace("1:0:0:0:0:1:1", "1:0:1:0:0:1:1"),
            userdata.replace("1:0:0:0:0:1:1", "1:0:0:0:0:1:0"),
            userdata.replace("1:0:0:0:0:1:1", ""),
            userdata.replace("1:0:0:0:0:1:1", "2:0:0:0:0:1:1"),
            userdata.replace("1:0:0:0:0:1:1", "1:0:0:0:0:1:1;1:0:0:0:0:1:1"),
            userdata.replace("RINZU_USERDATA_PROCESS=0,0,0\n", ""),
            userdata.replace(
                "RINZU_USERDATA_PROCESS=0,0,0",
                "RINZU_USERDATA_PROCESS=4,4,160",
            ),
            userdata.replace(
                "RINZU_USERDATA_PROCESS=0,0,0",
                "RINZU_USERDATA_PROCESS=3,2,120",
            ),
            userdata.replace(
                "RINZU_USERDATA_PROCESS=0,0,0",
                "RINZU_USERDATA_PROCESS=3,3,96",
            ),
        ] {
            assert!(
                compare_source_async_values_and_transport(&jit, &corrupt).is_err(),
                "accepted corrupt userdata: {corrupt}"
            );
        }
    }

    #[test]
    fn callback_resource_equivalence_keeps_retention_and_temporary_baseline_separate() {
        let correct = "RINZU_NATIVE_ENTRIES=780,2,65536,157568,0,0\nRINZU_CALLBACK_PROCESS=780,124800,48\nRINZU_CALLBACK_TEMPORARY=0,0\n";
        validate_callback_resource_evidence(correct).unwrap();
        for broken in [
            correct.replace("780,124800,48", "779,124800,48"),
            correct.replace("780,124800,48", "780,124799,48"),
            correct.replace("780,124800,48", "780,124800,0"),
            correct.replace("0,0\n", "1,0\n"),
            correct.replace(
                "RINZU_CALLBACK_TEMPORARY=0,0",
                "RINZU_CALLBACK_TEMPORARY=0,64",
            ),
            correct.replace("RINZU_CALLBACK_TEMPORARY=0,0\n", ""),
            correct.replace("780,2,65536", "780,1,65536"),
        ] {
            assert!(
                validate_callback_resource_evidence(&broken).is_err(),
                "{broken}"
            );
        }
    }

    #[test]
    fn callback_resource_equivalence_accepts_retired_and_mixed_controls() {
        for (control, tombstone, retired, slots, released) in [
            (32768, 0, 780, 24960, 124800),
            (93568, 60800, 400, 12800, 64000),
        ] {
            let correct = format!(
                "RINZU_NATIVE_ENTRIES=780,2,65536,{control},0,0\nRINZU_CALLBACK_PROCESS=780,{tombstone},48\nRINZU_CALLBACK_TEMPORARY=0,0\nRINZU_CALLBACK_RETIREMENT={retired},{slots},{released}\n"
            );
            validate_callback_resource_evidence(&correct).unwrap();
            for broken in [
                correct.replace(&format!("{retired},{slots},{released}"), "781,24992,124960"),
                correct.replace(
                    &format!("{retired},{slots},{released}"),
                    &format!("{retired},1,{released}"),
                ),
                correct.replace(
                    &format!("{retired},{slots},{released}"),
                    &format!("{retired},{slots},0"),
                ),
                correct.replace(
                    &format!("RINZU_CALLBACK_PROCESS=780,{tombstone},48"),
                    "RINZU_CALLBACK_PROCESS=780,1,48",
                ),
            ] {
                assert!(
                    validate_callback_resource_evidence(&broken).is_err(),
                    "{broken}"
                );
            }
        }
    }

    #[test]
    fn native_equivalence_rejects_missing_return_copy_and_live_resource_evidence() {
        let evidence = rinzu_codegen::SourceNativeCallEvidence {
            prepared: 5,
            returned: 5,
            closed: 5,
            settled: 5,
            read: 5,
            released: 5,
            callback_entries: 96,
            copied_bytes: 416,
            ..Default::default()
        };
        let correct = "RINZU_NATIVE_CALLS=5,5,5,5,5,5,96,416,0,0,0\nRINZU_NATIVE_ABANDONED=0\nRINZU_CALLBACK_RETURNS=0,0,0,0\nRINZU_NATIVE_ENTRIES=0,0,0,0,0,0\nRINZU_CALLBACK_PROCESS=0,0,0\nRINZU_CALLBACK_TEMPORARY=0,0\n";
        compare_native_call_evidence(evidence, correct).unwrap();
        for broken in [
            "RINZU_NATIVE_CALLS=5,4,5,5,5,5,96,416,0,0,0\n",
            "RINZU_NATIVE_CALLS=5,5,5,5,5,5,95,416,0,0,0\n",
            "RINZU_NATIVE_CALLS=5,5,5,5,5,5,96,415,0,0,0\n",
            "RINZU_NATIVE_CALLS=5,5,5,5,5,5,96,416,0,1,0\n",
            "",
        ] {
            assert!(compare_native_call_evidence(evidence, broken).is_err());
        }
        for broken in ["", "1,0,0,24", "0,1,1,24", "0,0,1,0", "0,0,0,24"] {
            assert!(
                compare_native_call_evidence(
                    evidence,
                    &correct.replace("0,0,0,0\n", &format!("{broken}\n"))
                )
                .is_err()
            );
        }
    }

    #[test]
    fn native_equivalence_compares_abandonment_without_synthesizing_return() {
        let evidence = rinzu_codegen::SourceNativeCallEvidence {
            prepared: 3,
            returned: 2,
            closed: 2,
            settled: 2,
            read: 2,
            released: 2,
            abandoned: 1,
            ..Default::default()
        };
        let correct = "RINZU_NATIVE_CALLS=3,2,2,2,2,2,0,0,0,0,0\nRINZU_NATIVE_ABANDONED=1\nRINZU_CALLBACK_RETURNS=0,0,0,0\nRINZU_NATIVE_ENTRIES=0,0,0,0,0,0\nRINZU_CALLBACK_PROCESS=0,0,0\nRINZU_CALLBACK_TEMPORARY=0,0\n";
        compare_native_call_evidence(evidence, correct).unwrap();
        for broken in [
            correct.replace("RINZU_NATIVE_ABANDONED=1", "RINZU_NATIVE_ABANDONED=0"),
            correct.replace("RINZU_NATIVE_ABANDONED=1\n", ""),
            correct.replace("3,2,2,2,2,2", "3,3,3,3,3,3"),
            correct.replace("RINZU_NATIVE_ABANDONED=1", "RINZU_NATIVE_ABANDONED=1,1"),
        ] {
            assert!(compare_native_call_evidence(evidence, &broken).is_err());
        }
    }

    #[test]
    fn native_equivalence_rejects_unreleased_scopes_and_false_promotion() {
        let evidence = rinzu_codegen::SourceNativeCallEvidence {
            prepared: 1,
            returned: 1,
            closed: 1,
            settled: 1,
            read: 1,
            released: 1,
            callback_returns: rinzu_codegen::CallbackReturnEvidence {
                normal: 1,
                fallback: 1,
                bytes: 8,
                metadata_bytes: 48,
            },
            ..Default::default()
        };
        let correct = "RINZU_NATIVE_CALLS=1,1,1,1,1,1,0,0,0,0,0\nRINZU_NATIVE_ABANDONED=0\nRINZU_CALLBACK_ALLOCATIONS=1,1,8,48\nRINZU_CALLBACK_RETURNS=0,0,0,0\nRINZU_CALLBACK_SCOPES=2,2,0,8,8,0,0,8,6,0,0\nRINZU_NATIVE_ENTRIES=0,0,0,0,0,0\nRINZU_CALLBACK_PROCESS=0,0,0\nRINZU_CALLBACK_TEMPORARY=0,0\n";
        compare_native_call_evidence(evidence, correct).unwrap();
        for replacement in [
            "2,1,0,8,8,0,0,8,6,0,0",
            "2,2,0,8,7,0,0,8,6,0,0",
            "2,2,0,8,8,0,1,8,6,0,0",
            "2,2,0,8,8,0,0,8,6,1,0",
            "2,2,0,8,8,0,0,8,6,0,24",
            "2,1,1,8,4,4,0,8,6,0,0",
            "2",
        ] {
            assert!(
                compare_native_call_evidence(
                    evidence,
                    &correct.replace("2,2,0,8,8,0,0,8,6,0,0", replacement)
                )
                .is_err()
            );
        }
    }

    #[test]
    fn native_equivalence_requires_all_foreign_receipts_retirement_and_wake_baseline() {
        let evidence = rinzu_codegen::SourceNativeCallEvidence {
            prepared: 1,
            returned: 1,
            closed: 1,
            settled: 1,
            read: 1,
            released: 1,
            foreign: rinzu_codegen::ForeignInvocationEvidence {
                created: 8,
                published: 8,
                received: 8,
                retired: 8,
                ..Default::default()
            },
            ..Default::default()
        };
        let correct = "RINZU_NATIVE_CALLS=1,1,1,1,1,1,0,0,0,0,0\nRINZU_NATIVE_ABANDONED=0\nRINZU_CALLBACK_RETURNS=0,0,0,0\nRINZU_FOREIGN=8,8,8,8,0,0,0,0\nRINZU_NATIVE_ENTRIES=0,0,0,0,0,0\nRINZU_CALLBACK_PROCESS=0,0,0\nRINZU_CALLBACK_TEMPORARY=0,0\n";
        compare_native_call_evidence(evidence, correct).unwrap();
        for corrupt in [
            "",
            "8,8,7,8,0,0,0,0",
            "8,8,8,7,0,0,0,0",
            "8,8,8,8,1,0,0,0",
            "8,8,8,8,0,1,0,0",
            "8,8,8,8,0,0,1,0",
            "8,8,8,8,0,0,0,1",
            "8,8,8,8,0,0,0,0,0",
        ] {
            assert!(
                compare_native_call_evidence(
                    evidence,
                    &correct.replace("8,8,8,8,0,0,0,0", corrupt)
                )
                .is_err(),
                "accepted {corrupt}"
            );
        }
    }

    #[test]
    fn concurrent_transport_evidence_proves_actual_peaks_and_packet_widths() {
        let early_inputs =
            parse_transport_credits("1:0:0:14:1:3;2:1:0:14:2:4;3:0:1:4:5:7;4:1:1:4:6:8").unwrap();
        let early_result =
            parse_transport_credits("1:0:0:14:1:2;2:0:1:4:3:7;3:1:0:14:4:5;4:1:1:4:6:8").unwrap();
        assert_eq!(
            validate_transport_credits(&early_inputs, 28).unwrap(),
            validate_transport_credits(&early_result, 18).unwrap()
        );
        for (credits, peak) in [(&early_inputs, 18), (&early_result, 28)] {
            assert!(validate_transport_credits(credits, peak).is_err());
        }
        for invalid in [
            "1:0:0:14:1:4;2:0:1:4:2:3", // result before input release
            "1:0:0:14:1:2;2:0:1:4:3:3", // duplicate sequence
            "1:0:0:14:1:0",             // missing release
            "1:0:1:4:1:2",              // no admitted input
        ] {
            assert!(
                validate_transport_credits(&parse_transport_credits(invalid).unwrap(), 18).is_err()
            );
        }
    }
}

#[cfg(all(test, unix))]
#[path = "aot_poll_tests.rs"]
mod poll_tests;
