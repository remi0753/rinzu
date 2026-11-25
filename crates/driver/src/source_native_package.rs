use crate::native_clang::TargetClang;
use crate::native_contract::validate_package_with_cache_inputs;
use rinzu_abi::target::Target;
use rinzu_bindgen::source_package::{
    SourceInterface, generate_source_package, generate_source_package_header, sha256,
};
use rinzu_codegen::source_package::SourcePackageArtifact;
use rinzu_compiler::ast::NativeBinding;
use rinzu_compiler::{NativeCliInputs, ResolvedSdkPackage, SourceAbiType};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

mod early_cache;
mod interface;
pub(crate) use early_cache::Pending as PendingEarlySdkCache;
pub(crate) use interface::compiler_digest;

#[derive(Clone)]
pub struct BuiltSourcePackage {
    pub artifact: SourcePackageArtifact,
    pub descriptor: PathBuf,
    pub contract: PathBuf,
    pub table: PathBuf,
    pub input_snapshot: Option<crate::source_receipt::InputSnapshot>,
    pub reference_check: Option<crate::native_binding::ReferenceCheck>,
    pub owner_manifest: Option<rinzu_compiler::Manifest>,
    pub pending_early_cache: Option<PendingEarlySdkCache>,
}

impl BuiltSourcePackage {
    pub fn verify_inputs(&self) -> Result<(), String> {
        Self::verify_all(std::slice::from_ref(self), None)
    }

    pub fn verify_all(
        packages: &[Self],
        additional: Option<&crate::source_receipt::InputSnapshot>,
    ) -> Result<(), String> {
        let toolchain = packages
            .iter()
            .any(|package| package.reference_check.is_some())
            .then(TargetClang::resolve)
            .transpose()?;
        crate::source_receipt::InputSnapshot::verify_all(
            additional.into_iter().chain(
                packages
                    .iter()
                    .filter_map(|package| package.input_snapshot.as_ref()),
            ),
        )?;
        if let Some(toolchain) = &toolchain {
            crate::native_binding::ReferenceCheck::verify_all(
                packages
                    .iter()
                    .filter_map(|package| package.reference_check.as_ref()),
                toolchain,
            )?;
        }
        for package in packages {
            package.verify_generated()?;
        }
        Ok(())
    }

    fn verify_generated(&self) -> Result<(), String> {
        let descriptor = rinzu_bindgen::source_package::validate_source_descriptor(
            &self.artifact.expected_wire,
        )?;
        crate::source_receipt::verify_seal(
            self.descriptor
                .parent()
                .ok_or("descriptor directory missing")?,
            &descriptor.interface.package.build_inputs,
        )
    }
}

#[cfg(test)]
pub fn build(
    package: &ResolvedSdkPackage,
    application_root: &Path,
    inputs: &NativeCliInputs,
    directory: &Path,
) -> Result<BuiltSourcePackage, String> {
    build_with_dependencies(
        package,
        application_root,
        inputs,
        directory,
        &BTreeMap::new(),
    )
}

pub fn build_with_dependencies(
    package: &ResolvedSdkPackage,
    application_root: &Path,
    inputs: &NativeCliInputs,
    directory: &Path,
    dependency_digests: &BTreeMap<String, String>,
) -> Result<BuiltSourcePackage, String> {
    build_with_mode(
        package,
        application_root,
        inputs,
        directory,
        dependency_digests,
        "Package",
    )
}

pub fn build_with_mode(
    package: &ResolvedSdkPackage,
    application_root: &Path,
    inputs: &NativeCliInputs,
    directory: &Path,
    dependency_digests: &BTreeMap<String, String>,
    mode: &str,
) -> Result<BuiltSourcePackage, String> {
    let _timing = rinzu_compiler::timing::span("native.package").detail(&package.manifest.name);
    let no_cli_inputs = NativeCliInputs::default();
    let inputs = if crate::native_binding::cli_belongs_to(package, application_root)? {
        inputs
    } else {
        &no_cli_inputs
    };
    let source = package
        .source
        .as_ref()
        .ok_or("source SDK interface is missing")?;
    if Target::host().is_some_and(|target| !target.supports_supplant())
        && source
            .signatures
            .iter()
            .any(|signature| signature.is_supplant)
    {
        return Err(format!(
            "native supplant in package `{}` is unavailable for {}",
            package.manifest.name,
            rinzu_runtime::current_target()
        ));
    }
    let checked: BTreeMap<_, _> = source
        .source_inputs
        .iter()
        .map(|(path, bytes)| (package.manifest.root.join(path), bytes.clone()))
        .collect();
    let mut input_snapshot =
        crate::source_receipt::InputSnapshot::checked(&checked, &source.module_graph)?;
    input_snapshot.native(&package.manifest, inputs)?;
    crate::native_binding::validate_signatures(source)?;
    let toolchain = TargetClang::resolve()?;
    let linux = toolchain.target == Target::X86_64UnknownLinuxGnu;
    let has_native = source
        .signatures
        .iter()
        .any(|signature| matches!(signature.binding, NativeBinding::Symbol(_)));
    let dependency_inputs = dependency_inputs(package, dependency_digests);
    let early = has_native
        .then(|| {
            early_cache::EarlyCache::open(
                package,
                &toolchain,
                inputs,
                &checked,
                dependency_inputs.as_ref().ok()?,
                mode,
            )
        })
        .flatten();
    if let Some(built) = early.as_ref().and_then(|cache| {
        cache.restore(
            package,
            application_root,
            inputs,
            &toolchain,
            directory,
            &input_snapshot,
        )
    }) {
        return Ok(built);
    }
    // With public headers, implementation compilation and the ABI query use
    // the same captured inputs and write independent outputs. Headerless SDKs
    // still need the validated generated header before Clang can compile them.
    let validate = || {
        validate_package_with_cache_inputs(
            &package.manifest,
            source,
            directory,
            &toolchain,
            early.is_some(),
        )
    };
    let (contract, compiled) = if has_native
        && !package.manifest.native.sources.is_empty()
        && !package.manifest.native.headers.is_empty()
        && package.manifest.native.config_tools.is_empty()
        && std::thread::available_parallelism().is_ok_and(|count| count.get() > 1)
    {
        std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        std::thread::scope(|scope| {
            let worker = std::thread::Builder::new()
                .name("rinzu-sdk-sources".into())
                .spawn_scoped(scope, || {
                    crate::cli_diagnostics::capture(|| {
                        compile_sources(package, directory, &toolchain)
                    })
                })
                .ok();
            let contract = validate();
            let compiled = worker.map(|worker| worker.join());
            let contract = contract?;
            let compiled = compiled
                .map(|result| {
                    let (compiled, diagnostics) = result
                        .map_err(|_| "native SDK source compiler worker panicked".to_owned())?;
                    for block in diagnostics {
                        crate::cli_diagnostics::write_block(&block);
                    }
                    compiled
                })
                .transpose()?;
            Ok::<_, String>((contract, compiled))
        })?
    } else {
        (validate()?, None)
    };
    let mut interface = interface::build(package, source, &contract, application_root, inputs)?;
    input_snapshot.recorded(
        &package.manifest.root,
        &toolchain.sysroot,
        &interface.package.build_inputs,
    )?;
    interface.package.build_inputs.extend(dependency_inputs?);
    let header_includes = header_includes(&package.manifest)?;
    let generate = |interface: SourceInterface, bytes: &[u8]| {
        let _timing = rinzu_compiler::timing::span("native.package.generate");
        let mut generated = generate_source_package(interface, bytes)?;
        generated.header.insert_str(0, &header_includes);
        Ok::<_, String>(generated)
    };
    let mut wrappers = String::new();
    wrappers.push_str("#include \"source-package.h\"\n#include <string.h>\n#include <pthread.h>\n");
    let mut slot_index = 0;
    for signature in &source.signatures {
        let NativeBinding::Symbol(symbol) = &signature.binding else {
            continue;
        };
        let record = interface
            .functions
            .iter_mut()
            .find(|record| record.identity == signature.name)
            .unwrap();
        let wrapper = format!(
            "__rinzu_import_{}",
            &sha256(signature.name.as_bytes())[..24]
        );
        let original = &contract.prototypes[&signature.name];
        if package.manifest.native.headers.is_empty() {
            wrappers.push_str(&format!("{original};\n"));
        }
        wrappers.push_str(&format!(
            "static __typeof__(&{symbol}) volatile __rinzu_native_slot_{slot_index};\n"
        ));
        // The native and callback C types are identical across this thunk. The
        // per-Origin callback capability enters with this checked physical ABI;
        // it is never cast from a canonical record callback type to the header's
        // different function type, or replaced by a shared callback entry.
        let prototype = contract.functions[&signature.name].declaration(&wrapper);
        let arguments = signature
            .parameters
            .iter()
            .enumerate()
            .map(|(index, _)| format!("a{index}"))
            .collect::<Vec<_>>()
            .join(", ");
        let call = format!("__rinzu_native_slot_{slot_index}({arguments})");
        let statement = if signature.result == SourceAbiType::Unit {
            format!("{call}; return;")
        } else {
            format!("return {call};")
        };
        wrappers.push_str(&format!("{prototype} {{ {statement} }}\n"));
        wrappers.push_str(&format!("_Static_assert(__builtin_types_compatible_p(__typeof__(&{wrapper}), __typeof__(&{symbol})), \"native thunk function type mismatch\");\n"));
        record.symbol = Some(wrapper);
        record.c_prototype = prototype;
        slot_index += 1;
    }
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let header_timing = rinzu_compiler::timing::span("native.package.header");
    let mut preliminary = generate_source_package_header(interface.clone())?;
    preliminary.insert_str(0, &header_includes);
    drop(header_timing);
    let native_contract_path = directory.join("native-contract.h");
    std::fs::write(&native_contract_path, &contract.header).map_err(|error| error.to_string())?;
    let header_path = directory.join("source-package.h");
    let wrapper_path = directory.join("source-wrappers.c");
    let table_path = directory.join("source-table.c");
    let descriptor_path = directory.join("source-package-v2.json");
    std::fs::write(&header_path, &preliminary).map_err(|error| error.to_string())?;
    std::fs::write(&wrapper_path, &wrappers).map_err(|error| error.to_string())?;
    let mut native_digests = BTreeMap::new();
    let mut reference_inputs = None;
    let mut reference_proof = None;
    let mut source_input_keys = Vec::new();
    let mut provider_input_key = None;
    let consumption = if has_native {
        let compiled = match compiled {
            Some(objects) => objects,
            None => compile_sources(package, directory, &toolchain)?,
        };
        interface
            .package
            .build_inputs
            .extend(compiled.header_inputs);
        native_digests.extend(compiled.object_digests);
        source_input_keys = compiled.input_keys;
        compiled.consumption
    } else {
        crate::native_consumption::prepare(package)
    };
    if has_native {
        for object in &package.manifest.native.objects {
            native_digests.insert(
                format!("sdk-object:{}", object.display()),
                sha256(
                    &std::fs::read(package.manifest.root.join(object))
                        .map_err(|error| error.to_string())?,
                ),
            );
        }
        for object in &inputs.objects {
            native_digests.insert(
                format!("cli-object:{}", object.display()),
                sha256(
                    &std::fs::read(application_root.join(object))
                        .map_err(|error| error.to_string())?,
                ),
            );
        }
    }
    if has_native {
        let provider_name = toolchain.target.shared_library_name(&format!(
            "provider-{}",
            &sha256(package.manifest.name.as_bytes())[..24]
        ));
        let index = crate::native_binding::ReferenceIndex::prepare(
            package,
            application_root,
            inputs,
            directory,
            &toolchain,
            early.is_some(),
        )
        .map_err(|error| {
            format!(
                "native scope `{}` ({}): {error}",
                package.manifest.name,
                source
                    .signatures
                    .iter()
                    .map(|signature| crate::native_binding::declaration_position(
                        source,
                        &signature.name
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
        reference_inputs = Some(crate::native_binding::ReferenceCheck::new(
            package,
            application_root,
            inputs,
            directory,
            &index,
        ));
        if early.is_some() {
            reference_proof = index.proof(directory);
        }
        let (plan, binding_inputs) = index
            .bind(
                source,
                directory,
                &toolchain,
                &provider_name,
                package,
                early.as_ref().map(|_| &mut provider_input_key),
            )
            .map_err(|error| {
                format!(
                    "native scope `{}` / target {} / declarations [{}]: {error}",
                    package.manifest.name,
                    toolchain.target.triple(),
                    source
                        .signatures
                        .iter()
                        .filter_map(|signature| match &signature.binding {
                            NativeBinding::Symbol(symbol) => Some(format!(
                                "{} `{}` symbol `{symbol}`",
                                crate::native_binding::declaration_position(
                                    source,
                                    &signature.name
                                ),
                                signature.name
                            )),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
        let digest = plan.encode(&mut interface.package.build_inputs)?;
        wrappers.push_str(&crate::native_binding::initializer(&plan, &digest));
        std::fs::write(&wrapper_path, &wrappers).map_err(|error| error.to_string())?;
        native_digests.insert("generated-wrappers".to_owned(), sha256(wrappers.as_bytes()));
        interface
            .package
            .build_inputs
            .extend(binding_inputs.clone());
        native_digests.extend(binding_inputs);
    }
    if !consumption.functions.is_empty() {
        let report = serde_json::to_string(&consumption).map_err(|error| error.to_string())?;
        interface.package.build_inputs.insert(
            "native.consumption-ir-flags".into(),
            crate::native_consumption::IR_FLAGS.join(" ") + "; object from IR -O0",
        );
        native_digests.insert("consumption-report".into(), sha256(report.as_bytes()));
        interface
            .package
            .build_inputs
            .insert(rinzu_bindgen::consumption::KEY.into(), report);
    }
    let record_early = early.is_some() && consumption.analyzer.is_none();
    let pending_early = |build_inputs: &BTreeMap<String, String>, facade: Option<[String; 2]>| {
        if !record_early {
            return None;
        }
        early?.prepare_store(
            directory,
            &toolchain,
            build_inputs,
            early_cache::Evidence {
                abi: contract.abi_input_key?,
                sources: source_input_keys.into_iter().collect::<Option<_>>()?,
                provider: provider_input_key,
                facade: facade?,
                references: reference_proof?,
            },
        )
    };
    let native_bytes = serde_json::to_vec(&native_digests).map_err(|error| error.to_string())?;
    // Local providers have just been linked against the current search results,
    // including implicit dependencies. Keep that proof and the fresh ABI/object
    // checks before looking up a completed facade generation.
    let completed_cache = if package.manifest.kind == rinzu_compiler::PackageKind::Sdk
        && has_native
        && rinzu_bindgen::native_binding::NativeBindingPlan::decode(
            &interface.package.build_inputs,
        )?
        .is_some_and(|plan| plan.images.iter().any(|image| image.bundled))
    {
        completed_cache_key(
            package,
            &toolchain,
            directory,
            &interface,
            &native_bytes,
            &wrappers,
        )?
        .and_then(|key| {
            crate::source_package_cache::directory(&package.manifest.root, application_root, &key)
                .ok()
        })
    } else {
        None
    };
    if let Some((cache, key)) = &completed_cache {
        let library_name = toolchain.target.shared_library_name(&format!(
            "source-{}",
            &sha256(package.manifest.name.as_bytes())[..24]
        ));
        let mut names: BTreeSet<String> = [
            "source-package-v2.json",
            "source-package.h",
            "source-table.c",
            "source-wrappers.c",
            "native-contract.h",
            "source-input-receipt.json",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        names.insert(library_name.clone());
        if let Some(plan) = rinzu_bindgen::native_binding::NativeBindingPlan::decode(
            &interface.package.build_inputs,
        )? {
            names.extend(
                plan.images
                    .into_iter()
                    .filter(|image| image.bundled)
                    .map(|image| image.path),
            );
        }
        if let Some(files) = crate::source_package_cache::completed(cache, key, &names) {
            let wire = &files["source-package-v2.json"];
            if let Ok(descriptor) = rinzu_bindgen::source_package::validate_source_descriptor(wire)
            {
                let mut snapshot = input_snapshot.clone();
                // Check cached resolved link/header identities against fresh bytes.
                if snapshot
                    .recorded(
                        &package.manifest.root,
                        &toolchain.sysroot,
                        &descriptor.interface.package.build_inputs,
                    )
                    .is_ok()
                {
                    for (name, bytes) in &files {
                        std::fs::write(directory.join(name), bytes)
                            .map_err(|error| error.to_string())?;
                    }
                    crate::source_receipt::verify_seal(
                        directory,
                        &descriptor.interface.package.build_inputs,
                    )?;
                    if let Some(check) = &reference_inputs {
                        check.verify_with_toolchain(&toolchain)?;
                    }
                    snapshot.verify()?;
                    toolchain.verify()?;
                    crate::native_consumption::verify(&consumption)?;
                    let _hit = rinzu_compiler::timing::span("native.package.completed.hit")
                        .detail(&package.manifest.name);
                    std::fs::write(directory.join("source-cache-report.json"), serde_json::to_vec(&serde_json::json!({
                        "schema": "rinzu.source-cache-report-v4", "hit": true, "completed": true,
                        "inputFingerprint": descriptor.interface.package.build_inputs[rinzu_bindgen::compilation::DIGEST_KEY],
                        "descriptorDigest": sha256(wire)
                    })).map_err(|error| error.to_string())?).map_err(|error| error.to_string())?;
                    return Ok(BuiltSourcePackage {
                        artifact: SourcePackageArtifact {
                            library: Some(directory.join(&library_name)),
                            expected_wire: wire.clone(),
                            facade_digest: Some(sha256(&files[&library_name])),
                        },
                        descriptor: descriptor_path,
                        contract: header_path,
                        table: table_path,
                        input_snapshot: Some(snapshot),
                        reference_check: reference_inputs,
                        owner_manifest: Some(package.manifest.clone()),
                        pending_early_cache: pending_early(
                            &descriptor.interface.package.build_inputs,
                            record_early
                                .then(|| facade_input_keys(package, directory, &toolchain))
                                .flatten(),
                        ),
                    });
                }
            }
        }
    }
    let finish = |mut interface: SourceInterface, native_bytes: &[u8]| {
        crate::source_receipt::fingerprint(
            &package.manifest,
            mode,
            &source.module_graph,
            &checked,
            inputs,
            &mut interface.package.build_inputs,
        )?;
        let generated = generate(interface, native_bytes)?;
        std::fs::write(&header_path, &generated.header).map_err(|error| error.to_string())?;
        std::fs::write(&table_path, &generated.c_table).map_err(|error| error.to_string())?;
        std::fs::write(&descriptor_path, &generated.wire).map_err(|error| error.to_string())?;
        Ok::<_, String>(generated)
    };
    let mut cache_hit = false;
    let mut pending_cache = None;
    let mut facade_keys = None;
    let (generated, library) = if has_native {
        // The trace probe resolves the same link inputs using the final
        // header and checked wrappers. Only the immutable table data depends
        // on that trace, and it is generated once before the final link.
        std::fs::write(
            &table_path,
            "#include \"rinzu/runtime.h\"\n#include \"source-package.h\"\nRINZU_API const rinzu_source_package_descriptor_v2_t *rinzu_source_package_v2(void) { return NULL; }\n",
        )
        .map_err(|error| error.to_string())?;
        let library_name = toolchain.target.shared_library_name(&format!(
            "source-{}",
            &sha256(package.manifest.name.as_bytes())[..24]
        ));
        let library = directory.join(&library_name);
        let mut command =
            facade_source_command(toolchain.shared_command(&library_name), package, directory)?;
        if record_early {
            crate::compile_cache::map_generated_paths(&mut command, directory);
        }
        command.arg("-Wl,-t").arg(&wrapper_path).arg(&table_path);
        let runtime_paths: BTreeSet<_> = package
            .manifest
            .native
            .search_paths
            .iter()
            .map(|path| package.manifest.root.join(path))
            .chain(
                inputs
                    .search_paths
                    .iter()
                    .map(|path| application_root.join(path)),
            )
            .chain(
                package
                    .manifest
                    .native
                    .objects
                    .iter()
                    .filter(|path| {
                        path.extension()
                            .is_some_and(|extension| extension == "dylib")
                    })
                    .filter_map(|path| {
                        package
                            .manifest
                            .root
                            .join(path)
                            .parent()
                            .map(Path::to_path_buf)
                    }),
            )
            .chain(
                inputs
                    .objects
                    .iter()
                    .filter(|path| {
                        path.extension()
                            .is_some_and(|extension| extension == "dylib")
                    })
                    .filter_map(|path| application_root.join(path).parent().map(Path::to_path_buf)),
            )
            .collect();
        if !linux {
            for path in runtime_paths {
                command
                    .arg("-Xlinker")
                    .arg("-rpath")
                    .arg("-Xlinker")
                    .arg(path);
            }
        }
        command.arg("-o").arg(&library);
        let link_timing = rinzu_compiler::timing::span("native.package.link.probe")
            .detail(&package.manifest.name);
        let first_link = crate::cli_diagnostics::tool_output(
            command.output(),
            Some(&library),
            "source SDK linking",
            "clang",
        )?;
        drop(link_timing);
        let linked = crate::native_fingerprint::link_inputs(
            &package.manifest.root,
            &toolchain.sysroot,
            &String::from_utf8(first_link.stdout).map_err(|error| error.to_string())?,
        )?;
        if linked.is_empty() {
            return Err("target linker omitted native input trace".to_owned());
        }
        interface.package.build_inputs.extend(linked.clone());
        native_digests.extend(linked.clone());
        let generated = finish(
            interface,
            &serde_json::to_vec(&native_digests).map_err(|error| error.to_string())?,
        )?;
        let cached_files = cache_files(&generated, &wrappers, &contract.header);
        let cache = if package.manifest.kind == rinzu_compiler::PackageKind::Sdk {
            crate::source_package_cache::directory(
                &package.manifest.root,
                application_root,
                &generated.wire,
            )
            .ok()
        } else {
            None
        };
        if let Some((directory, key)) = &cache {
            cache_hit = crate::source_package_cache::restore(
                directory,
                key,
                &cached_files,
                Some(&library),
            )?;
        }
        if !cache_hit {
            if record_early {
                facade_keys = facade_input_keys(package, directory, &toolchain);
            }
            let link_timing = rinzu_compiler::timing::span("native.package.link.final")
                .detail(&package.manifest.name);
            let final_link = crate::cli_diagnostics::tool_output(
                command.output(),
                Some(&library),
                "source SDK linking",
                "clang",
            )?;
            drop(link_timing);
            let final_inputs = crate::native_fingerprint::link_inputs(
                &package.manifest.root,
                &toolchain.sysroot,
                &String::from_utf8(final_link.stdout).map_err(|error| error.to_string())?,
            )?;
            if linked != final_inputs {
                return Err("native library inputs changed during facade generation".to_owned());
            }
            if let Some((directory, key)) = &cache {
                pending_cache = Some((
                    directory.clone(),
                    key.clone(),
                    cached_files,
                    Some(library.clone()),
                ));
            }
        }
        crate::native_fingerprint::verify_link_inputs(
            &package.manifest.root,
            &toolchain.sysroot,
            &linked,
        )?;
        (generated, Some(library))
    } else {
        let generated = finish(interface, &native_bytes)?;
        let files = cache_files(&generated, &wrappers, &contract.header);
        if package.manifest.kind == rinzu_compiler::PackageKind::Sdk
            && let Ok((cache, key)) = crate::source_package_cache::directory(
                &package.manifest.root,
                application_root,
                &generated.wire,
            )
        {
            cache_hit = crate::source_package_cache::restore(&cache, &key, &files, None)?;
            if !cache_hit {
                pending_cache = Some((cache, key, files, None));
            }
        }
        (generated, None)
    };
    if let Some(check) = &reference_inputs {
        check.verify_with_toolchain(&toolchain)?;
    }
    input_snapshot.recorded(
        &package.manifest.root,
        &toolchain.sysroot,
        &generated.descriptor.interface.package.build_inputs,
    )?;
    input_snapshot.verify()?;
    toolchain.verify()?;
    crate::native_consumption::verify(&consumption)?;
    let mut sealed_files = cache_files(&generated, &wrappers, &contract.header);
    if let Some(plan) = rinzu_bindgen::native_binding::NativeBindingPlan::decode(
        &generated.descriptor.interface.package.build_inputs,
    )? {
        for image in plan.images.iter().filter(|image| image.bundled) {
            sealed_files.insert(
                image.path.clone(),
                std::fs::read(directory.join(&image.path)).map_err(|error| error.to_string())?,
            );
        }
    }
    if let Some(path) = &library {
        sealed_files.insert(
            path.file_name()
                .ok_or("facade file name missing")?
                .to_string_lossy()
                .into_owned(),
            std::fs::read(path).map_err(|error| error.to_string())?,
        );
    }
    crate::source_receipt::seal(
        directory,
        &generated.descriptor.interface.package.build_inputs,
        &sealed_files,
    )?;
    if completed_cache.is_some() || pending_cache.is_some() {
        input_snapshot.verify()?;
    }
    // These generations only accelerate future builds. Cache save/GC errors
    // leave the verified artifacts in directory available to this build.
    if let Some((cache, key)) = completed_cache {
        sealed_files.insert(
            "source-input-receipt.json".into(),
            std::fs::read(directory.join("source-input-receipt.json"))
                .map_err(|error| error.to_string())?,
        );
        let _ = crate::source_package_cache::store(&cache, &key, &sealed_files, None);
    }
    if let Some((cache, key, files, facade)) = pending_cache {
        let _ = crate::source_package_cache::store(&cache, &key, &files, facade.as_deref());
    }
    std::fs::write(directory.join("source-cache-report.json"), serde_json::to_vec(&serde_json::json!({ "schema": "rinzu.source-cache-report-v4", "hit": cache_hit, "inputFingerprint": generated.descriptor.interface.package.build_inputs[rinzu_bindgen::compilation::DIGEST_KEY], "descriptorDigest": sha256(&generated.wire) })).map_err(|error| error.to_string())?).map_err(|error| error.to_string())?;
    let facade_digest = library
        .as_ref()
        .map(|path| std::fs::read(path).map(|bytes| sha256(&bytes)))
        .transpose()
        .map_err(|error| error.to_string())?;
    let pending_early_cache = pending_early(
        &generated.descriptor.interface.package.build_inputs,
        facade_keys,
    );
    Ok(BuiltSourcePackage {
        artifact: SourcePackageArtifact {
            library,
            expected_wire: generated.wire,
            facade_digest,
        },
        descriptor: descriptor_path,
        contract: header_path,
        table: table_path,
        input_snapshot: Some(input_snapshot),
        reference_check: reference_inputs,
        owner_manifest: Some(package.manifest.clone()),
        pending_early_cache,
    })
}

fn dependency_inputs(
    package: &ResolvedSdkPackage,
    digests: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, String> {
    let mut inputs = BTreeMap::new();
    for dependency in &package.manifest.dependencies {
        let digest = digests.get(&dependency.name).ok_or_else(|| {
            format!(
                "source SDK dependency `{}` has no validated build interface",
                dependency.name
            )
        })?;
        inputs.insert(format!("dependency:{}", dependency.name), digest.clone());
    }
    if rinzu_compiler::standard_library::is_standard_manifest(&package.manifest)? {
        for name in rinzu_compiler::standard_library::dependencies(&package.manifest.name) {
            let digest = digests.get(*name).ok_or_else(|| {
                format!("standard dependency `{name}` has no validated build interface")
            })?;
            inputs.insert(format!("dependency:{name}"), digest.clone());
        }
    }
    Ok(inputs)
}

struct CompiledSources {
    header_inputs: BTreeMap<String, String>,
    object_digests: BTreeMap<String, String>,
    consumption: rinzu_bindgen::consumption::Report,
    input_keys: Vec<Option<String>>,
}

fn compile_sources(
    package: &ResolvedSdkPackage,
    directory: &Path,
    toolchain: &TargetClang,
) -> Result<CompiledSources, String> {
    let _timing = rinzu_compiler::timing::span("native.sources").detail(&package.manifest.name);
    let linux = toolchain.target == Target::X86_64UnknownLinuxGnu;
    let mut consumption = crate::native_consumption::prepare(package);
    let analyze_consumption =
        !linux && crate::native_consumption::supported(package, &mut consumption);
    let mut header_inputs = BTreeMap::new();
    let mut object_digests = BTreeMap::new();
    let mut input_keys = Vec::with_capacity(package.manifest.native.sources.len());
    for (index, source_path) in package.manifest.native.sources.iter().enumerate() {
        let output = directory.join(format!("source-{index}.o"));
        let dependencies = directory.join(format!("source-{index}.d"));
        let ir = directory.join(format!("source-{index}.ll"));
        let source = package.manifest.root.join(source_path);
        let destination = if analyze_consumption { &ir } else { &output };
        let command = source_command(package, directory, toolchain, index, analyze_consumption)?;
        let input_key = crate::compile_cache::compile(
            toolchain,
            command,
            directory,
            destination,
            &source,
            "compile source SDK object",
        )?;
        input_keys.push(input_key);
        if analyze_consumption {
            let mut backend = toolchain.command();
            backend.args(["-c", "-O0"]).arg(&ir).arg("-o").arg(&output);
            crate::cli_diagnostics::tool_output(
                backend.output(),
                Some(&ir),
                "native compilation",
                "clang",
            )?;
            crate::native_consumption::analyze(package, &mut consumption, &ir, &output)?;
        }
        crate::native_fingerprint::include_inputs(
            &package.manifest.root,
            &toolchain.sysroot,
            directory,
            &std::fs::read_to_string(dependencies).map_err(|error| error.to_string())?,
            &mut header_inputs,
        )?;
        object_digests.insert(
            format!("compiled:{index}"),
            sha256(&std::fs::read(&output).map_err(|error| error.to_string())?),
        );
    }
    Ok(CompiledSources {
        header_inputs,
        object_digests,
        consumption,
        input_keys,
    })
}

fn source_command(
    package: &ResolvedSdkPackage,
    directory: &Path,
    toolchain: &TargetClang,
    index: usize,
    analyze_consumption: bool,
) -> Result<std::process::Command, String> {
    let path = &package.manifest.native.sources[index];
    let mut command = toolchain.command();
    if analyze_consumption {
        command.args(crate::native_consumption::IR_FLAGS);
    } else {
        command.args(["-c", "-O2"]);
    }
    command
        .args([
            "-Wall",
            "-Wextra",
            "-Werror",
            "-fno-builtin",
            "-MD",
            "-MT",
            "rinzu",
            "-MF",
        ])
        .arg(directory.join(format!("source-{index}.d")));
    command.args(source_language_flags(path));
    if toolchain.target == Target::X86_64UnknownLinuxGnu {
        command.arg("-fPIC");
    }
    add_package_flags(&mut command, package, false)?;
    if package.manifest.native.headers.is_empty() {
        command
            .arg("-include")
            .arg(directory.join("native-contract.h"));
    } else {
        for header in &package.manifest.native.headers {
            command
                .arg("-include")
                .arg(package.manifest.root.join(header));
        }
    }
    let extension = if analyze_consumption { "ll" } else { "o" };
    command
        .arg(package.manifest.root.join(path))
        .arg("-o")
        .arg(directory.join(format!("source-{index}.{extension}")));
    Ok(command)
}

fn source_input_keys(
    package: &ResolvedSdkPackage,
    directory: &Path,
    toolchain: &TargetClang,
    build_inputs: &BTreeMap<String, String>,
) -> Option<Vec<String>> {
    let mut consumption = crate::native_consumption::prepare(package);
    if toolchain.target != Target::X86_64UnknownLinuxGnu
        && crate::native_consumption::supported(package, &mut consumption)
    {
        // An analyzed body also needs the analyzer's executable and IR proof.
        return None;
    }
    if !consumption.functions.is_empty()
        && build_inputs.get(rinzu_bindgen::consumption::KEY)
            != Some(&serde_json::to_string(&consumption).ok()?)
    {
        return None;
    }
    (0..package.manifest.native.sources.len())
        .map(|index| {
            let mut command = source_command(package, directory, toolchain, index, false).ok()?;
            crate::compile_cache::map_generated_paths(&mut command, directory);
            crate::compile_cache::input_key(
                toolchain,
                &command,
                directory,
                &directory.join(format!("source-{index}.d")),
            )
        })
        .collect()
}

fn facade_source_command(
    mut command: std::process::Command,
    package: &ResolvedSdkPackage,
    directory: &Path,
) -> Result<std::process::Command, String> {
    command.args([
        "-O2",
        "-Wall",
        "-Wextra",
        "-Werror",
        "-Wcast-function-type-strict",
        "-Wincompatible-function-pointer-types-strict",
        "-fno-builtin",
        "-fno-lto",
    ]);
    command
        .arg("-I")
        .arg(rinzu_compiler::standard_library::resource_root()?.join("include"));
    command.arg("-I").arg(&package.manifest.root);
    add_package_flags(&mut command, package, false)?;
    command.arg("-I").arg(directory);
    Ok(command)
}

fn facade_input_keys(
    package: &ResolvedSdkPackage,
    directory: &Path,
    toolchain: &TargetClang,
) -> Option<[String; 2]> {
    let label = &sha256(package.manifest.name.as_bytes())[..24];
    let facade = toolchain
        .target
        .shared_library_name(&format!("source-{label}"));
    let key = |name: &str| {
        let path = directory.join(name);
        let mut command =
            facade_source_command(toolchain.shared_command(&facade), package, directory).ok()?;
        // The real link passes these flags; the proof preprocesses only one
        // translation unit. Keep compile flags such as -pthread and -fPIC.
        command.arg("-Qunused-arguments");
        crate::compile_cache::map_generated_paths(&mut command, directory);
        command.arg(&path);
        crate::compile_cache::input_key(
            toolchain,
            &command,
            directory,
            &path.with_extension("cache.d"),
        )
    };
    Some([key("source-wrappers.c")?, key("source-table.c")?])
}

fn completed_cache_key(
    package: &ResolvedSdkPackage,
    toolchain: &TargetClang,
    directory: &Path,
    interface: &SourceInterface,
    native_bytes: &[u8],
    wrappers: &str,
) -> Result<Option<Vec<u8>>, String> {
    let _timing = rinzu_compiler::timing::span("native.package.completed.probe");
    let query = directory.join("source-cache-query.c");
    let deps = directory.join("source-cache-query.d");
    std::fs::write(
        &query,
        "#include \"rinzu/runtime.h\"\n#include \"source-wrappers.c\"\n",
    )
    .map_err(|error| error.to_string())?;
    let mut command = toolchain.command();
    command
        .args(["-E", "-P", "-MD", "-MT", "rinzu", "-MF"])
        .arg(&deps)
        .args(["-Werror=date-time", "-fno-builtin", "-I"])
        .arg(directory)
        .arg("-I")
        .arg(rinzu_compiler::standard_library::resource_root()?.join("include"))
        .arg("-I")
        .arg(&package.manifest.root);
    add_package_flags(&mut command, package, false)?;
    command
        .arg(format!(
            "-ffile-prefix-map={}=/rinzu-generated",
            directory.display()
        ))
        .arg(&query)
        .args(["-o", "-"]);
    let output = command.output().map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Ok(None);
    }
    let mut dependency_bytes = BTreeMap::new();
    for path in crate::native_fingerprint::dependency_paths(
        &std::fs::read_to_string(deps).map_err(|error| error.to_string())?,
    )? {
        let identity = path.strip_prefix(directory).map_or_else(
            |_| path.display().to_string(),
            |path| format!("@generated/{}", path.display()),
        );
        dependency_bytes.insert(
            identity,
            sha256(&std::fs::read(path).map_err(|error| error.to_string())?),
        );
    }
    let mut interface = interface.clone();
    interface.canonicalize();
    use std::os::unix::ffi::OsStrExt;
    let environment: BTreeMap<_, _> = std::env::vars_os()
        .filter(|(key, _)| !key.as_bytes().starts_with(b"RINZU_"))
        .map(|(key, value)| (key.as_bytes().to_vec(), value.as_bytes().to_vec()))
        .collect();
    let environment: Vec<_> = environment.into_iter().collect();
    // Native/ABI flags, compiler and tools, owner/dependency source receipts,
    // compiled objects and current provider/link selection are in interface.
    serde_json::to_vec(&(
        "rinzu.completed-sdk-v1",
        interface,
        sha256(native_bytes),
        sha256(wrappers.as_bytes()),
        sha256(&output.stdout),
        dependency_bytes,
        environment,
        std::env::current_dir().map_err(|error| error.to_string())?,
    ))
    .map(Some)
    .map_err(|error| error.to_string())
}

pub(super) fn header_includes(manifest: &rinzu_compiler::Manifest) -> Result<String, String> {
    let mut includes = String::new();
    for header in &manifest.native.headers {
        let path = header.to_str().ok_or("native header path must be UTF-8")?;
        if path.contains(['"', '\\', '\n', '\r']) {
            return Err("native header path cannot be encoded in an include".into());
        }
        includes.push_str(&format!("#include \"{path}\"\n"));
    }
    Ok(includes)
}

fn cache_files(
    generated: &rinzu_bindgen::source_package::GeneratedSourcePackage,
    wrappers: &str,
    contract: &str,
) -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([
        ("source-package-v2.json".to_owned(), generated.wire.clone()),
        (
            "source-package.h".to_owned(),
            generated.header.as_bytes().to_vec(),
        ),
        (
            "source-table.c".to_owned(),
            generated.c_table.as_bytes().to_vec(),
        ),
        ("source-wrappers.c".to_owned(), wrappers.as_bytes().to_vec()),
        ("native-contract.h".to_owned(), contract.as_bytes().to_vec()),
    ])
}

#[cfg(test)]
fn run(mut command: std::process::Command, action: &str) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|error| format!("{action}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{action} failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}
/// Selected source standards are explicit, and part of the native input identity.
fn source_language_flags(path: &Path) -> Vec<&'static str> {
    match path.extension().and_then(|value| value.to_str()) {
        Some("cpp") => vec!["-std=c++17"],
        Some("mm") => vec!["-std=c++17", "-fobjc-arc"],
        Some("m") => vec!["-std=c11", "-fobjc-arc"],
        _ => vec!["-std=c11"],
    }
}

pub(crate) fn add_package_flags(
    command: &mut std::process::Command,
    package: &ResolvedSdkPackage,
    linking: bool,
) -> Result<(), String> {
    command
        .arg("-I")
        .arg(rinzu_compiler::standard_library::resource_root()?.join("include"));
    for path in &package.manifest.native.include_dirs {
        command.arg("-I").arg(package.manifest.root.join(path));
    }
    if linking {
        for path in &package.manifest.native.search_paths {
            command.arg("-L").arg(package.manifest.root.join(path));
        }
        for library in &package.manifest.native.libraries {
            command.arg(format!("-l{library}"));
        }
        for framework in &package.manifest.native.frameworks {
            command.arg("-framework").arg(framework);
        }
    }
    for tool in &package.manifest.native.config_tools {
        let flags = crate::native_config::resolve(tool)?.flags;
        if linking {
            command.args(flags);
        } else {
            let mut pairs = flags.into_iter();
            while let Some(flag) = pairs.next() {
                let value = pairs.next().ok_or("unpaired config flag")?;
                if matches!(flag.as_str(), "-I" | "-D" | "-F") {
                    command.arg(flag).arg(value);
                }
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests;
