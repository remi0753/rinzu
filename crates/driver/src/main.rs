use std::collections::BTreeMap;
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use rinzu_codegen::{BaselineFailure, BaselineRun};
use rinzu_trace::{EventKind, TRACE_SCHEMA, TRACE_VERSION, TaskOutcome, TraceDocument, TraceEvent};

thread_local! {
    // CLI outcome only; never terminates an embedding host during settlement.
    static PROGRAM_EXIT_STATUS: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

#[global_allocator]
#[cfg(not(test))]
static ALLOCATOR: rinzu_runtime::allocation::FatalAllocator<std::alloc::System> =
    rinzu_runtime::allocation::FatalAllocator(std::alloc::System);

#[global_allocator]
#[cfg(test)]
static ALLOCATOR: rinzu_runtime::allocation::FatalAllocator<
    rinzu_codegen::allocation_probe::Allocator,
> = rinzu_runtime::allocation::FatalAllocator(rinzu_codegen::allocation_probe::Allocator);

mod aot;
mod aot_source_native;
mod app_layout;
mod appkit;
mod compile_cache;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod compiled_async;
mod dev;
mod diagnostics;
mod editor_protocol;
mod format_cli;
mod future_identity;
mod input;
#[cfg(test)]
mod m5_cli_tests;
mod macos_bundle;
mod native_binding;
mod native_build_scheduler;
mod native_clang;
mod native_cli;
mod native_config;
mod native_consumption;
mod native_contract;
mod native_deployment;
mod native_fingerprint;
mod native_image;
mod native_package;
mod package_commands;
mod package_tests;
mod release;
#[cfg(test)]
mod resource_accounting_tests;
mod resource_report;
#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
#[path = "../../compiler/test-support/source_edit.rs"]
mod source_edit;
#[cfg(test)]
mod source_macos_data_tests;
#[cfg(test)]
mod source_native_byte_array_tests;
#[cfg(test)]
mod source_native_callback_return_tests;
#[cfg(test)]
mod source_native_callback_tests;
#[cfg(test)]
mod source_native_capsule_tests;
#[cfg(test)]
mod source_native_consumption_tests;
#[cfg(test)]
mod source_native_data_tests;
#[cfg(test)]
mod source_native_dispatcher_tests;
#[cfg(test)]
mod source_native_foreign_tests;
mod source_native_package;
#[cfg(test)]
mod source_native_record_tests;
#[cfg(test)]
mod source_native_supplant_tests;
#[cfg(test)]
mod source_native_termination_tests;
#[cfg(test)]
mod source_native_userdata_tests;
mod source_package_bundle;
mod source_package_cache;
mod source_receipt;
#[cfg(test)]
mod source_service_tests;
#[cfg(test)]
mod source_sqlite_tests;
mod toolchain_bundle;
mod verify_capture;

fn main() -> ExitCode {
    rinzu_runtime::standalone::initialize_interrupt();
    match run(env::args().skip(1)) {
        Ok(()) => PROGRAM_EXIT_STATUS.with(|status| ExitCode::from(status.get())),
        Err(message) => {
            eprintln!("rinzu: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let _timing = rinzu_compiler::timing::span("driver.command");
    match arguments.next().as_deref() {
        Some("__test-worker") => package_tests::worker(arguments),
        Some("__verify-aot-worker") => verify_capture::worker(arguments),
        Some("__verify-aot-build") => verify_capture::build_worker(arguments),
        Some("--version" | "-V") => {
            println!("rinzu {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("doctor") => {
            let target = rinzu_runtime::current_target();
            let support = if rinzu_runtime::validate_v1_target().is_ok() {
                "supported"
            } else {
                "portability-check-only"
            };
            println!("target: {target}");
            println!("v1 runtime: {support}");
            Ok(())
        }
        Some("toolchain-bundle") => toolchain_bundle::run(arguments),
        Some("editor") => editor_protocol::run(arguments),
        Some("fmt") => {
            if format_cli::run(arguments)? {
                PROGRAM_EXIT_STATUS.with(|status| status.set(1));
            }
            Ok(())
        }
        Some("native-quality-sources") => {
            let directory = arguments
                .next()
                .ok_or("native-quality-sources requires an output directory")?;
            if arguments.next().is_some() {
                return Err("native-quality-sources accepts one output directory".into());
            }
            aot::native_quality::write_sources(&PathBuf::from(directory))
        }
        Some("run-empty") => {
            let result =
                rinzu_runtime::run_empty_standalone().map_err(|error| error.to_string())?;
            println!("empty standalone completed on {}", result.target);
            Ok(())
        }
        Some("check") => {
            let parsed = native_cli::parse("check", arguments)?;
            let path = parsed
                .path
                .map_or_else(|| env::current_dir().map_err(|error| error.to_string()), Ok)?;
            match input::check(&path, &parsed.input, &parsed.native) {
                Ok(context) => {
                    let manifest = context.metadata;
                    let artifact = context.artifact;
                    let checked_native = native_package::build_artifact(&manifest, &artifact)?;
                    std::fs::remove_dir_all(&checked_native.directory)
                        .map_err(|error| error.to_string())?;
                    println!(
                        "checked {} ({} functions, {} IR blocks)",
                        manifest.name,
                        artifact.checked.functions.len(),
                        artifact
                            .ir
                            .functions
                            .iter()
                            .map(|function| function.blocks.len())
                            .sum::<usize>()
                    );
                    Ok(())
                }
                Err(message) => Err(message),
            }
        }
        Some(command @ ("lock" | "api")) => package_commands::run(command, arguments),
        Some("diagnose") => diagnostics::analyze(arguments),
        Some("test") => package_tests::run(arguments),
        Some(command @ "run") => {
            let parsed = native_cli::parse(command, arguments)?;
            let path = parsed
                .path
                .map_or_else(|| env::current_dir().map_err(|error| error.to_string()), Ok)?;
            if let Some(report) = parsed.diagnostic_report {
                return diagnostics::run_package(
                    path,
                    parsed.trace,
                    &parsed.native,
                    parsed.resource_report.as_ref(),
                    report,
                    &parsed.input,
                );
            }
            PROGRAM_EXIT_STATUS.with(|status| status.set(0));
            let context = input::check(&path, &parsed.input, &parsed.native)?;
            if let Some(resources) = &parsed.resource_report {
                input::protect_outputs(&context, std::slice::from_ref(&resources.path))?;
            }
            run_checked_package(
                &context.metadata,
                &context.artifact,
                parsed.trace,
                parsed.resource_report.as_ref(),
            )
        }
        Some("bundle") => macos_bundle::run(arguments),
        Some("bundle-verify") => macos_bundle::verify(arguments),
        Some("build") => {
            let parsed = native_cli::parse("build", arguments)?;
            let path = parsed
                .path
                .map_or_else(|| env::current_dir().map_err(|error| error.to_string()), Ok)?;
            let context = input::check(&path, &parsed.input, &parsed.native)?;
            if context.metadata.kind == rinzu_compiler::PackageKind::Sdk {
                let source = context
                    .artifact
                    .source_interface
                    .clone()
                    .ok_or("SDK source interface is missing")?;
                let manifest = context.metadata;
                let artifact = context.artifact;
                let directory = parsed
                    .output
                    .unwrap_or_else(|| manifest.root.join("build").join(&manifest.name));
                std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
                let package = rinzu_compiler::ResolvedSdkPackage {
                    manifest,
                    native_imports: Vec::new(),
                    source: Some(source),
                };
                let mut dependencies = native_package::build_with_native_inputs(
                    &artifact.sdk_packages,
                    &package.manifest.root,
                    &artifact.native_inputs,
                )?;
                let built = source_native_package::build_with_dependencies(
                    &package,
                    &package.manifest.root,
                    &artifact.native_inputs,
                    &directory,
                    &dependencies.dependency_digests,
                )?;
                dependencies.context(&package.manifest, &artifact, Some(&built))?;
                package_commands::check_sdk_lock(
                    &package,
                    &artifact.native_inputs,
                    &artifact.sdk_packages,
                    &dependencies,
                    &built,
                )?;
                dependencies.validate_source_packages()?;
                source_package_bundle::save(
                    &dependencies.source_packages,
                    &directory.join("dependencies"),
                )?;
                built.verify_inputs()?;
                rinzu_codegen::source_package::LoadedSourcePackages::load(std::slice::from_ref(
                    &built.artifact,
                ))
                .map_err(|error| format!("validate generated source facade: {error:#}"))?;
                if let Some(library) = built.artifact.library {
                    println!("facade: {}", library.display());
                }
                println!("descriptor: {}", built.descriptor.display());
                println!("contract: {}", built.contract.display());
                println!("table: {}", built.table.display());
                let _ = std::fs::remove_dir_all(dependencies.directory);
                return Ok(());
            }
            let build = aot::build_checked_input(&context, parsed.output)?;
            println!("executable: {}", build.executable.display());
            println!("object: {}", build.object.display());
            println!("runtime metadata: {}", build.metadata.display());
            Ok(())
        }
        Some("verify-aot") => {
            let mut path = None;
            let mut input_options = input::Options::default();
            let mut native = rinzu_compiler::NativeCliInputs::default();
            let mut cancel_worker_after_polls = None;
            let mut options = rinzu_codegen::BaselineOptions::default();
            while let Some(argument) = arguments.next() {
                match argument.as_str() {
                    "--language" | "--target" | "--native-object" | "--native-header" | "-L"
                    | "-l" => {
                        let value = arguments
                            .next()
                            .filter(|value| !value.is_empty() && !value.starts_with('-'))
                            .ok_or_else(|| format!("{argument} requires a value"))?;
                        match argument.as_str() {
                            "--language" | "--target" => {
                                let slot = if argument == "--language" {
                                    &mut input_options.language
                                } else {
                                    &mut input_options.target
                                };
                                if slot.is_some() {
                                    return Err(format!("duplicate {argument}"));
                                }
                                *slot = Some(value);
                            }
                            "--native-object" => native.objects.push(PathBuf::from(value)),
                            "--native-header" => native.headers.push(PathBuf::from(value)),
                            "-L" => native.search_paths.push(PathBuf::from(value)),
                            "-l" => native.libraries.push(value),
                            _ => unreachable!(),
                        }
                    }
                    "--" => {
                        if arguments.next().is_some() {
                            return Err(
                                "program arguments are not supported in this language revision"
                                    .into(),
                            );
                        }
                        break;
                    }
                    "--cancel-after-polls" => {
                        options.cancel_after_polls =
                            Some(parse_limit(arguments.next(), "--cancel-after-polls")?);
                    }
                    "--cancel-worker-after-polls" => {
                        cancel_worker_after_polls = Some(parse_limit(
                            arguments.next(),
                            "--cancel-worker-after-polls",
                        )?);
                    }
                    "--max-allocations" => {
                        options.max_allocations =
                            Some(parse_limit(arguments.next(), "--max-allocations")?);
                    }
                    "--shutdown-after-polls" => {
                        options.shutdown_after_polls =
                            Some(parse_limit(arguments.next(), "--shutdown-after-polls")?);
                    }
                    _ if argument.starts_with("-L") && argument.len() > 2 => {
                        native.search_paths.push(PathBuf::from(&argument[2..]))
                    }
                    _ if argument.starts_with("-l") && argument.len() > 2 => {
                        native.libraries.push(argument[2..].into())
                    }
                    _ if argument.starts_with('-') || argument.starts_with('@') => {
                        return Err(format!("unsupported verify-aot flag `{argument}`"));
                    }
                    _ if path.is_none() => path = Some(PathBuf::from(argument)),
                    _ => return Err(format!("unexpected argument `{argument}`")),
                }
            }
            let terminal_controls = [
                options.cancel_after_polls,
                options.max_allocations,
                options.shutdown_after_polls,
                cancel_worker_after_polls,
            ]
            .into_iter()
            .flatten()
            .count();
            if terminal_controls > 1 {
                return Err(
                    "verify-aot accepts only one cancel, overload, or shutdown control".into(),
                );
            }
            let path =
                path.map_or_else(|| env::current_dir().map_err(|error| error.to_string()), Ok)?;
            let report = verify_capture::verify_with_input(
                &path,
                &input_options,
                &native,
                options,
                cancel_worker_after_polls,
            )?;
            println!(
                "JIT/AOT equivalent: result={}, outcome={}, resume-domain=main, resources=baseline, shutdown=complete",
                report.result, report.outcome
            );
            Ok(())
        }
        Some("verify-appkit") => {
            let mut path = None;
            let mut output = None;
            while let Some(argument) = arguments.next() {
                if argument == "--output" {
                    output = Some(PathBuf::from(arguments.next().ok_or_else(|| {
                        "--output requires an application bundle".to_owned()
                    })?));
                } else if path.is_none() {
                    path = Some(PathBuf::from(argument));
                } else {
                    return Err(format!("unexpected argument `{argument}`"));
                }
            }
            let path =
                path.map_or_else(|| env::current_dir().map_err(|error| error.to_string()), Ok)?;
            let output = output.unwrap_or_else(|| path.join("build/v0_2_appkit.app"));
            let report = appkit::verify(&path, output)?;
            println!("AppKit JIT/AOT equivalent: result={}", report.result);
            println!("static handler callbacks: {}", report.callbacks);
            println!("application bundle: {}", report.bundle.display());
            Ok(())
        }
        Some("verify-async") => {
            let path = arguments.next().map_or_else(
                || env::current_dir().map_err(|error| error.to_string()),
                |value| Ok(PathBuf::from(value)),
            )?;
            if let Some(extra) = arguments.next() {
                return Err(format!("unexpected argument `{extra}`"));
            }
            input::require_package("verify-async", &path)?;
            let (manifest, artifact) =
                rinzu_compiler::check_package(&path).map_err(|diagnostics| {
                    print_diagnostics(diagnostics);
                    "package failed to compile".to_owned()
                })?;
            let options = rinzu_codegen::SourceAsyncOptions::from_sdk_packages(
                &manifest.root,
                &artifact.sdk_packages,
            )
            .map_err(|error| format!("configure source async runtime: {error:#}"))?;
            let run = rinzu_codegen::run_source_async(&artifact.ir, "main", options)
                .map_err(|error| format!("source async runtime failed: {error:#}"))?;
            eprintln!(
                "source async complete: result={}, last-awaited={}, resources={}",
                run.result,
                run.last_awaited_result
                    .map_or_else(|| "none".to_owned(), |value| value.to_string()),
                if run.resources_returned {
                    "baseline"
                } else {
                    "leaked"
                }
            );
            eprintln!("source async events: {}", run.events.len());
            Ok(())
        }
        Some("dev") => {
            let (path, options) = dev::parse(arguments)?;
            dev::run_dev(path, options)
        }
        Some("release") => {
            let mut path = None;
            let mut output = None;
            while let Some(argument) = arguments.next() {
                if argument == "--output" {
                    output = Some(PathBuf::from(
                        arguments
                            .next()
                            .ok_or_else(|| "--output requires a directory".to_owned())?,
                    ));
                } else if path.is_none() {
                    path = Some(PathBuf::from(argument));
                } else {
                    return Err(format!("unexpected argument `{argument}`"));
                }
            }
            let path =
                path.map_or_else(|| env::current_dir().map_err(|error| error.to_string()), Ok)?;
            let output = output.unwrap_or_else(|| path.join("build/release-v0.1"));
            let artifact = release::build_release(&path, output)?;
            println!("release directory: {}", artifact.directory.display());
            println!("executable: {}", artifact.executable.display());
            println!("manifest: {}", artifact.manifest.display());
            Ok(())
        }
        Some("help" | "--help" | "-h") | None => {
            println!(
                "Rinzu toolchain\n\nCommands:\n  editor --stdio [workspace-path] (versioned JSON lines)\n  fmt <file|directory>... [--check | --write] (use - for stdin)\n  check [file|directory] [input options] [native inputs]\n  run [file|directory] [input options] [--trace] [--diagnostic-report file] [resource options] [native inputs]\n  diagnose report.json [--resources file] [--native-log file] [--trace file] [--exit-record file] [--format text|json]\n  test [path] [resource options] [native inputs] (run test package main)\n  test [path] --unit [--doc file] [--list] [--filter text] [--format text|json] [--timeout-ms n]\n  test [path] --doc file (explicit rinzu-test blocks)\n  resource options: --resource-report file, --resource-interval-ms n (default 100; 0=start/end)\n  build [file|directory] [input options] [--output file] [native inputs]\n  bundle [package] [--config macos-app.json] [--output path.app] [native inputs]\n  bundle-verify path.app (inventory + code-signature integrity)\n  lock [path] [--check] [native inputs]\n  api [SDK path] [--output file] [--compare previous.json] [--markdown file] [native inputs]\n  input options: --language 0.5 (file only), --target host-target\n  file mode: standard/local imports, ordinary native modules and async standard libraries; no manifest needed\n  native inputs (file/package, macOS arm64): -L path, -l name, --native-object path\n  optional ABI headers (file/package): --native-header path; no header required\n  native scope: empty by default; System symbols require explicit -lSystem; -L/headers do not grant access\n  native validation: unresolved/duplicate/target/header mismatch rejected; no guarantee for wrong ABI/native faults\n  AOT distribution: move executable with .source-packages/.resources/native artifacts; custom dylibs bundled; System OS UUID must match\n  migration: rebuild source-package 2.5; SDK cache v4; refresh package locks explicitly\n  verify-aot [file|directory] [input options] [native inputs] [--cancel-after-polls n | --cancel-worker-after-polls n | --max-allocations n | --shutdown-after-polls n]\n  extra program arguments are unsupported; main has no parameters\n  package-only commands below and test/lock/api/editor workspace require a directory; editor initialize.entry selects a file\n  verify-appkit [path] [--output bundle]\n  verify-async [path]\n  dev [path] [--once] [--poll-ms n]\n  dev [path] --restart cooperative [--shutdown-timeout-ms n] [--on-timeout force] [--stdin null]\n  release [path] [--output directory]\n  toolchain-bundle --output directory (bin/rinzu + share/rinzu)\n  standard imports: core/stdio/timer/io/process/stream/testing/shutdown/http/socket; no SDK path needed\n  doctor\n  run-empty\n  --version"
            );
            Ok(())
        }
        Some(other) => Err(format!("unknown command `{other}`")),
    }
}

fn parse_limit(value: Option<String>, option: &str) -> Result<usize, String> {
    let value = value.ok_or_else(|| format!("{option} requires a number"))?;
    let limit = value
        .parse::<usize>()
        .map_err(|error| format!("invalid limit `{value}` for {option}: {error}"))?;
    if limit == 0 {
        return Err(format!("{option} requires a positive number"));
    }
    Ok(limit)
}

fn run_package_with_native_inputs(
    path: PathBuf,
    show_trace: bool,
    inputs: &rinzu_compiler::NativeCliInputs,
    resource_config: Option<&rinzu_runtime::resource_report::Config>,
) -> Result<(), String> {
    PROGRAM_EXIT_STATUS.with(|status| status.set(0));
    let (manifest, artifact) = match rinzu_compiler::check_package_with_native_inputs(&path, inputs)
    {
        Ok(compiled) => compiled,
        Err(diagnostics) => {
            print_diagnostics(diagnostics);
            return Err("package failed to compile".into());
        }
    };
    run_checked_package(&manifest, &artifact, show_trace, resource_config)
}

fn run_checked_package(
    manifest: &rinzu_compiler::Manifest,
    artifact: &rinzu_compiler::FrontendArtifact,
    show_trace: bool,
    resource_config: Option<&rinzu_runtime::resource_report::Config>,
) -> Result<(), String> {
    let uses_c =
        aot::main_uses_source_async(artifact) && !artifact.ir.source_native_signatures.is_empty();
    let mut report = if !uses_c {
        resource_config
            .map(|config| {
                rinzu_runtime::resource_report::Session::start(
                    config,
                    "rust-jit",
                    rinzu_codegen::resource_source_files(&artifact.ir),
                )
            })
            .transpose()
            .map_err(|error| format!("start resource report: {error}"))?
    } else {
        None
    };
    let outcome = run_package_engines(manifest, artifact, show_trace, resource_config, &mut report);
    if let Some(report) = report {
        report
            .finish()
            .map_err(|error| format!("finish resource report: {error}"))?;
    }
    outcome
}

fn run_package_engines(
    manifest: &rinzu_compiler::Manifest,
    artifact: &rinzu_compiler::FrontendArtifact,
    show_trace: bool,
    resource_config: Option<&rinzu_runtime::resource_report::Config>,
    report: &mut Option<rinzu_runtime::resource_report::Session>,
) -> Result<(), String> {
    let main_facts = artifact
        .checked
        .functions
        .iter()
        .find(|function| function.declaration.name == "main")
        .map(|function| &function.facts);
    let represented_exports = artifact
        .sdk_packages
        .iter()
        .flat_map(|package| &package.native_imports)
        .map(|import| import.export_name.as_str())
        .chain(
            artifact
                .ir
                .source_native_signatures
                .iter()
                .map(|signature| signature.name.as_str()),
        )
        .collect::<std::collections::HashSet<_>>();
    let has_source_packages = artifact.source_interface.is_some()
        || artifact
            .sdk_packages
            .iter()
            .any(|package| package.source.is_some());
    let has_native_artifacts = artifact
        .sdk_packages
        .iter()
        .flat_map(|package| &package.native_imports)
        .any(|import| import.runtime_service.is_none());
    let source_async = main_facts.is_some_and(|facts| {
        aot::main_uses_source_async(artifact)
            && facts
                .native_requirements
                .iter()
                .all(|requirement| represented_exports.contains(requirement.as_str()))
    });
    if source_async {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if !artifact.ir.source_native_signatures.is_empty() {
            let execution = if show_trace {
                compiled_async::run_with_observation(manifest, artifact, resource_config, true)?
            } else if resource_config.is_some() {
                compiled_async::run_with_resource_report(manifest, artifact, resource_config)?
            } else {
                compiled_async::run(manifest, artifact, None, None, false)?
            };
            diagnostics::observe_compiled(&execution);
            if !execution.retired
                || execution.rwx_mappings != 0
                || execution.executable_mappings == 0
                || execution.icache_flushes != execution.executable_mappings
                || execution.rx_transitions != execution.executable_mappings
                || execution.compile.code_size == 0
            {
                return Err("compiled async code publication or retirement failed".into());
            }
            package_tests::observe_compiled(&execution);
            let failed = execution
                .output
                .lines()
                .any(|line| line.starts_with("RINZU_FAILURE=") && line != "RINZU_FAILURE=none");
            if execution.exit_code != 0 && failed {
                let message = execution
                    .output
                    .lines()
                    .find_map(|line| line.strip_prefix("RINZU_FAILURE_MESSAGE="))
                    .ok_or("compiled async report lost its failure message")?;
                return Err(format!(
                    "compiled async runtime failed: {message}\n{}",
                    execution.output
                ));
            }
            eprintln!(
                "ran {} with compiled JIT continuation runtime",
                manifest.name
            );
            if show_trace {
                let trace = execution
                    .output
                    .lines()
                    .find_map(|line| line.strip_prefix("RINZU_TRACE="))
                    .ok_or("compiled async report lost its trace")?;
                let trace: TraceDocument =
                    serde_json::from_str(trace).map_err(|error| error.to_string())?;
                eprintln!(
                    "{}",
                    serde_json::to_string_pretty(&trace).map_err(|error| error.to_string())?
                );
            }
            let result = execution
                .output
                .lines()
                .find_map(|line| line.strip_prefix("RINZU_RESULT="))
                .ok_or("compiled async report lost its result")?;
            eprintln!("result: {result}");
            let status =
                u8::try_from(execution.exit_code).map_err(|_| "invalid process exit status")?;
            PROGRAM_EXIT_STATUS.with(|value| value.set(status));
            eprintln!("exit status: {status}");
            return Ok(());
        }
        let mut options = rinzu_codegen::SourceAsyncOptions::from_sdk_packages(
            &manifest.root,
            &artifact.sdk_packages,
        )
        .map_err(|error| format!("configure source async runtime: {error:#}"))?;
        let built =
            if has_native_artifacts || has_source_packages || !artifact.native_inputs.is_empty() {
                let built = native_package::build_artifact(manifest, artifact)?;
                built.validate_source_packages()?;
                source_package_bundle::save(
                    &built.source_packages,
                    &input::publication_directory(manifest, artifact, &built.directory),
                )?;
                options.native_packages = Some(
                    rinzu_codegen::LoadedNativePackages::load(&built.artifacts)
                        .map_err(|error| format!("load native SDK packages: {error:#}"))?,
                );
                options.source_packages = Some(built.load_source_packages()?);
                Some(built)
            } else {
                None
            };
        let execute = if package_tests::is_worker() || diagnostics::is_active() {
            rinzu_codegen::run_source_async_report
        } else {
            rinzu_codegen::run_source_async
        };
        let execution = execute(&artifact.ir, "main", options)
            .map_err(|error| format!("source async runtime failed: {error:#}"));
        if let Some(built) = built {
            let _ = std::fs::remove_dir_all(built.directory);
        }
        let execution = execution?;
        package_tests::observe_source(&execution);
        if let Some(report) = report {
            report.end_categories(resource_report::native_end(execution.native_calls));
        }
        eprintln!("ran {} with baseline continuation runtime", manifest.name);
        if show_trace {
            eprintln!(
                "{}",
                serde_json::to_string_pretty(&normalized_source_async_trace(&execution))
                    .map_err(|error| format!("serialize normalized trace: {error}"))?
            );
        }
        eprintln!("result: {}", execution.result);
        if let Some(failure) = &execution.failure_details {
            eprint!(
                "{}",
                rinzu_codegen::format_error_traces(&artifact.ir, failure)
            );
        }
        if diagnostics::is_active()
            && let Some(failure) = &execution.failure_details
            && !execution.shutdown_complete
        {
            return Err(format!(
                "source async runtime failed: {}",
                failure.primary.cause.message()
            ));
        }
        PROGRAM_EXIT_STATUS.with(|value| value.set(execution.exit_status));
        eprintln!("exit status: {}", execution.exit_status);
        return Ok(());
    }
    let execution =
        if has_native_artifacts || has_source_packages || !artifact.native_inputs.is_empty() {
            let built = native_package::build_artifact(manifest, artifact)?;
            built.validate_source_packages()?;
            source_package_bundle::save(
                &built.source_packages,
                &input::publication_directory(manifest, artifact, &built.directory),
            )?;
            let loaded = rinzu_codegen::LoadedNativePackages::load(&built.artifacts)
                .map_err(|error| format!("load native SDK packages: {error:#}"))?;
            let execution = rinzu_codegen::run_baseline_jit_with_package_bindings(
                &artifact.ir,
                "main",
                rinzu_codegen::BaselineOptions::default(),
                Some(loaded),
                Some(built.load_source_packages()?),
            )
            .map_err(|error| format!("baseline native-package JIT failed: {error:#}"));
            let _ = std::fs::remove_dir_all(&built.directory);
            execution?
        } else {
            rinzu_codegen::run_baseline_jit(&artifact.ir, "main")
                .map_err(|error| format!("baseline JIT failed: {error:#}"))?
        };
    eprintln!("ran {} with baseline JIT", manifest.name);
    package_tests::observe_baseline(&execution);
    if let Some(report) = report {
        report.end_categories(resource_report::native_end(execution.native_calls));
    }
    if show_trace {
        let trace = normalized_trace(&execution);
        eprintln!(
            "{}",
            serde_json::to_string_pretty(&trace)
                .map_err(|error| format!("serialize normalized trace: {error}"))?
        );
    }
    match execution.result {
        Ok(value) => {
            eprintln!("result: {value}");
            eprintln!("exit status: 0");
            Ok(())
        }
        Err(failure) => {
            eprintln!("runtime diagnostic: {failure:?}");
            if let Some(details) = &execution.failure_details {
                eprint!(
                    "{}",
                    rinzu_codegen::format_error_traces(&artifact.ir, details)
                );
            }
            eprintln!("exit status: 1");
            Err("program execution failed".into())
        }
    }
}

pub(crate) fn normalized_source_async_trace(run: &rinzu_codegen::SourceAsyncRun) -> TraceDocument {
    let mut events = vec![TraceEvent {
        sequence: 0,
        domain: "main".into(),
        task: Some("main".into()),
        source: None,
        event: EventKind::TaskStart,
        observation: None,
    }];
    for event in &run.events {
        use rinzu_codegen::SourceAsyncEvent::*;
        let (scope, event) = match event {
            Suspended {
                scope, operation, ..
            } => (
                *scope,
                EventKind::AwaitPending {
                    operation: operation.clone(),
                },
            ),
            Awaited { scope, future } => (
                *scope,
                EventKind::AwaitReady {
                    operation: format!("future:{future}"),
                },
            ),
            NativeEntered { scope, .. } => (
                *scope,
                EventKind::NativeEnter {
                    transition: rinzu_trace::NativeTransition::NativeSafe,
                },
            ),
            CallbackEntered { scope, .. } => (
                *scope,
                EventKind::NativeEnter {
                    transition: rinzu_trace::NativeTransition::CallbackManaged,
                },
            ),
            CallbackLeft { scope, .. } => (
                *scope,
                EventKind::NativeLeave {
                    transition: rinzu_trace::NativeTransition::CallbackManaged,
                },
            ),
            NativeLeft { scope, .. } => (
                *scope,
                EventKind::NativeLeave {
                    transition: rinzu_trace::NativeTransition::NativeSafe,
                },
            ),
            Cancelled { .. }
            | Resumed { .. }
            | Submitted { .. }
            | FileCompleted { .. }
            | TimerCompleted { .. }
            | WorkerCompleted { .. } => {
                continue;
            }
        };
        events.push(TraceEvent {
            sequence: events.len() as u64,
            domain: "main".into(),
            task: Some(if scope == 0 {
                "main".into()
            } else {
                format!("main:{scope}")
            }),
            source: None,
            event,
            observation: None,
        });
    }
    if !events.iter().any(|event| {
        matches!(
            event.event,
            EventKind::TaskComplete {
                outcome: TaskOutcome::Cancelled
            }
        )
    }) {
        events.push(TraceEvent {
            sequence: events.len() as u64,
            domain: "main".into(),
            task: Some("main".into()),
            source: None,
            event: EventKind::TaskComplete {
                outcome: match run
                    .failure_details
                    .as_ref()
                    .map(|failure| failure.primary.cause)
                {
                    None => TaskOutcome::Success,
                    Some(rinzu_runtime::failure::FailureCause::Cancelled) => TaskOutcome::Cancelled,
                    Some(_) => TaskOutcome::Failure,
                },
            },
            observation: None,
        });
    }
    TraceDocument {
        schema: TRACE_SCHEMA.into(),
        version: TRACE_VERSION,
        producer: "rinzu-source-async".into(),
        context: BTreeMap::from([
            ("resume-domain".into(), "main".into()),
            (
                "resources".into(),
                if run.resources_returned {
                    "baseline".into()
                } else {
                    "leaked".into()
                },
            ),
        ]),
        events,
    }
}

fn print_diagnostics(diagnostics: Vec<rinzu_compiler::Diagnostic>) {
    for diagnostic in diagnostics {
        eprintln!(
            "{}: {} (bytes {}..{})",
            diagnostic.code.as_str(),
            diagnostic.message,
            diagnostic.primary.start_byte,
            diagnostic.primary.end_byte
        );
        for suggestion in diagnostic.suggestions {
            eprintln!("  help: {}", suggestion.message);
        }
    }
}

pub(crate) fn normalized_trace(run: &BaselineRun) -> TraceDocument {
    if let Some(trace) = &run.host_trace {
        return trace.clone();
    }
    let outcome = match &run.result {
        Ok(_) => TaskOutcome::Success,
        Err(BaselineFailure::Cancelled) => TaskOutcome::Cancelled,
        Err(BaselineFailure::Shutdown) => TaskOutcome::Success,
        Err(_) => TaskOutcome::Failure,
    };
    let mut context = BTreeMap::new();
    context.insert("engine".into(), "baseline-jit".into());
    context.insert("poll-count".into(), run.poll_count.to_string());
    context.insert("allocation-count".into(), run.allocation_count.to_string());
    context.insert(
        "callback-dispatches".into(),
        run.callback_dispatches.to_string(),
    );
    context.insert("resume-domain".into(), "main".into());
    context.insert("resource-baseline".into(), "true".into());
    context.insert("shutdown".into(), "complete".into());
    TraceDocument {
        schema: TRACE_SCHEMA.into(),
        version: TRACE_VERSION,
        producer: format!("rinzu {}", env!("CARGO_PKG_VERSION")),
        context,
        events: vec![
            TraceEvent {
                sequence: 0,
                domain: "main".into(),
                task: Some("main".into()),
                source: None,
                event: EventKind::TaskStart,
                observation: None,
            },
            TraceEvent {
                sequence: 1,
                domain: "main".into(),
                task: Some("main".into()),
                source: None,
                event: EventKind::TaskComplete { outcome },
                observation: None,
            },
        ],
    }
}
