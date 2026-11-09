use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use rinzu_codegen::{
    AotFunctionMetadata, BaselineAotObject, emit_baseline_aot_object_with_native_bindings,
    emit_source_async_aot_object,
};
use serde_json::json;

mod equivalence;
pub(crate) mod native_quality;

pub(crate) use equivalence::{
    EquivalenceReport, verify_checked_input, verify_equivalence, verify_worker_cancel_checked,
};
#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
pub(crate) use equivalence::{
    compare_async_task_traces, compare_source_async_failures,
    compare_source_async_values_and_transport, verify_equivalence_with_options,
};
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
mod userdata_tests;

#[cfg(test)]
mod callback_tests;

#[cfg(test)]
mod error_tests;

#[cfg(test)]
mod native_record_tests;

#[cfg(test)]
mod tests;

#[cfg(all(test, unix))]
mod poll_tests;
