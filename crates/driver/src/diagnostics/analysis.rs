//! Finite offline views of distinct producers; legacy joins remain candidates.
use super::*;
use std::io::Read;

struct Input {
    bytes: Vec<u8>,
    total: u64,
    truncated: bool,
}

fn read(path: &Path, limit: usize, complete: bool) -> Result<Input, String> {
    if !std::fs::metadata(path)
        .map_err(|error| format!("{}: {error}", path.display()))?
        .is_file()
    {
        return Err("diagnostic inputs must be regular files".into());
    }
    use std::os::unix::fs::OpenOptionsExt;
    // A path changed into a FIFO between metadata/open must not block the reader.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("diagnostic inputs must be regular files".into());
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    let truncated = bytes.len() > limit;
    bytes.truncate(limit);
    if truncated && complete {
        return Err(format!("{} exceeds the input budget", path.display()));
    }
    Ok(Input {
        bytes,
        total: metadata.len(),
        truncated,
    })
}

fn validate(report: &Report) -> Result<(), String> {
    if report.schema != SCHEMA
        || report.version != 1
        || report.digest_algorithm != "fnv1a64"
        || report.run_id.is_empty()
        || !matches!(report.phase.as_str(), "prepared" | "finished")
        || report.sources.len() > ROW_LIMIT
        || report.inputs.len() > ROW_LIMIT
    {
        return Err("invalid diagnostic report schema/version/limits".into());
    }
    if report.phase == "prepared" && (report.outcome.is_some() || report.exit_code.is_some()) {
        return Err("prepared report contains a finished outcome".into());
    }
    if let Some(outcome) = &report.outcome {
        if !outcome.is_object() {
            return Err("runtime outcome must be an object".into());
        }
        if let Some(failure) = outcome.get("failure").filter(|failure| !failure.is_null())
            && (!failure.is_object()
                || !failure["primary"].is_object()
                || !failure["cleanup"].as_array().is_some_and(|cleanup| {
                    cleanup.len() <= ROW_LIMIT && cleanup.iter().all(Value::is_object)
                }))
        {
            return Err("invalid runtime failure shape or cleanup limit".into());
        }
    }
    let mut seen = BTreeSet::new();
    let (mut bytes, mut files) = (0, 0);
    for source in &report.sources {
        if !seen.insert(source.file) {
            return Err("duplicate diagnostic source identity".into());
        }
        if let Some(text) = &source.text {
            bytes += text.len();
            files += 1;
            if text.len() > SOURCE_LIMIT
                || bytes > SOURCE_TOTAL
                || files > SOURCE_FILES
                || source.byte_length != Some(text.len())
                || source.digest.as_deref()
                    != Some(&format!(
                        "{:016x}",
                        rinzu_compiler::fnv1a64(text.as_bytes())
                    ))
            {
                return Err("captured source does not match its receipt or budget".into());
            }
        }
    }
    Ok(())
}

fn resolve(report: &Report, position: &Value, strength: &str) -> Value {
    let Some(file) = position["file"].as_u64() else {
        return json!({"strength":"unresolved","reason":"no source position"});
    };
    let Some(source) = report.sources.iter().find(|source| source.file == file) else {
        return json!({"strength":"unresolved","position":position,"reason":"unknown runtime source ID"});
    };
    let mut result = json!({
        "strength":strength,"runId":report.run_id,"position":position,
        "path":source.path,"diskPath":source.disk_path,"digest":source.digest,
        "digestAlgorithm":report.digest_algorithm,"line":Value::Null,"column":Value::Null,
        "reason":source.omission
    });
    let location = (|| {
        let text = source.text.as_ref()?;
        let start = usize::try_from(position["startByte"].as_u64()?).ok()?;
        let end = usize::try_from(position["endByte"].as_u64()?).ok()?;
        let bom = if text.starts_with('\u{feff}') { 3 } else { 0 };
        if start < bom
            || start > end
            || end > text.len()
            || !text.is_char_boundary(start)
            || !text.is_char_boundary(end)
        {
            return None;
        }
        // Same original-byte/scalar coordinates as SourceFile; borrow captured text.
        let prefix = &text[bom..start];
        let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
        let column = prefix.rsplit('\n').next()?.chars().count() + 1;
        Some((line, column))
    })();
    if let Some((line, column)) = location {
        result["line"] = json!(line);
        result["column"] = json!(column);
    } else {
        result["strength"] = json!("unresolved");
        result["reason"] = json!("source body omitted or invalid byte/UTF-8 boundary");
    }
    result
}

fn resolved_outcome(report: &Report) -> Value {
    let Some(mut outcome) = report.outcome.clone() else {
        return json!({"status":"unknown","settlement":"unconfirmed","reason":"no private runtime outcome"});
    };
    if let Some(failure) = outcome["failure"].as_object_mut() {
        if let Some(primary) = failure.get_mut("primary") {
            primary["location"] = resolve(report, &primary["source"], "captured");
        }
        if let Some(cleanup) = failure.get_mut("cleanup").and_then(Value::as_array_mut) {
            cleanup.truncate(ROW_LIMIT);
            for diagnostic in cleanup {
                diagnostic["location"] = resolve(report, &diagnostic["source"], "captured");
            }
        }
    }
    outcome
}

fn input_summary(path: &Path, input: &Input) -> Value {
    json!({"path":path,"readBytes":input.bytes.len(),"fileBytesAtOpen":input.total,"truncated":input.truncated,"omittedBytesAtOpen":input.total.saturating_sub(input.bytes.len() as u64)})
}

fn complete_lines(input: &Input) -> &[u8] {
    if input.truncated {
        &input.bytes[..input
            .bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1)]
    } else {
        &input.bytes
    }
}

fn resources(path: &Path, report: &Report) -> Result<Value, String> {
    let input = read(path, STREAM_LIMIT, false)?;
    let expected: BTreeMap<String, String> = report
        .sources
        .iter()
        .map(|source| (source.file.to_string(), source.path.clone()))
        .collect();
    let expected = serde_json::to_value(expected).map_err(|error| error.to_string())?;
    let recorded = report
        .resource_report
        .as_ref()
        .map(|recorded| same_file(path, recorded))
        .transpose()?
        .unwrap_or(false);
    let mut rows = BTreeMap::<String, Value>::new();
    let (mut records, mut invalid, mut mismatched, mut omitted, mut end) = (0, 0, 0, 0, false);
    let mut contexts = BTreeSet::new();
    for bytes in complete_lines(&input)
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let Ok(sample) = serde_json::from_slice::<Value>(bytes) else {
            invalid += 1;
            continue;
        };
        if sample["schema"] != "rinzu.resource-report"
            || sample["version"] != 1
            || sample["process_id"].as_u64().is_none()
            || sample["sequence"].as_u64().is_none()
            || sample["elapsed_ns"].as_u64().is_none()
            || sample["producer"].as_str().is_none()
            || !matches!(sample["phase"].as_str(), Some("start" | "sample" | "end"))
            || sample["sources"].as_object().is_none()
        {
            invalid += 1;
            continue;
        }
        let Some(categories) = sample["categories"].as_array() else {
            invalid += 1;
            continue;
        };
        records += 1;
        let matched = sample["process_id"] == report.process_id
            && sample["producer"].as_str() == report.resource_producer.as_deref()
            && sample["sources"] == expected
            && report.omitted_sources == 0;
        if !matched {
            mismatched += 1;
        }
        if matched && recorded && sample["phase"] == "end" {
            end = true;
        }
        let context = json!([sample["process_id"], sample["producer"], sample["sources"]]);
        let context_key = format!(
            "{:016x}",
            rinzu_compiler::fnv1a64(context.to_string().as_bytes())
        );
        if contexts.len() < ROW_LIMIT {
            contexts.insert(context_key.clone());
        }
        for category in categories {
            if !matches!(category["unit"].as_str(), Some("bytes" | "count"))
                || !matches!(
                    category["status"].as_str(),
                    Some("measured" | "unknown" | "unsupported")
                )
                || ["id", "scope", "retention_reason"].iter().any(|key| {
                    !category[key]
                        .as_str()
                        .is_some_and(|text| text.len() <= RAW_LIMIT)
                })
                || ["live", "allocated", "released", "peak"]
                    .iter()
                    .any(|key| !category[key].is_null() && category[key].as_u64().is_none())
                || category["attribution"].as_object().is_none()
            {
                invalid += 1;
                continue;
            }
            let key = json!([
                context_key,
                category["id"],
                category["scope"],
                category["unit"],
                category["subset_of"],
                category["attribution"]
            ])
            .to_string();
            if !rows.contains_key(&key) && rows.len() == ROW_LIMIT {
                omitted += 1;
                continue;
            }
            let position = json!({"file":category["attribution"]["source_file"],"startByte":category["attribution"]["start_byte"],"endByte":category["attribution"]["end_byte"]});
            let mut latest = json!({});
            for field in [
                "id",
                "scope",
                "unit",
                "status",
                "live",
                "allocated",
                "released",
                "peak",
                "retention_reason",
                "subset_of",
                "attribution",
            ] {
                latest[field] = category[field].clone();
            }
            let strength = if matched && recorded {
                "captured"
            } else {
                "candidate"
            };
            latest["location"] = resolve(report, &position, strength);
            latest["association"] = json!(if matched && recorded {
                "recorded-run-context"
            } else {
                "legacy-candidate"
            });
            latest["producer"] = sample["producer"].clone();
            latest["processId"] = sample["process_id"].clone();
            latest["phase"] = sample["phase"].clone();
            latest["sequence"] = sample["sequence"].clone();
            let row = rows
                .entry(key)
                .or_insert_with(|| json!({"samples":0,"observedMaxLive":Value::Null}));
            row["samples"] = json!(row["samples"].as_u64().unwrap_or(0) + 1);
            if let Some(live) = category["live"].as_u64() {
                row["observedMaxLive"] =
                    json!(row["observedMaxLive"].as_u64().unwrap_or(0).max(live));
            }
            row["latest"] = latest;
        }
    }
    Ok(
        json!({"input":input_summary(path,&input),"records":records,"invalidRecordsOrRows":invalid,"mismatchedContexts":mismatched,"contextCount":contexts.len(),"omittedRowObservations":omitted,"omittedPartialLineBytes":input.bytes.len()-complete_lines(&input).len(),"recordedRunEndSeen":end,"categories":rows.into_values().collect::<Vec<_>>(),"note":"Subsets/virtual/count/SDK unknown are separate; no sum or full retention graph"}),
    )
}

fn native(path: &Path, report: &Report) -> Result<Value, String> {
    let input = read(path, STREAM_LIMIT, false)?;
    let (mut rows, mut omitted, mut invalid) = (Vec::new(), 0, 0);
    for bytes in complete_lines(&input)
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        if rows.len() == ROW_LIMIT {
            omitted += 1;
            continue;
        }
        let Ok(line) = std::str::from_utf8(bytes) else {
            invalid += 1;
            rows.push(json!({"parsed":false,"rawBytes":bytes.iter().copied().take(1024).collect::<Vec<_>>(),"omittedRawBytes":bytes.len().saturating_sub(1024),"association":"legacy-candidate","note":"Invalid UTF-8 notification; raw prefix retained"}));
            continue;
        };
        let body = line
            .strip_prefix("rinzu warning: ")
            .or_else(|| line.strip_prefix("rinzu fatal: "))
            .or_else(|| line.strip_prefix("rinzu wait: "));
        let kind = body.and_then(|body| body.split_whitespace().next());
        let field = |key: &str| {
            line.split_whitespace()
                .find_map(|part| part.strip_prefix(key))
        };
        let position = field("source=")
            .and_then(|text| parse_position(text).ok())
            .unwrap_or(Value::Null);
        rows.push(json!({
            "raw":clipped(line,RAW_LIMIT),"omittedRawBytes":line.len().saturating_sub(RAW_LIMIT),
            "kind":kind,"association":"legacy-candidate","location":resolve(report,&position,"candidate"),
            "logical":{"symbol":field("symbol="),"task":Value::Null,"scope":Value::Null,"owner":Value::Null,"codeVersion":Value::Null},
            "physical":{"pthreadOwnerTag":field("owner="),"nativeDepth":field("depth=").and_then(|text|text.parse::<u64>().ok()),"elapsedMs":field("elapsed_ms=").and_then(|text|text.parse::<u64>().ok())},
            "boardLocalActivity":field("activity=").and_then(|text|text.parse::<u64>().ok()),
            "activityStack":field("stack="),"potentiallyTruncated":line.len()>=510,
            "parsed":body.is_some(),"note":"Prepared activity scopes only; no full managed/native stack or verified run ID"
        }));
    }
    Ok(
        json!({"input":input_summary(path,&input),"events":rows,"omittedEvents":omitted,"invalidUtf8Lines":invalid,"omittedPartialLineBytes":input.bytes.len()-complete_lines(&input).len(),"finalNewlineSeen":input.bytes.last()==Some(&b'\n'),"note":"Legacy append log can contain other runs; no clock/task/Owner join is inferred"}),
    )
}

fn trace(path: &Path) -> Result<Value, String> {
    let input = read(path, STREAM_LIMIT, true)?;
    let mut trace = rinzu_trace::TraceDocument::from_reader(input.bytes.as_slice())
        .map_err(|error| format!("invalid trace: {error}"))?;
    let total = trace.events.len();
    trace.events.truncate(ROW_LIMIT);
    Ok(
        json!({"input":input_summary(path,&input),"association":"independent-trace-candidate","eventCount":total,"omittedEvents":total.saturating_sub(ROW_LIMIT),"document":trace,"note":"Task labels and optional source/physical observations retain trace-local meanings"}),
    )
}

fn exit(path: &Path, report: &Report) -> Result<Value, String> {
    let input = read(path, RAW_LIMIT, true)?;
    let record: Value = serde_json::from_slice(&input.bytes)
        .map_err(|error| format!("invalid exit record: {error}"))?;
    if record["schema"] != "rinzu.process-exit"
        || record["version"] != 1
        || record["processId"].as_u64().is_none()
        || record["observer"].as_str().is_none()
        || !((record["exitCode"].as_u64().is_some() && record["signal"].is_null())
            || (record["signal"].as_u64().is_some_and(|signal| signal > 0)
                && record["exitCode"].is_null()))
    {
        return Err("invalid external exit record".into());
    }
    Ok(
        json!({"input":input_summary(path,&input),"processMatches":record["processId"]==report.process_id,"record":record,"provenance":"explicit external observer receipt; not private runtime protocol","note":"Exit code alone does not identify OOM/protocol/user-requested failure"}),
    )
}

pub(crate) fn analyze(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let (mut report_path, mut resources_path, mut native_path, mut trace_path, mut exit_path) =
        (None, None, None, None, None);
    let mut format = None;
    while let Some(argument) = arguments.next() {
        let slot = match argument.as_str() {
            "--resources" => Some(&mut resources_path),
            "--native-log" => Some(&mut native_path),
            "--trace" => Some(&mut trace_path),
            "--exit-record" => Some(&mut exit_path),
            _ => None,
        };
        if let Some(slot) = slot {
            if slot.is_some() {
                return Err(format!("duplicate {argument}"));
            }
            let value = arguments
                .next()
                .filter(|value| !value.starts_with('-') && !value.is_empty())
                .ok_or_else(|| format!("{argument} requires a file"))?;
            *slot = Some(PathBuf::from(value));
        } else if argument == "--format" {
            if format.is_some() {
                return Err("duplicate --format".into());
            }
            let value = arguments.next().ok_or("--format requires text or json")?;
            if !matches!(value.as_str(), "text" | "json") {
                return Err("--format requires text or json".into());
            }
            format = Some(value);
        } else if argument.starts_with('-') || report_path.is_some() {
            return Err(format!("unexpected diagnose argument {argument}"));
        } else {
            report_path = Some(PathBuf::from(argument));
        }
    }
    let report_path = report_path.ok_or("diagnose requires a diagnostic report file")?;
    let input = read(&report_path, REPORT_LIMIT, true)?;
    let report: Report = serde_json::from_slice(&input.bytes)
        .map_err(|error| format!("invalid diagnostic report: {error}"))?;
    validate(&report)?;
    let explicit_resources = resources_path.is_some();
    let resources_path = resources_path.or_else(|| report.resource_report.clone());
    let resource_view = resources_path
        .as_deref()
        .map(|path| match resources(path, &report) {
            Ok(value) => Ok(value),
            Err(error) if !explicit_resources => {
                Ok(json!({"path":path,"unavailable":error,"recordedRunEndSeen":false}))
            }
            Err(error) => Err(error),
        })
        .transpose()?;
    let investigation = json!({
        "schema":"rinzu.diagnostic-investigation","version":1,"runId":report.run_id,"package":report.package,
        "phase":report.phase,"physical":{"processId":report.process_id},"reportInput":input_summary(&report_path,&input),
        "outcome":resolved_outcome(&report),"driverError":report.driver_error,"observedExitCode":report.exit_code,
        "resources":resource_view,
        "native":native_path.as_deref().map(|path|native(path,&report)).transpose()?,
        "trace":trace_path.as_deref().map(trace).transpose()?,
        "externalExit":exit_path.as_deref().map(|path|exit(path,&report)).transpose()?,
        "sourceCatalog":report.sources.iter().map(|source| json!({"file":source.file,"path":source.path,"digest":source.digest,"bodyCaptured":source.text.is_some(),"omission":source.omission})).collect::<Vec<_>>(),
        "omittedSources":report.omitted_sources,"omittedInputs":report.omitted_inputs,"limitations":report.limitations
    });
    if format.as_deref() == Some("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&investigation).map_err(|error| error.to_string())?
        );
    } else {
        print_text(&investigation);
    }
    Ok(())
}

fn location(value: &Value) -> String {
    match (
        value["path"].as_str(),
        value["line"].as_u64(),
        value["column"].as_u64(),
    ) {
        (Some(path), Some(line), Some(column)) => format!(
            "{path}:{line}:{column} ({})",
            value["strength"].as_str().unwrap_or("unknown")
        ),
        _ => format!(
            "source unresolved: {}",
            value["reason"].as_str().unwrap_or("unknown")
        ),
    }
}

fn print_text(value: &Value) {
    println!(
        "{} — {} [{}]",
        value["package"].as_str().unwrap_or("unknown"),
        value["runId"].as_str().unwrap_or("unknown"),
        value["phase"].as_str().unwrap_or("unknown")
    );
    println!(
        "outcome: {} / settlement: {}",
        value["outcome"]["status"].as_str().unwrap_or("unknown"),
        value["outcome"]["settlement"]
            .as_str()
            .unwrap_or("unconfirmed")
    );
    if let Some(error) = value["driverError"].as_str() {
        println!("driver: {error}");
    }
    let failure = &value["outcome"]["failure"];
    if !failure.is_null() {
        println!(
            "failure: {} — {}",
            failure["primary"]["cause"].as_str().unwrap_or("unknown"),
            location(&failure["primary"]["location"])
        );
        if let Some(cleanup) = failure["cleanup"].as_array() {
            for row in cleanup {
                println!(
                    "cleanup: {} — {}",
                    row["cause"].as_str().unwrap_or("unknown"),
                    location(&row["location"])
                );
            }
        }
    }
    if let Some(rows) = value["native"]["events"].as_array() {
        for row in rows {
            println!(
                "activity: {} — {}\n  {}",
                row["kind"].as_str().unwrap_or("unparsed"),
                location(&row["location"]),
                row["raw"].as_str().unwrap_or("")
            );
        }
        println!(
            "native association: legacy candidate; omitted {}",
            value["native"]["omittedEvents"]
        );
    }
    if let Some(rows) = value["resources"]["categories"].as_array() {
        println!(
            "resources: {} records; recorded end {}; invalid {}",
            value["resources"]["records"],
            value["resources"]["recordedRunEndSeen"],
            value["resources"]["invalidRecordsOrRows"]
        );
        for row in rows {
            let latest = &row["latest"];
            println!(
                "  {} [{} {} / {}] live={} observedMax={} — {}",
                latest["id"].as_str().unwrap_or("unknown"),
                latest["scope"].as_str().unwrap_or("unknown"),
                latest["unit"].as_str().unwrap_or("unknown"),
                latest["status"].as_str().unwrap_or("unknown"),
                latest["live"],
                row["observedMaxLive"],
                latest["retention_reason"].as_str().unwrap_or("unknown")
            );
            if !latest["location"]["line"].is_null() {
                println!("    {}", location(&latest["location"]));
            }
        }
        println!(
            "  input truncated {}; omitted rows {}; omitted partial-line bytes {}",
            value["resources"]["input"]["truncated"],
            value["resources"]["omittedRowObservations"],
            value["resources"]["omittedPartialLineBytes"]
        );
    }
    if let Some(error) = value["resources"]["unavailable"].as_str() {
        println!("resources unavailable: {error}; end unconfirmed");
    }
    if !value["trace"].is_null() {
        println!(
            "trace: {} events; independent candidate",
            value["trace"]["eventCount"]
        );
    }
    if !value["externalExit"].is_null() {
        println!(
            "external exit: {} / PID matches {}",
            value["externalExit"]["record"], value["externalExit"]["processMatches"]
        );
    }
    println!(
        "logical task/scope/Owner/version and full native stack/SDK allocator remain unknown where unpublished"
    );
}
