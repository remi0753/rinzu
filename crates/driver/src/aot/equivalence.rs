//! Compare JIT and AOT results, traces and resource lifetimes.
use super::main_uses_source_async;
use rinzu_codegen::{
    BaselineFailure, BaselineOptions, LoadedNativePackages, run_baseline_jit_with_arguments,
};
use rinzu_trace::TraceDocument;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

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

pub(super) fn parse_resource_numbers(value: &str) -> Result<Vec<usize>, String> {
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

pub(super) fn parse_line<'a>(output: &'a str, prefix: &str) -> Result<&'a str, String> {
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

pub(super) fn diagnostic_signature(failure: Option<&rinzu_runtime::failure::Failure>) -> String {
    let Some(failure) = failure else {
        return "0".into();
    };
    std::iter::once(&failure.primary)
        .chain(&failure.cleanup)
        .map(|diagnostic| {
            let cause = diagnostic.cause.diagnostic_code();
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

#[cfg(test)]
mod tests;
