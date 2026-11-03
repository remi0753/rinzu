use std::env;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use anyhow::{Context, Result, anyhow, bail, ensure};
use rinzu_codegen::{emit_aot_object, run_jit_spike};
use rinzu_conformance::resource_probe::run_resource_probe;
use rinzu_conformance::stress::{StressConfig, run_host_stress};
use rinzu_trace::TraceDocument;
use serde_json::json;

fn main() -> ExitCode {
    match run(env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("rinzu-conformance: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(mut arguments: impl Iterator<Item = String>) -> Result<()> {
    match arguments.next().as_deref() {
        Some("trace") if arguments.next().as_deref() == Some("validate") => {
            let path = arguments
                .next()
                .map(PathBuf::from)
                .ok_or_else(|| anyhow!("trace validate requires a file"))?;
            validate_trace(&path)
        }
        Some("trace-schema") => validate_schema_document(),
        Some("backend-spike") => {
            let output = arguments
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("artifacts/backend-spike"));
            backend_spike(&output)
        }
        Some("host-stress") => host_stress(arguments),
        Some("resource-profile") => {
            let output = arguments.next().map(PathBuf::from);
            let report = run_resource_probe(output.as_deref())?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        _ => bail!(
            "usage: rinzu-conformance trace validate <file> | trace-schema | backend-spike [output-dir] | host-stress [--seed n] [--rounds n] | resource-profile [output.json]"
        ),
    }
}

fn host_stress(mut arguments: impl Iterator<Item = String>) -> Result<()> {
    let mut config = StressConfig::default();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--seed" => {
                config.seed = arguments
                    .next()
                    .ok_or_else(|| anyhow!("--seed requires a u64"))?
                    .parse()
                    .context("parse Host stress seed")?;
            }
            "--rounds" => {
                config.rounds = arguments
                    .next()
                    .ok_or_else(|| anyhow!("--rounds requires a count"))?
                    .parse()
                    .context("parse Host stress rounds")?;
            }
            other => bail!("unknown host-stress argument `{other}`"),
        }
    }
    run_host_stress(config)?;
    println!(
        "Host stress passed: seed={}, rounds={}, producers={}, items={}",
        config.seed, config.rounds, config.producers, config.items_per_producer
    );
    Ok(())
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn validate_trace(path: &Path) -> Result<()> {
    let trace = TraceDocument::from_reader(
        File::open(path).with_context(|| format!("open {}", path.display()))?,
    )?;
    println!(
        "trace valid: {} event(s), {} semantic event(s)",
        trace.events.len(),
        trace.semantic().events.len()
    );
    Ok(())
}

fn validate_schema_document() -> Result<()> {
    let path = workspace_root().join("docs/abi/event-trace.schema.json");
    let schema: serde_json::Value = serde_json::from_reader(
        File::open(&path).with_context(|| format!("open {}", path.display()))?,
    )
    .context("parse trace JSON Schema")?;
    ensure!(
        schema.pointer("/properties/schema/const") == Some(&json!("rinzu.event-trace")),
        "trace JSON Schema has the wrong schema discriminator"
    );
    ensure!(
        schema.pointer("/properties/version/const") == Some(&json!(1)),
        "trace JSON Schema has the wrong version"
    );
    ensure!(
        schema.pointer("/$defs/event").is_some(),
        "trace JSON Schema does not define events"
    );
    println!("trace schema valid: {}", path.display());
    Ok(())
}

fn backend_spike(output_directory: &Path) -> Result<()> {
    ensure!(
        cfg!(all(target_os = "macos", target_arch = "aarch64")),
        "backend spike execution requires aarch64-apple-darwin"
    );
    fs::create_dir_all(output_directory)
        .with_context(|| format!("create {}", output_directory.display()))?;

    let jit = run_jit_spike().context("run baseline JIT spike")?;
    let aot = emit_aot_object().context("emit AOT object")?;
    let object_path = output_directory.join("rinzu-backend-spike.o");
    fs::write(&object_path, &aot.object)
        .with_context(|| format!("write {}", object_path.display()))?;

    let runner_source = workspace_root().join("tests/fixtures/aot/runner.c");
    let runner_path = output_directory.join("rinzu-backend-spike-runner");
    let compiler = env::var_os("CC").unwrap_or_else(|| "cc".into());
    let link = Command::new(&compiler)
        .arg("-std=c11")
        .arg("-Wall")
        .arg("-Wextra")
        .arg("-Werror")
        .arg(&runner_source)
        .arg(&object_path)
        .arg("-o")
        .arg(&runner_path)
        .output()
        .context("link AOT object with C runner")?;
    if !link.status.success() {
        bail!(
            "AOT C runner link failed:\n{}",
            String::from_utf8_lossy(&link.stderr)
        );
    }
    let execution = Command::new(&runner_path)
        .output()
        .context("execute AOT C runner")?;
    if !execution.status.success() {
        bail!(
            "AOT C runner failed:\n{}",
            String::from_utf8_lossy(&execution.stderr)
        );
    }

    let report = json!({
        "backend": "cranelift-0.136",
        "target": "aarch64-apple-darwin",
        "jit": {
            "result": jit.result,
            "memory_policy": jit.memory_policy,
            "executable_mappings": jit.executable_mappings,
            "executable_bytes": jit.executable_bytes,
            "code_size": jit.compile.code_size,
            "stack_maps": jit.compile.stack_map_count,
            "stack_map_entries": jit.compile.stack_map_entry_count
        },
        "aot": {
            "object": object_path.file_name().and_then(|name| name.to_str()),
            "object_size": aot.object.len(),
            "code_size": aot.compile.code_size,
            "stack_maps": aot.compile.stack_map_count,
            "stack_map_entries": aot.compile.stack_map_entry_count,
            "c_abi_runner": "passed"
        }
    });
    let report_path = output_directory.join("backend-report.json");
    fs::write(&report_path, serde_json::to_vec_pretty(&report)?)
        .with_context(|| format!("write {}", report_path.display()))?;
    println!("backend spike passed: {}", report_path.display());
    Ok(())
}
