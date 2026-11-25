//! Compiled Main continuations share the C runtime used by AOT, while managed
//! bodies and physical ABI stubs are finalized Cranelift JIT code.
use rinzu_abi::target::Target;
#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "x86_64", target_env = "gnu")
))]
use rinzu_codegen::BaselineOptions;
use rinzu_codegen::{
    BaselineAotObject, CompiledRuntimeLibrary, PreparedAsyncJit, emit_source_async_aot_plans,
};
use rinzu_compiler::{FrontendArtifact, Manifest, SourceNativeSignature};
use std::collections::{BTreeMap, HashMap};
use std::ffi::{CString, OsString, c_char};
use std::path::{Path, PathBuf};

mod managed_cache;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod native_preload;
mod runtime_cache;
pub(crate) mod source;

pub struct CompiledAsyncRun {
    pub output: String,
    #[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
    pub source_files: BTreeMap<u64, String>,
    pub exit_code: i32,
    pub compile: rinzu_codegen::CompileEvidence,
    pub executable_mappings: usize,
    pub icache_flushes: usize,
    pub rx_transitions: usize,
    pub rwx_mappings: usize,
    pub retired: bool,
}
#[cfg(test)]
thread_local! {static PREPARATION_PROBE: std::cell::Cell<bool> = const {std::cell::Cell::new(false)};}
#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
pub(crate) fn enable_preparation_probe() {
    PREPARATION_PROBE.with(|probe| probe.set(true));
}
#[cfg(test)]
thread_local! {static FOREIGN_RETURN_PROBE: std::cell::Cell<Option<usize>> = const {std::cell::Cell::new(None)};}
#[cfg(test)]
thread_local! {static ERROR_TRACE_PROBE: std::cell::Cell<bool> = const {std::cell::Cell::new(false)};}
#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
pub(crate) fn configure_error_trace_probe() {
    ERROR_TRACE_PROBE.with(|probe| probe.set(true));
}
#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
pub(crate) fn configure_foreign_return_probe(count: usize) {
    FOREIGN_RETURN_PROBE.with(|probe| probe.set(Some(count)));
}
fn native_signatures(aot: &BaselineAotObject) -> Vec<&SourceNativeSignature> {
    let mut signatures = aot
        .native_signatures
        .iter()
        .filter(|signature| !signature.is_async)
        .collect::<Vec<_>>();
    signatures.sort_by(|a, b| a.name.cmp(&b.name));
    signatures
}
pub(crate) fn runtime_source(
    aot: &BaselineAotObject,
    source: &str,
    record_evidence: bool,
) -> String {
    runtime_source_with_locations(aot, source, record_evidence, None)
}

pub(crate) fn runtime_source_with_locations(
    aot: &BaselineAotObject,
    source: &str,
    record_evidence: bool,
    locations: Option<&Path>,
) -> String {
    let signatures = native_signatures(aot);
    let mut prelude = source::template(
        "compiled_async.c",
        include_str!("compiled_async.c"),
        locations,
    )
    .replace(
        "/* JIT_ENTRY_COUNTS */",
        &format!(
            "#define RINZU_JIT_FUNCTION_COUNT {}\n#define RINZU_JIT_STUB_COUNT {}\n",
            aot.functions.len(),
            signatures.len()
        ),
    );
    // Cast at the C call sites so Clang only compiles the actual dispatches.
    // The generated table and Cranelift share the same function indices.
    for function in &aot.functions {
        let types = std::iter::once("void *")
            .chain(std::iter::repeat_n("int64_t", function.parameter_count))
            .collect::<Vec<_>>()
            .join(",");
        prelude.push_str(&format!(
            "#define {} (*(int64_t (*)({types}))rinzu_jit_functions[{}])\n",
            function.symbol, function.index
        ));
    }
    // Supplant plans store a constant C function pointer. Keep a wrapper for
    // those physical ABI stubs; managed calls use their JIT addresses directly.
    for (index, _) in signatures
        .iter()
        .enumerate()
        .filter(|(_, signature)| signature.is_supplant)
    {
        prelude.push_str(&format!("uint64_t rinzu_source_native_{index}(uint64_t address,const uint64_t *words) {{return ((uint64_t (*)(uint64_t,const uint64_t *))rinzu_jit_stubs[{index}])(address,words);}}\n"));
    }
    // This library executes inside its host process. Only the CLI's standalone
    // startup owns SIGINT policy; embedding must not replace a host handler.
    let source = source.replace("  rinzu_standalone_interrupt_normalize();\n", "");
    let mut source = source.replace(
        "int main(int argc, char **argv) {",
        "static int rinzu_jit_main(int argc,char **argv) {",
    );
    assert_eq!(
        source.matches("rinzu_aot_source_native_startup();").count(),
        1
    );
    source = source.replace("rinzu_aot_source_native_startup();", "{ uint64_t started = rinzu_jit_timing ? rinzu_jit_clock() : 0; rinzu_aot_source_native_startup(); if (rinzu_jit_timing) { uint64_t ended = rinzu_jit_clock(); rinzu_jit_timing->native_startup = started && ended >= started ? ended - started : 0; } rinzu_jit_publish(); }");
    source = source.replace(
        "char *executable_directory = rinzu_source_executable_directory();",
        "char *executable_directory = rinzu_jit_directory_copy();",
    );
    source=source.replace("const char *cancel = getenv(\"RINZU_CANCEL_AFTER_POLLS\"); if (cancel) context.cancel_after = strtoull(cancel, NULL, 10);", "context.cancel_after = rinzu_jit_cancel_after; context.shutdown_after=rinzu_jit_shutdown_after;");
    source=source.replace("const char *shutdown = getenv(\"RINZU_SHUTDOWN_AFTER_POLLS\"); if (shutdown) context.shutdown_after = strtoull(shutdown, NULL, 10);", "");
    source=source.replace("const char *cancel = getenv(\"RINZU_CANCEL_WORKER_AFTER_POLLS\"); if (cancel) context.cancel_after = strtoull(cancel, NULL, 10);", "context.cancel_after = rinzu_jit_cancel_worker_after;");
    source = source.replace(
        "async.runtime.record_evidence = getenv(\"RINZU_TRANSPORT_EVIDENCE\") != NULL;",
        if record_evidence {
            "async.runtime.record_evidence = 1;"
        } else {
            "async.runtime.record_evidence = 0;"
        },
    );
    source = source
        .replace("\"rinzu-aot\"", "\"rinzu-jit\"")
        .replace("\\\"rinzu-aot\\\"", "\\\"rinzu-jit\\\"");
    if !aot.native_call_plans.is_empty() {
        prelude.push_str("RINZU_PROGRAM_API char *rinzu_jit_directory_copy(void) {size_t length=strlen(rinzu_jit_directory);if(length==SIZE_MAX)_Exit(71);char *copy=malloc(length+1);if(!copy)_Exit(71);memcpy(copy,rinzu_jit_directory,length+1);return copy;}\n");
    }
    /* Keep the executable-path utility referenced while replacing its source
    bundle caller with the runtime's explicit, validated directory. */
    if !aot.native_call_plans.is_empty() {
        source=source.replace("static int rinzu_jit_main(int argc,char **argv) {", "static int rinzu_jit_main(int argc,char **argv) {\n(void)rinzu_source_executable_directory;");
    }
    format!(
        "{prelude}\n{source}\n{}",
        source::template(
            "compiled_async_entry.c",
            include_str!("compiled_async_entry.c"),
            locations
        )
    )
}
fn checked<T>(value: std::io::Result<T>) -> Result<T, String> {
    rinzu_runtime::allocation::checked_io(value).map_err(|error| error.to_string())
}
#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
pub fn run(
    manifest: &Manifest,
    artifact: &FrontendArtifact,
    cancel_after: Option<usize>,
    shutdown_after: Option<usize>,
    record_evidence: bool,
) -> Result<CompiledAsyncRun, String> {
    run_configured(
        manifest,
        artifact,
        RunOptions {
            cancel_after,
            shutdown_after,
            record_evidence,
            ..Default::default()
        },
    )
}
#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
pub fn run_with_resource_report(
    manifest: &Manifest,
    artifact: &FrontendArtifact,
    resource_config: Option<&rinzu_runtime::resource_report::Config>,
) -> Result<CompiledAsyncRun, String> {
    run_with_observation(
        manifest,
        artifact,
        resource_config,
        false,
        &[],
        Default::default(),
    )
}
pub(crate) fn run_with_observation(
    manifest: &Manifest,
    artifact: &FrontendArtifact,
    resource_config: Option<&rinzu_runtime::resource_report::Config>,
    show_trace: bool,
    arguments: &[OsString],
    preparation: crate::RunPreparation<'_>,
) -> Result<CompiledAsyncRun, String> {
    run_configured(
        manifest,
        artifact,
        RunOptions {
            dependencies: preparation.dependencies,
            frontend_generation: preparation.frontend_generation,
            on_ready: preparation.on_ready,
            record_evidence: show_trace,
            resource_config,
            arguments,
            ..Default::default()
        },
    )
}

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "x86_64", target_env = "gnu")
))]
pub(crate) fn run_for_equivalence(
    manifest: &Manifest,
    artifact: &FrontendArtifact,
    options: BaselineOptions,
    cancel_worker_after: Option<usize>,
    arguments: &[OsString],
) -> Result<CompiledAsyncRun, String> {
    run_configured(
        manifest,
        artifact,
        RunOptions {
            cancel_after: options.cancel_after_polls,
            cancel_worker_after,
            shutdown_after: options.shutdown_after_polls,
            record_evidence: true,
            capture_error_trace: options.capture_error_trace,
            arguments,
            ..Default::default()
        },
    )
}

#[derive(Default)]
struct RunOptions<'a, 'ready> {
    dependencies: Option<crate::native_package::PreparedDependencies>,
    frontend_generation: Option<&'a str>,
    on_ready: Option<&'ready mut dyn FnMut()>,
    cancel_after: Option<usize>,
    cancel_worker_after: Option<usize>,
    shutdown_after: Option<usize>,
    record_evidence: bool,
    capture_error_trace: bool,
    resource_config: Option<&'a rinzu_runtime::resource_report::Config>,
    arguments: &'a [OsString],
}

fn run_configured(
    manifest: &Manifest,
    artifact: &FrontendArtifact,
    mut options: RunOptions<'_, '_>,
) -> Result<CompiledAsyncRun, String> {
    let dependencies = options.dependencies.take();
    let _timing = rinzu_compiler::timing::span("jit.total");
    crate::jit_progress::phase("Compiling JIT code");
    let native_bindings = artifact
        .sdk_packages
        .iter()
        .flat_map(|package| &package.native_imports)
        .map(|import| (import.export_name.clone(), import.import_key))
        .collect::<HashMap<_, _>>();
    let plans_timing = rinzu_compiler::timing::span("jit.plans");
    let aot =
        emit_source_async_aot_plans(&artifact.ir, "main", &native_bindings).map_err(|error| {
            crate::cli_diagnostics::stage_failure(
                Some(&manifest.root.join(&manifest.entry)),
                "async runtime code generation",
                None,
                &format!("{error:#}"),
            )
        })?;
    let plan =
        crate::aot::async_aot_plan(artifact, &aot)?.ok_or("compiled async requires async Main")?;
    drop(plans_timing);
    // Lowering does not resolve symbols or publish executable memory. Native
    // preparation stays on the caller's thread, as do linking and execution.
    let symbols = runtime_symbol_names(&aot);
    let (runtime, (code, pending)) = std::thread::scope(|scope| {
        let prepare_managed = || {
            let _timing = rinzu_compiler::timing::span("jit.managed");
            managed_cache::prepare(
                manifest,
                artifact,
                &aot,
                &native_bindings,
                &symbols,
                options.frontend_generation,
            )
            .map_err(|error| {
                crate::cli_diagnostics::stage_failure(
                    None,
                    "async JIT code generation",
                    None,
                    &format!("{error:#}"),
                )
            })
        };
        let worker = if std::thread::available_parallelism().is_ok_and(|count| count.get() > 1) {
            std::thread::Builder::new()
                .name("rinzu-jit".into())
                .stack_size(8 * 1024 * 1024)
                .spawn_scoped(scope, prepare_managed)
                .ok()
        } else {
            None
        };
        let runtime = prepare_runtime(manifest, artifact, &aot, &plan, &options, dependencies);
        let managed = match worker {
            Some(worker) => worker
                .join()
                .map_err(|_| "async JIT preparation panicked".to_owned())?,
            None => prepare_managed(),
        };
        Ok::<_, String>((runtime?, managed?))
    })?;
    run_image(
        &manifest.root,
        aot,
        runtime,
        code,
        symbols,
        pending,
        options,
    )
}

struct PreparedRuntime {
    directory: crate::verify_capture::TemporaryDirectory,
    library: PathBuf,
    sdk_cache: Vec<crate::source_native_package::PendingEarlySdkCache>,
    runtime_cache: Option<runtime_cache::Pending>,
}

/// C calls this transaction once, after its native startup has validated all
/// descriptors/providers and before managed Main can run or terminate us.
struct Publication<'runtime, 'ready> {
    managed: Option<managed_cache::Pending>,
    runtime: &'runtime mut PreparedRuntime,
    on_ready: Option<&'ready mut dyn FnMut()>,
}

extern "C" fn publish_caches(raw: *mut std::ffi::c_void) {
    // SAFETY: run_image retains this exclusive stack value throughout the
    // synchronous rinzu_jit_run call; the private C API calls it only once.
    let publication = unsafe { &mut *raw.cast::<Publication<'_, '_>>() };
    if let Some(cache) = publication.managed.take() {
        cache.store();
    }
    for cache in publication.runtime.sdk_cache.drain(..) {
        cache.store();
    }
    if let Some(cache) = publication.runtime.runtime_cache.take() {
        cache.store();
    }
    if let Some(publish) = publication.on_ready.take() {
        publish();
    }
}

fn prepare_runtime(
    manifest: &Manifest,
    artifact: &FrontendArtifact,
    aot: &BaselineAotObject,
    plan: &crate::aot::AsyncAotPlan,
    options: &RunOptions<'_, '_>,
    dependencies: Option<crate::native_package::PreparedDependencies>,
) -> Result<PreparedRuntime, String> {
    crate::jit_progress::phase("Preparing native dependencies");
    let mut built =
        crate::native_package::build_artifact_with_dependencies(manifest, artifact, dependencies)?;
    crate::jit_progress::phase("Preparing JIT runtime");
    let sdk_cache = built
        .source_packages
        .iter_mut()
        .filter_map(|package| package.pending_early_cache.take())
        .collect();
    let directory = crate::verify_capture::TemporaryDirectory(built.directory.clone());
    let imports = artifact
        .sdk_packages
        .iter()
        .flat_map(|package| &package.native_imports)
        .filter(|import| import.runtime_service.is_none())
        .cloned()
        .collect::<Vec<_>>();
    let bundle = built.directory.join("source-packages");
    let publication = crate::input::publication_directory(manifest, artifact, &built.directory);
    let publish = || {
        crate::source_package_bundle::save_to(&built.source_packages, &[&bundle, &publication])
            .map(|_| ())
    };
    let resources = rinzu_compiler::standard_library::resource_root()?;
    let toolchain = crate::native_clang::TargetClang::resolve()?;
    let compile = || -> Result<(PathBuf, Option<runtime_cache::Pending>), String> {
        let directory = &built.directory;
        let source_path = directory.join("compiled-async.runtime.c");
        let units = RuntimeUnit::all(aot, &source_path, directory, &resources);
        let objects: Vec<_> = units.iter().map(|unit| unit.object.clone()).collect();
        let library_name = aot.target.shared_library_name("compiled-async");
        let library = directory.join(&library_name);
        let command = runtime_link_command(&built, aot, &toolchain, &library_name, &objects);
        let exact = options
            .frontend_generation
            .filter(|_| !cfg!(test))
            .and_then(|generation| {
                runtime_cache::Exact::open(
                    (
                        generation,
                        options.record_evidence,
                        options.capture_error_trace,
                        options
                            .resource_config
                            .map(|config| (&config.path, config.interval_ms)),
                    ),
                    &built,
                    &resources,
                    &toolchain,
                )
            });
        if let Some(entry) = &exact
            && entry.restore(
                runtime_cache::RestoreInputs {
                    command: &command,
                    library: &library,
                    objects: &objects,
                    source: &source_path,
                    resources: &resources,
                    toolchain: &toolchain,
                },
                || {
                    units
                        .iter()
                        .map(|unit| unit.input_key(directory, &resources, &toolchain))
                        .collect()
                },
            )
        {
            toolchain.verify()?;
            rinzu_compiler::standard_library::verify(&resources)?;
            return Ok((library, None));
        }
        let source_timing = rinzu_compiler::timing::span("jit.runtime.generate");
        let source = crate::aot::runtime_shim_with_source_packages(
            aot,
            &imports,
            Some(plan),
            &built.source_packages,
            &bundle,
        )?;
        let source = runtime_source(aot, &source, options.record_evidence);
        let capture_error_trace = options.capture_error_trace;
        #[cfg(test)]
        let capture_error_trace =
            ERROR_TRACE_PROBE.with(std::cell::Cell::take) || capture_error_trace;
        let source = if capture_error_trace {
            format!("#define RINZU_ERROR_TRACE_DEFAULT 1\n{source}")
        } else {
            source
        };
        let startup = if let Some(config) = options.resource_config {
            format!(
                "if (rinzu_resource_report_start({}, UINT64_C({}))) return 1;",
                crate::resource_report::c_string(
                    config
                        .path
                        .to_str()
                        .ok_or("resource report path must be UTF-8")?
                ),
                config.interval_ms
            )
        } else {
            String::new()
        };
        let source = format!(
            "#define RINZU_RESOURCE_PRODUCER \"c-compiled-jit\"\n{}",
            source.replace("if (rinzu_resource_report_from_env()) return 1;", &startup)
        );
        #[cfg(test)]
        let foreign_return_probe = FOREIGN_RETURN_PROBE.with(std::cell::Cell::take);
        #[cfg(test)]
        let source = if let Some(count) = foreign_return_probe {
            foreign_return_probe::instrument(source, count)
        } else {
            source
        };
        #[cfg(test)]
        let preparation_probe = PREPARATION_PROBE.with(std::cell::Cell::take);
        drop(source_timing);
        let sources = source::Sources::split(aot, source, options.resource_config.is_some())?;
        #[cfg(test)]
        let sources = {
            let mut sources = sources;
            if preparation_probe {
                let common = sources.files.remove(source::COMMON).unwrap();
                let header = sources.files.remove(source::HEADER).unwrap();
                let (common, header) = preparation_probe::instrument_shared(common, header);
                sources.files.insert(source::COMMON.into(), common);
                sources.files.insert(source::HEADER.into(), header);
            }
            sources
        };
        for (name, contents) in &sources.files {
            checked(std::fs::write(directory.join(name), contents))?;
        }
        #[cfg(test)]
        let test_dispatcher = foreign_return_probe.is_some();
        #[cfg(not(test))]
        let test_dispatcher = false;
        let compile_timing = rinzu_compiler::timing::span("jit.runtime.clang");
        let unit_keys =
            compile_runtime(&units, directory, &resources, &toolchain, test_dispatcher)?;
        let exact = exact.and_then(|entry| entry.capture(sources.files, unit_keys));
        let runtime_cache = runtime_cache::link(
            command, &library, &objects, &built, &resources, &toolchain, exact,
        )?;
        drop(compile_timing);
        toolchain.verify()?;
        rinzu_compiler::standard_library::verify(&resources)?;
        Ok((library, runtime_cache))
    };
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let (library, runtime_cache) =
        native_preload::during_compile(&built, &bundle, &toolchain, publish, compile)?;
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    let (library, runtime_cache) = {
        publish()?;
        compile()?
    };
    Ok(PreparedRuntime {
        directory,
        library,
        sdk_cache,
        runtime_cache,
    })
}

struct RuntimeUnit {
    source: PathBuf,
    object: PathBuf,
    action: &'static str,
}

impl RuntimeUnit {
    fn all(
        aot: &BaselineAotObject,
        source: &Path,
        directory: &Path,
        resources: &Path,
    ) -> Vec<Self> {
        let runtime = resources.join("runtime");
        let linux = aot.target == Target::X86_64UnknownLinuxGnu;
        let unit = |source, name, action| Self {
            source,
            object: directory.join(format!("compiled-async.{name}.o")),
            action,
        };
        let mut units = vec![unit(
            source.to_owned(),
            "runtime",
            "compile async runtime source",
        )];
        units.push(unit(
            directory.join(source::PROGRAM),
            "program",
            "compile async program tables",
        ));
        if !linux || !aot.native_call_plans.is_empty() {
            units.extend([
                unit(
                    runtime.join("callback_entry.c"),
                    "callback-entry",
                    "compile callback entry",
                ),
                unit(
                    runtime.join("callback_template.S"),
                    "callback-template",
                    "compile callback template",
                ),
            ]);
        }
        units.push(if linux {
            unit(
                runtime.join("continuation.S"),
                "continuation",
                "compile x86_64 continuation",
            )
        } else {
            unit(
                runtime.join("owner_dispatcher.c"),
                "owner-dispatcher",
                "compile owner dispatcher",
            )
        });
        units
    }

    fn command(
        &self,
        resources: &Path,
        toolchain: &crate::native_clang::TargetClang,
    ) -> std::process::Command {
        let mut command = toolchain.command();
        command.args([
            "-c",
            "-O1",
            "-gline-tables-only",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pthread",
        ]);
        if toolchain.target == Target::X86_64UnknownLinuxGnu {
            command.arg("-fPIC");
        }
        if self
            .source
            .extension()
            .is_some_and(|extension| extension == "c")
        {
            command.arg("-std=c11");
        }
        command
            .arg("-I")
            .arg(resources.join("include"))
            .arg(&self.source)
            .arg("-o")
            .arg(&self.object);
        command
    }

    fn input_key(
        &self,
        directory: &Path,
        resources: &Path,
        toolchain: &crate::native_clang::TargetClang,
    ) -> Option<String> {
        let mut command = self.command(resources, toolchain);
        crate::compile_cache::map_generated_paths(&mut command, directory);
        crate::compile_cache::input_key(
            toolchain,
            &command,
            directory,
            &self.object.with_extension("cache.d"),
        )
    }
}

fn compile_runtime(
    units: &[RuntimeUnit],
    directory: &Path,
    resources: &Path,
    toolchain: &crate::native_clang::TargetClang,
    test_dispatcher: bool,
) -> Result<Vec<Option<String>>, String> {
    let compile = |unit: &RuntimeUnit| {
        let mut command = unit.command(resources, toolchain);
        if test_dispatcher {
            command.arg("-DRINZU_DISPATCHER_TESTING");
        }
        crate::compile_cache::compile(
            toolchain,
            command,
            directory,
            &unit.object,
            &unit.source,
            unit.action,
        )
    };
    let (runtime, helpers) = units.split_first().expect("async runtime source");
    std::thread::scope(|scope| {
        let compile_helpers = || helpers.iter().map(compile).collect::<Result<Vec<_>, _>>();
        let worker = if std::thread::available_parallelism().is_ok_and(|count| count.get() > 1) {
            std::thread::Builder::new()
                .name("rinzu-runtime-helpers".into())
                .spawn_scoped(scope, compile_helpers)
                .ok()
        } else {
            None
        };
        let runtime = compile(runtime);
        let helpers = match worker {
            Some(worker) => worker
                .join()
                .map_err(|_| "async runtime helper compilation panicked".to_owned())?,
            None => compile_helpers(),
        };
        let mut keys = vec![runtime?];
        keys.extend(helpers?);
        Ok(keys)
    })
}

fn runtime_link_command(
    built: &crate::native_package::BuiltNativePackages,
    aot: &BaselineAotObject,
    toolchain: &crate::native_clang::TargetClang,
    library_name: &str,
    objects: &[PathBuf],
) -> std::process::Command {
    let linux = aot.target == Target::X86_64UnknownLinuxGnu;
    let mut command = toolchain.shared_command(library_name);
    if !linux {
        command.arg("-pthread");
    }
    command.args(objects);
    for artifact in &built.artifacts {
        command.arg(&artifact.library);
    }
    if linux {
        crate::platform::linux::link_runtime_libraries(&mut command);
        if !aot.native_call_plans.is_empty() {
            command.args(["-lcrypto", "-ldl"]);
        }
    } else {
        command.args(["-framework", "CoreFoundation"]);
    }
    command.arg("-o").arg(built.directory.join(library_name));
    command
}

fn runtime_symbol_names(aot: &BaselineAotObject) -> Vec<String> {
    let mut symbols = Vec::new();
    for symbol in [
        "poll",
        "set_source",
        "set_source_packed",
        "observe_source",
        "has_failure",
        "function_entry",
        "publish_roots",
        "prepare_roots",
        "commit_roots",
        "publish_roots_poll",
        "commit_roots_poll",
        "commit_proven_roots_poll",
        "commit_proven_roots_poll_owner_proven",
        "commit_partial_roots_poll",
        "commit_partial_roots_poll_owner_proven",
        "commit_roots_poll_owner_proven",
        "publish_roots_poll_owner_proven",
        "project_owner_proven",
        "construct_owner_proven",
        "data_operation_owner_proven",
        "bytes_slice_payload_owner_proven",
        "indexed_option_owner_proven",
        "bytes_int_length_owner_proven",
        "string_scalar_length_owner_proven",
        "collection_length_owner_proven",
        "construct",
        "data_operation",
        "bytes_slice_payload",
        "indexed_option",
        "bytes_int_length",
        "string_scalar_length",
        "collection_length",
        "project",
        "store_field",
        "bytes",
        "bytes_pad",
        "bytes_length",
        "enum_variant",
        "value_compare",
        "checked_arithmetic",
        "scalar_arithmetic",
        "scalar_compare",
        "scalar_convert",
        "has_error",
        "catch_error",
        "string",
        "error_message",
        "error_data",
        "stdout",
        "format_string",
        "utf8_length",
        "task_control",
        "string_compare",
        "begin_cleanup",
        "end_cleanup",
        "resolve_dispatch",
        "leave_dispatch",
        "retain_return",
        "native_invoke",
        "async_spawn",
        "async_await",
        "async_call",
        "async_service",
        "task_scope",
        "task_error_origin",
        "check_local",
        "record_task_error",
        "take_task_errors",
    ] {
        symbols.push(format!("rinzu_baseline_{symbol}"));
    }
    if !aot.native_call_plans.is_empty() {
        for helper in [
            "prepare", "address", "words", "returned", "close", "settle", "read", "release",
        ] {
            symbols.push(format!("rinzu_baseline_source_native_{helper}"));
        }
    }
    if aot
        .native_call_plans
        .iter()
        .any(|plan| plan.signature.is_supplant)
    {
        for helper in ["reserve", "start", "settle"] {
            symbols.push(format!("rinzu_baseline_supplant_{helper}"));
        }
    }
    symbols
}

fn run_image(
    package_root: &Path,
    aot: BaselineAotObject,
    mut runtime: PreparedRuntime,
    managed: PreparedAsyncJit,
    symbol_names: Vec<String>,
    pending: Option<managed_cache::Pending>,
    options: RunOptions<'_, '_>,
) -> Result<CompiledAsyncRun, String> {
    let RunOptions {
        cancel_after,
        cancel_worker_after,
        shutdown_after,
        arguments,
        on_ready,
        ..
    } = options;
    let linux = aot.target == Target::X86_64UnknownLinuxGnu;
    crate::jit_progress::phase("Loading and validating JIT code");
    // The startup transaction validates the saved descriptors and native
    // images before executing the program. Open only the runtime image here.
    let mut library = {
        let _timing = rinzu_compiler::timing::span("jit.runtime.dlopen");
        CompiledRuntimeLibrary::open(&runtime.library)
            .map_err(|error| format!("load async runtime: {error:#}"))?
    };
    let symbols = symbol_names
        .into_iter()
        .map(|symbol| {
            let address = library.symbol(&symbol).map_err(|error| error.to_string())?;
            Ok((symbol, address))
        })
        .collect::<Result<_, String>>()?;
    let code = managed.publish(symbols).map_err(|error| {
        crate::cli_diagnostics::stage_failure(
            None,
            "async JIT code publication",
            None,
            &format!("{error:#}"),
        )
    })?;
    let functions = aot
        .functions
        .iter()
        .map(|function| {
            code.symbols
                .get(&function.symbol)
                .copied()
                .ok_or_else(|| format!("missing compiled function {}", function.name))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let stubs = native_signatures(&aot)
        .iter()
        .enumerate()
        .map(|(index, _)| {
            code.symbols
                .get(&format!("rinzu_source_native_{index}"))
                .copied()
                .ok_or("missing physical JIT stub".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut result = CompiledAsyncRun {
        output: String::new(),
        #[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
        source_files: aot.source_files,
        exit_code: 0,
        compile: code.compile.clone(),
        executable_mappings: code.executable_mappings,
        icache_flushes: code.icache_flushes,
        rx_transitions: code.rx_transitions,
        rwx_mappings: code.rwx_mappings,
        retired: false,
    };
    if result.rwx_mappings != 0
        || result.executable_mappings == 0
        || result.rx_transitions != result.executable_mappings
        || result.icache_flushes
            != result.executable_mappings * usize::from(aot.target.jit_flushes_instruction_cache())
        || result.compile.code_size == 0
    {
        return Err("compiled async code publication failed".into());
    }
    let anchor = runtime.directory.0.join("jit-entry");
    // Symlink targets are resolved relative to the generated runtime directory,
    // while CLI package paths can be relative to the caller's working directory.
    let package_root = checked(package_root.canonicalize())?;
    checked(std::os::unix::fs::symlink(
        &package_root,
        anchor.with_file_name("jit-entry.resources"),
    ))?;
    let anchor = CString::new(anchor.to_str().ok_or("runtime anchor is not UTF-8")?)
        .map_err(|error| error.to_string())?;
    let argc = arguments
        .len()
        .checked_add(1)
        .and_then(|count| std::ffi::c_int::try_from(count).ok())
        .ok_or("too many program arguments for the C runtime")?;
    let strings = arguments
        .iter()
        .map(|argument| CString::new(argument.as_encoded_bytes()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "program argument contains a NUL byte")?;
    let mut argv: Vec<_> = std::iter::once(&anchor)
        .chain(&strings)
        .map(|value| value.as_ptr().cast_mut())
        .chain(std::iter::once(std::ptr::null_mut()))
        .collect();
    let canonical_directory = checked(runtime.directory.0.canonicalize())?;
    let directory = CString::new(
        canonical_directory
            .to_str()
            .ok_or("runtime directory is not UTF-8")?,
    )
    .map_err(|error| error.to_string())?;
    #[derive(Default)]
    #[repr(C)]
    struct RuntimeTimings {
        native_startup: u64,
        native_dlopen: u64,
        native_images: u64,
        main: u64,
    }
    let mut runtime_timings = RuntimeTimings::default();
    type Run = unsafe extern "C" fn(
        *const usize,
        usize,
        *const usize,
        usize,
        std::ffi::c_int,
        *mut *mut c_char,
        *const c_char,
        u64,
        u64,
        u64,
        extern "C" fn() -> u64,
        *mut RuntimeTimings,
        extern "C" fn(*mut std::ffi::c_void),
        *mut std::ffi::c_void,
        *mut *mut c_char,
        *mut usize,
    ) -> i32;
    type Free = unsafe extern "C" fn(*mut c_char);
    // SAFETY: these exports have the closed private runtime API prototypes.
    let run: Run = unsafe {
        std::mem::transmute(
            library
                .symbol("rinzu_jit_run")
                .map_err(|error| error.to_string())?,
        )
    };
    // SAFETY: the image exports its matching report-buffer release function.
    let free: Free = unsafe {
        std::mem::transmute(
            library
                .symbol("rinzu_jit_report_free")
                .map_err(|error| error.to_string())?,
        )
    };
    if !linux
        || aot
            .native_call_plans
            .iter()
            .any(|plan| !plan.callbacks.is_empty())
    {
        // Closed physical callback entries can still reach their tombstone.
        library.retain_process_image();
    }
    let mut report = std::ptr::null_mut();
    let mut length = 0;
    let mut publication = Publication {
        managed: pending,
        runtime: &mut runtime,
        on_ready,
    };
    let execute_timing = rinzu_compiler::timing::span("jit.execute");
    // SAFETY: tables point at finalized code retained through all C runtime
    // settlement; strings, output slots and arrays remain valid for this call.
    let exit_code = unsafe {
        run(
            functions.as_ptr(),
            functions.len(),
            stubs.as_ptr(),
            stubs.len(),
            argc,
            argv.as_mut_ptr(),
            directory.as_ptr(),
            cancel_after.unwrap_or(0) as u64,
            shutdown_after.unwrap_or(0) as u64,
            cancel_worker_after.unwrap_or(0) as u64,
            rinzu_runtime::data::native_context_identity,
            if execute_timing.is_active() {
                &mut runtime_timings
            } else {
                std::ptr::null_mut()
            },
            publish_caches,
            (&mut publication as *mut Publication<'_, '_>).cast(),
            &mut report,
            &mut length,
        )
    };
    if execute_timing.is_active() {
        let mut images = rinzu_compiler::timing::span("jit.execute.native_dlopen");
        images.set_counter("images", runtime_timings.native_images);
        images.finish_with_duration(std::time::Duration::from_nanos(
            runtime_timings.native_dlopen,
        ));
        rinzu_compiler::timing::span("jit.execute.native_startup").finish_with_duration(
            std::time::Duration::from_nanos(runtime_timings.native_startup),
        );
        rinzu_compiler::timing::span("jit.execute.runtime_and_program").finish_with_duration(
            std::time::Duration::from_nanos(
                runtime_timings
                    .main
                    .saturating_sub(runtime_timings.native_startup),
            ),
        );
    }
    drop(execute_timing);
    if report.is_null() {
        // SAFETY: an early startup/report error returned before managed entry;
        // no callbacks, tasks or roots were published by the C image.
        unsafe { code.retire() }.map_err(|error| error.to_string())?;
        return Err("compiled async runtime returned no report".into());
    }
    // SAFETY: run returns its owned readable report buffer and exact byte count.
    let output = unsafe { std::slice::from_raw_parts(report.cast::<u8>(), length) }.to_vec();
    // SAFETY: report belongs to this image and has been copied exactly once.
    unsafe { free(report) };
    result.output = String::from_utf8(output).map_err(|error| error.to_string())?;
    result.exit_code = exit_code;
    if !result.output.contains("\"resources\":\"baseline\"") {
        return Err(format!(
            "compiled async runtime leaked resources:\n{}",
            result.output
        ));
    }
    // SAFETY: the shared runtime requires all tasks, Workers, callback gates,
    // roots and continuations to settle before returning its baseline report.
    unsafe { code.retire() }.map_err(|error| error.to_string())?;
    result.retired = true;
    Ok(result)
}

#[cfg(test)]
mod preparation_probe {
    include!("../tests/fixtures/native_supplant_sdk/preparation.rs");
}
#[cfg(test)]
mod foreign_return_probe {
    include!("../tests/fixtures/native_foreign_sdk/return_probe.rs");
}

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
mod identity_tests {
    use super::*;
    #[test]
    fn embedded_runtime_images_share_context_identities_with_the_host() {
        let root =
            std::env::temp_dir().join(format!("rinzu-context-issuers-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("identity.c");
        std::fs::write(
            &source,
            r#"
#include <stdint.h>
static uint64_t (*issuer)(void);
#define RINZU_CONTEXT_IDENTITY() issuer()
#include "rinzu/native_userdata.h"
uint64_t next_context_identity(uint64_t (*next)(void)) {
  issuer=next;return rinzu_userdata_identity();
}
"#,
        )
        .unwrap();
        let mut identities = vec![rinzu_runtime::data::native_context_identity()];
        for index in 0..2 {
            let path = root.join(format!("identity-{index}.dylib"));
            let output = std::process::Command::new("clang")
                .args(["-std=c11", "-O2", "-dynamiclib"])
                .arg(&source)
                .arg("-I")
                .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
                .arg("-o")
                .arg(&path)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let library = CompiledRuntimeLibrary::open(&path).unwrap();
            type Next = unsafe extern "C" fn(extern "C" fn() -> u64) -> u64;
            // SAFETY: this fixture export has the exact Next prototype above;
            // the permanent host issuer needs no managed context or code pin.
            let next: Next =
                unsafe { std::mem::transmute(library.symbol("next_context_identity").unwrap()) };
            // SAFETY: both functions and this library remain live during entry.
            identities.push(unsafe { next(rinzu_runtime::data::native_context_identity) });
            identities.push(rinzu_runtime::data::native_context_identity());
        }
        assert!(
            identities.windows(2).all(|pair| pair[0] < pair[1]),
            "{identities:?}"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
