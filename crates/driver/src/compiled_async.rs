//! Compiled Main continuations share the C runtime used by AOT, while managed
//! bodies and physical ABI stubs are finalized Cranelift JIT code.
use rinzu_codegen::{
    BaselineAotObject, CompiledRuntimeLibrary, compile_source_async_jit,
    emit_source_async_aot_plans,
};
use rinzu_compiler::{FrontendArtifact, Manifest, SourceNativeSignature};
use std::collections::{BTreeMap, HashMap};
use std::ffi::{CString, OsString, c_char};

pub struct CompiledAsyncRun {
    pub output: String,
    pub exit_code: i32,
    pub compile: rinzu_codegen::CompileEvidence,
    pub executable_mappings: usize,
    pub icache_flushes: usize,
    pub rx_transitions: usize,
    pub rwx_mappings: usize,
    pub retired: bool,
}
#[cfg(test)]
thread_local! {static PREPARATION_PROBE: std::cell::Cell<Option<usize>> = const {std::cell::Cell::new(None)};}
#[cfg(test)]
pub(crate) fn configure_preparation_probe(budget: usize) {
    PREPARATION_PROBE.with(|probe| probe.set(Some(budget)));
}
#[cfg(test)]
thread_local! {static FOREIGN_RETURN_PROBE: std::cell::Cell<Option<usize>> = const {std::cell::Cell::new(None)};}
#[cfg(test)]
thread_local! {static ERROR_TRACE_PROBE: std::cell::Cell<bool> = const {std::cell::Cell::new(false)};}
#[cfg(test)]
pub(crate) fn configure_error_trace_probe() {
    ERROR_TRACE_PROBE.with(|probe| probe.set(true));
}
#[cfg(test)]
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
    let signatures = native_signatures(aot);
    let mut prelude = include_str!("compiled_async.c").replace(
        "/* JIT_ENTRY_COUNTS */",
        &format!(
            "#define RINZU_JIT_FUNCTION_COUNT {}\n#define RINZU_JIT_STUB_COUNT {}\n",
            aot.functions.len(),
            signatures.len()
        ),
    );
    for function in &aot.functions {
        let mut parameters = vec!["void *context".to_owned()];
        parameters
            .extend((0..function.parameter_count).map(|index| format!("int64_t argument_{index}")));
        let types = std::iter::once("void *".to_owned())
            .chain((0..function.parameter_count).map(|_| "int64_t".to_owned()))
            .collect::<Vec<_>>()
            .join(",");
        let arguments = (0..function.parameter_count)
            .map(|index| format!(",argument_{index}"))
            .collect::<String>();
        prelude.push_str(&format!("int64_t {}({}) {{return ((int64_t (*)({types}))(uintptr_t)rinzu_jit_functions[{}])(context{arguments});}}\n",function.symbol,parameters.join(","),function.index));
    }
    for (index, _) in signatures.iter().enumerate() {
        prelude.push_str(&format!("uint64_t rinzu_source_native_{index}(uint64_t address,const uint64_t *words) {{return ((uint64_t (*)(uint64_t,const uint64_t *))(uintptr_t)rinzu_jit_stubs[{index}])(address,words);}}\n"));
    }
    // This library executes inside its host process. Only the CLI's standalone
    // startup owns SIGINT policy; embedding must not replace a host handler.
    let source = source.replace("  rinzu_standalone_interrupt_normalize();\n", "");
    let mut source = source.replace(
        "int main(int argc, char **argv) {",
        "static int rinzu_jit_main(int argc,char **argv) {",
    );
    source = source.replace("rinzu_aot_source_native_startup();", "{ uint64_t started = rinzu_jit_timing ? rinzu_jit_clock() : 0; rinzu_aot_source_native_startup(); if (rinzu_jit_timing) { uint64_t ended = rinzu_jit_clock(); rinzu_jit_timing->native_startup = started && ended >= started ? ended - started : 0; } }");
    source = source.replace(
        "char *executable_directory = rinzu_source_executable_directory();",
        "char *executable_directory = rinzu_jit_directory_copy();",
    );
    source=source.replace("const char *cancel = getenv(\"RINZU_CANCEL_AFTER_POLLS\"); if (cancel) context.cancel_after = strtoull(cancel, NULL, 10);", "context.cancel_after = rinzu_jit_cancel_after; context.shutdown_after=rinzu_jit_shutdown_after;");
    source=source.replace("const char *shutdown = getenv(\"RINZU_SHUTDOWN_AFTER_POLLS\"); if (shutdown) context.shutdown_after = strtoull(shutdown, NULL, 10);", "");
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
        prelude.push_str("static char *rinzu_jit_directory_copy(void) {size_t length=strlen(rinzu_jit_directory);if(length==SIZE_MAX)_Exit(71);char *copy=malloc(length+1);if(!copy)_Exit(71);memcpy(copy,rinzu_jit_directory,length+1);return copy;}\n");
    }
    /* Keep the executable-path utility referenced while replacing its source
    bundle caller with the runtime's explicit, validated directory. */
    if !aot.native_call_plans.is_empty() {
        source=source.replace("static int rinzu_jit_main(int argc,char **argv) {", "static int rinzu_jit_main(int argc,char **argv) {\n(void)rinzu_source_executable_directory;");
    }
    format!(
        "{prelude}\n{source}\n{}",
        include_str!("compiled_async_entry.c")
    )
}
fn checked<T>(value: std::io::Result<T>) -> Result<T, String> {
    rinzu_runtime::allocation::checked_io(value).map_err(|error| error.to_string())
}
#[cfg(test)]
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
#[cfg(test)]
pub fn run_with_resource_report(
    manifest: &Manifest,
    artifact: &FrontendArtifact,
    resource_config: Option<&rinzu_runtime::resource_report::Config>,
) -> Result<CompiledAsyncRun, String> {
    run_with_observation(manifest, artifact, resource_config, false, &[])
}
pub(crate) fn run_with_observation(
    manifest: &Manifest,
    artifact: &FrontendArtifact,
    resource_config: Option<&rinzu_runtime::resource_report::Config>,
    show_trace: bool,
    arguments: &[OsString],
) -> Result<CompiledAsyncRun, String> {
    run_configured(
        manifest,
        artifact,
        RunOptions {
            record_evidence: show_trace,
            resource_config,
            arguments,
            ..Default::default()
        },
    )
}

#[derive(Default)]
struct RunOptions<'a> {
    cancel_after: Option<usize>,
    shutdown_after: Option<usize>,
    record_evidence: bool,
    resource_config: Option<&'a rinzu_runtime::resource_report::Config>,
    arguments: &'a [OsString],
}

fn run_configured(
    manifest: &Manifest,
    artifact: &FrontendArtifact,
    options: RunOptions<'_>,
) -> Result<CompiledAsyncRun, String> {
    let _timing = rinzu_compiler::timing::span("jit.total");
    let built = crate::native_package::build_artifact(manifest, artifact)?;
    let outcome = run_built(manifest, artifact, &built, options);
    let _ = std::fs::remove_dir_all(&built.directory);
    outcome
}
fn run_built(
    manifest: &Manifest,
    artifact: &FrontendArtifact,
    built: &crate::native_package::BuiltNativePackages,
    RunOptions {
        cancel_after,
        shutdown_after,
        record_evidence,
        resource_config,
        arguments,
    }: RunOptions<'_>,
) -> Result<CompiledAsyncRun, String> {
    // The C startup transaction validates every saved descriptor, digest,
    // table and image before publishing any managed entry. Opening the source
    // facade here as well would register native classes/run initializers again
    // when startup opens its bundle copy.
    let native_bindings = artifact
        .sdk_packages
        .iter()
        .flat_map(|package| &package.native_imports)
        .map(|import| (import.export_name.clone(), import.import_key))
        .collect::<HashMap<_, _>>();
    let imports = artifact
        .sdk_packages
        .iter()
        .flat_map(|package| &package.native_imports)
        .filter(|import| import.runtime_service.is_none())
        .cloned()
        .collect::<Vec<_>>();
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
    let bundle = built.directory.join("source-packages");
    let publication = crate::input::publication_directory(manifest, artifact, &built.directory);
    crate::source_package_bundle::save_to(&built.source_packages, &[&bundle, &publication])?;
    let source_timing = rinzu_compiler::timing::span("jit.runtime.generate");
    let source = crate::aot::runtime_shim_with_source_packages(
        &aot,
        &imports,
        Some(&plan),
        &built.source_packages,
        &bundle,
    )?;
    let source = runtime_source(&aot, &source, record_evidence);
    #[cfg(test)]
    let source = if ERROR_TRACE_PROBE.with(std::cell::Cell::take) {
        format!("#define RINZU_ERROR_TRACE_DEFAULT 1\n{source}")
    } else {
        source
    };
    let startup = if let Some(config) = resource_config {
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
    let source = if let Some(budget) = PREPARATION_PROBE.with(std::cell::Cell::get) {
        preparation_probe::instrument(source, Some(budget))
    } else {
        source
    };
    drop(source_timing);
    let path = built.directory.join("compiled-async.runtime.c");
    checked(std::fs::write(&path, &source))?;
    let library = built.directory.join("compiled-async.dylib");
    let resources = rinzu_compiler::standard_library::resource_root()?;
    let runtime = resources.join("runtime");
    let toolchain = crate::native_clang::TargetClang::resolve()?;
    let compile_timing = rinzu_compiler::timing::span("jit.runtime.clang");
    let mut objects = Vec::new();
    for (source, name, action) in [
        (path, "runtime", "compile async runtime source"),
        (
            runtime.join("callback_entry.c"),
            "callback-entry",
            "compile callback entry",
        ),
        (
            runtime.join("callback_template.S"),
            "callback-template",
            "compile callback template",
        ),
        (
            runtime.join("owner_dispatcher.c"),
            "owner-dispatcher",
            "compile owner dispatcher",
        ),
    ] {
        let output = built.directory.join(format!("compiled-async.{name}.o"));
        let mut command = toolchain.command();
        command.args(["-c", "-O1", "-g", "-Wall", "-Wextra", "-Werror", "-pthread"]);
        if source.extension().is_some_and(|extension| extension == "c") {
            command.arg("-std=c11");
        }
        command
            .arg("-I")
            .arg(resources.join("include"))
            .arg(&source)
            .arg("-o")
            .arg(&output);
        #[cfg(test)]
        if foreign_return_probe.is_some() {
            command.arg("-DRINZU_DISPATCHER_TESTING");
        }
        crate::compile_cache::compile(
            &toolchain,
            command,
            &built.directory,
            &output,
            &source,
            action,
        )?;
        objects.push(output);
    }
    let mut command = toolchain.command();
    command
        .args(["-dynamiclib", "-pthread"])
        .args(&objects)
        .args(["-framework", "CoreFoundation"]);
    for artifact in &built.artifacts {
        command.arg(&artifact.library);
    }
    crate::cli_diagnostics::tool_output(
        command.arg("-o").arg(&library).output(),
        Some(&library),
        "async runtime linking",
        "clang",
    )?;
    drop(compile_timing);
    toolchain.verify()?;
    rinzu_compiler::standard_library::verify(&resources)?;
    let mut library = CompiledRuntimeLibrary::open(&library)
        .map_err(|error| format!("load async runtime: {error:#}"))?;
    let mut symbols = BTreeMap::new();
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
        "bytes_int_length_owner_proven",
        "string_scalar_length_owner_proven",
        "collection_length_owner_proven",
        "construct",
        "data_operation",
        "bytes_slice_payload",
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
        let symbol = format!("rinzu_baseline_{symbol}");
        symbols.insert(
            symbol.clone(),
            library.symbol(&symbol).map_err(|error| error.to_string())?,
        );
    }
    if !aot.native_call_plans.is_empty() {
        for helper in [
            "prepare", "address", "words", "returned", "close", "settle", "read", "release",
        ] {
            let symbol = format!("rinzu_baseline_source_native_{helper}");
            symbols.insert(
                symbol.clone(),
                library.symbol(&symbol).map_err(|error| error.to_string())?,
            );
        }
    }
    if aot
        .native_call_plans
        .iter()
        .any(|plan| plan.signature.is_supplant)
    {
        for helper in ["reserve", "start", "settle"] {
            let symbol = format!("rinzu_baseline_supplant_{helper}");
            symbols.insert(
                symbol.clone(),
                library.symbol(&symbol).map_err(|error| error.to_string())?,
            );
        }
    }
    let managed_timing = rinzu_compiler::timing::span("jit.managed");
    let code =
        compile_source_async_jit(&artifact.ir, &symbols, &native_bindings).map_err(|error| {
            crate::cli_diagnostics::stage_failure(
                None,
                "async JIT code generation",
                None,
                &format!("{error:#}"),
            )
        })?;
    drop(managed_timing);
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
    let anchor = built.directory.join("jit-entry");
    // Symlink targets are resolved relative to the generated runtime directory,
    // while CLI package paths can be relative to the caller's working directory.
    let package_root = checked(manifest.root.canonicalize())?;
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
    let canonical_directory = checked(built.directory.canonicalize())?;
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
        extern "C" fn() -> u64,
        *mut RuntimeTimings,
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
    library.retain_process_image();
    let mut report = std::ptr::null_mut();
    let mut length = 0;
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
            rinzu_runtime::data::native_context_identity,
            if execute_timing.is_active() {
                &mut runtime_timings
            } else {
                std::ptr::null_mut()
            },
            &mut report,
            &mut length,
        )
    };
    if execute_timing.is_active() {
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
        unsafe { code.retire() };
        return Err("compiled async runtime returned no report".into());
    }
    // SAFETY: run returns its owned readable report buffer and exact byte count.
    let output = unsafe { std::slice::from_raw_parts(report.cast::<u8>(), length) }.to_vec();
    // SAFETY: report belongs to this image and has been copied exactly once.
    unsafe { free(report) };
    let run = CompiledAsyncRun {
        output: String::from_utf8(output).map_err(|error| error.to_string())?,
        exit_code,
        compile: code.compile.clone(),
        executable_mappings: code.executable_mappings,
        icache_flushes: code.icache_flushes,
        rx_transitions: code.rx_transitions,
        rwx_mappings: code.rwx_mappings,
        retired: true,
    };
    if !run.output.contains("\"resources\":\"baseline\"") {
        return Err(format!(
            "compiled async runtime leaked resources:\n{}",
            run.output
        ));
    }
    // SAFETY: the shared runtime requires all tasks, Workers, callback gates,
    // roots and continuations to settle before returning its baseline report.
    unsafe { code.retire() };
    Ok(run)
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
