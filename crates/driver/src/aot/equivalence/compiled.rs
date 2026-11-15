//! Compare the compiled async engine with a separate AOT process.
use super::{
    BaselineOptions, EquivalenceReport, Target, TraceDocument, TransportPackets,
    compare_async_task_traces, compare_userdata_process, execute_async_aot, parse_i64_line,
    parse_line, parse_resource_numbers, parse_transport_credits,
    validate_callback_resource_evidence, validate_transport_credits,
};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;

pub(super) fn verify(
    context: &rinzu_compiler::CompilationContext,
    options: BaselineOptions,
    cancel_worker_after: Option<usize>,
    arguments: &[OsString],
) -> Result<EquivalenceReport, String> {
    let jit = crate::compiled_async::run_for_equivalence(
        &context.metadata,
        &context.artifact,
        options,
        cancel_worker_after,
        arguments,
    )?;
    let (process, output, source_files) =
        execute_async_aot(context, options, cancel_worker_after, arguments)?;
    if process.status.code() != Some(jit.exit_code) {
        return Err(format!(
            "async JIT/AOT exit status differs: JIT={}, AOT={}",
            jit.exit_code, process.status
        ));
    }
    if source_files != serde_json::to_value(&jit.source_files).map_err(|error| error.to_string())? {
        return Err("async JIT/AOT diagnostic source identities differ".into());
    }
    let aot = crate::future_identity::normalize_compiled(&jit.output, &output)?;
    let jit = jit.output;
    for key in [
        "RINZU_RESULT=",
        "RINZU_FAILURE=",
        "RINZU_FAILURE_MESSAGE=",
        "RINZU_DIAGNOSTICS=",
        "RINZU_CAUGHT_ERRORS=",
        "RINZU_STARTED_CHILDREN=",
        "RINZU_STARTED_TIMERS=",
        "RINZU_NATIVE_COMPLETIONS=",
    ] {
        if parse_line(&jit, key)? != parse_line(&aot, key)? {
            return Err(format!("async JIT/AOT evidence differs for {key}"));
        }
    }
    for key in ["RINZU_NATIVE_CALLS=", "RINZU_NATIVE_ABANDONED="] {
        if optional(&jit, key) != optional(&aot, key) {
            return Err(format!(
                "async JIT/AOT native call evidence differs for {key}"
            ));
        }
    }
    let jit_trace = settled_trace(&jit)?;
    let aot_trace = settled_trace(&aot)?;
    compare_async_task_traces(&jit_trace, &aot_trace)?;
    if awaited_values(&jit)? != awaited_values(&aot)? {
        return Err("async JIT/AOT awaited values differ within a task".into());
    }

    let jit_packets = settled_transport(&jit)?;
    let aot_packets = settled_transport(&aot)?;
    let cancelled = [
        parse_line(&jit, "RINZU_CANCELLED_CHILDREN=")?,
        parse_line(&aot, "RINZU_CANCELLED_CHILDREN=")?,
    ]
    .into_iter()
    .flat_map(|ids| ids.split(',').filter(|id| !id.is_empty()))
    .map(|id| id.parse::<u64>().map_err(|_| "invalid cancelled Worker"))
    .collect::<Result<BTreeSet<_>, _>>()?;
    if jit_packets.keys().ne(aot_packets.keys())
        || jit_packets.iter().any(|(child, (input, results))| {
            let (actual_input, actual_results) = &aot_packets[child];
            input != actual_input || (!cancelled.contains(child) && results != actual_results)
        })
    {
        return Err("async JIT/AOT Worker transport differs".into());
    }
    if settled_workers(&jit)? != settled_workers(&aot)? {
        return Err("async JIT/AOT settled Worker identities differ".into());
    }
    Ok(EquivalenceReport {
        result: parse_i64_line(&jit, "RINZU_RESULT=")?,
        outcome: match parse_line(&jit, "RINZU_FAILURE=")? {
            "none" => "success",
            "cancelled" => "cancelled",
            "overloaded" => "overloaded",
            "shutdown" => "shutdown",
            _ => "failure",
        },
    })
}

fn optional<'a>(report: &'a str, key: &str) -> Option<&'a str> {
    report.lines().find_map(|line| line.strip_prefix(key))
}

fn settled_trace(report: &str) -> Result<TraceDocument, String> {
    let trace = TraceDocument::from_reader(parse_line(report, "RINZU_TRACE=")?.as_bytes())
        .map_err(|error| format!("invalid async trace: {error}"))?;
    if trace.context.get("resources").map(String::as_str) != Some("baseline")
        || trace.context.get("resume-domain").map(String::as_str) != Some("main")
    {
        return Err("async runtime failed to settle on Main".into());
    }
    Ok(trace)
}

fn awaited_values(report: &str) -> Result<BTreeMap<u64, Vec<&str>>, String> {
    let mut by_task = BTreeMap::<u64, Vec<&str>>::new();
    for row in parse_line(report, "RINZU_AWAITED_TASK_VALUES=")?
        .split(',')
        .filter(|row| !row.is_empty())
    {
        let (task, value) = row.split_once(':').ok_or("invalid await proof")?;
        let task = task
            .parse::<u64>()
            .map_err(|_| "invalid await task identity")?;
        by_task.entry(task).or_default().push(value);
    }
    Ok(by_task)
}

fn settled_transport(report: &str) -> Result<TransportPackets, String> {
    let counters = parse_resource_numbers(parse_line(report, "RINZU_TRANSPORT=")?)?;
    let [0, _, 0, _, 0, peak] = counters[..] else {
        return Err("async runtime retained Worker domains or transport".into());
    };
    let continuations = parse_resource_numbers(parse_line(report, "RINZU_CONTINUATIONS=")?)?;
    if !matches!(continuations[..], [0, _, 0, _]) {
        return Err("async runtime retained continuation stacks".into());
    }
    let credits = parse_transport_credits(parse_line(report, "RINZU_CREDITS=")?)?;
    validate_transport_credits(&credits, peak)
}

fn settled_workers(report: &str) -> Result<BTreeSet<usize>, String> {
    let mut workers = BTreeSet::new();
    for row in parse_line(report, "RINZU_WORKER_HEAPS=")?
        .split(';')
        .filter(|row| !row.is_empty())
    {
        let fields = row
            .split(':')
            .map(str::parse::<usize>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "invalid Worker heap evidence")?;
        let [id, 0, 0, collections, _, _, 0 | 1] = fields[..] else {
            return Err("async runtime retained Worker heap objects or roots".into());
        };
        if collections == 0 || !workers.insert(id) {
            return Err("invalid settled Worker heap identity".into());
        }
    }
    let mut userdata = BTreeSet::new();
    for row in parse_line(report, "RINZU_WORKER_USERDATA=")?
        .split(';')
        .filter(|row| !row.is_empty())
    {
        let fields = row
            .split(':')
            .map(str::parse::<usize>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "invalid Worker userdata evidence")?;
        let [id, 0, 0, 0, 0, created, released] = fields[..] else {
            return Err("async runtime retained Worker userdata".into());
        };
        if created != released || !userdata.insert(id) {
            return Err("invalid settled Worker userdata identity".into());
        }
    }
    if userdata != workers {
        return Err("Worker userdata and heap identities differ".into());
    }
    if let Some(calls) = optional(report, "RINZU_NATIVE_CALLS=") {
        if !matches!(
            parse_resource_numbers(calls)?[..],
            [_, _, _, _, _, _, _, _, 0, 0, 0]
        ) {
            return Err("async runtime retained native calls or callbacks".into());
        }
        validate_callback_resource_evidence(Target::X86_64UnknownLinuxGnu, report)?;
    }
    compare_userdata_process(report)?;
    Ok(workers)
}
