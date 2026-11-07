use crate::native_clang::TargetClang;
use crate::native_contract::{NativeContract, validate_package_with_toolchain};
use rinzu_bindgen::source_package::{
    BackendRecord, CallbackRecord, CallbackSiteRecord, CodeRecord, FallbackRecord, FieldLayout,
    FunctionKind, FunctionRecord, PackageIdentity, SourceInterface, SourceTemplateRecord,
    SourceTypeRecord, TypeRecord, generate_source_package, sha256,
};
use rinzu_codegen::source_package::SourcePackageArtifact;
use rinzu_compiler::ast::NativeBinding;
use rinzu_compiler::{NativeCliInputs, ResolvedSdkPackage, SourceAbiType, SourceSdkInterface};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct BuiltSourcePackage {
    pub artifact: SourcePackageArtifact,
    pub descriptor: PathBuf,
    pub contract: PathBuf,
    pub table: PathBuf,
    pub input_snapshot: Option<crate::source_receipt::InputSnapshot>,
    pub reference_check: Option<crate::native_binding::ReferenceCheck>,
    pub owner_manifest: Option<rinzu_compiler::Manifest>,
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
    let contract =
        validate_package_with_toolchain(&package.manifest, source, directory, &toolchain)?;
    let mut interface = interface(package, source, &contract, application_root, inputs)?;
    input_snapshot.recorded(
        &package.manifest.root,
        &toolchain.sysroot,
        &interface.package.build_inputs,
    )?;
    for dependency in &package.manifest.dependencies {
        let digest = dependency_digests.get(&dependency.name).ok_or_else(|| {
            format!(
                "source SDK dependency `{}` has no validated build interface",
                dependency.name
            )
        })?;
        interface
            .package
            .build_inputs
            .insert(format!("dependency:{}", dependency.name), digest.clone());
    }
    if rinzu_compiler::standard_library::is_standard_manifest(&package.manifest)? {
        for name in rinzu_compiler::standard_library::dependencies(&package.manifest.name) {
            let digest = dependency_digests.get(*name).ok_or_else(|| {
                format!("standard dependency `{name}` has no validated build interface")
            })?;
            interface
                .package
                .build_inputs
                .insert(format!("dependency:{name}"), digest.clone());
        }
    }
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
    let preliminary = generate(interface.clone(), &[])?;
    let native_contract_path = directory.join("native-contract.h");
    std::fs::write(&native_contract_path, &contract.header).map_err(|error| error.to_string())?;
    let header_path = directory.join("source-package.h");
    let wrapper_path = directory.join("source-wrappers.c");
    let table_path = directory.join("source-table.c");
    let descriptor_path = directory.join("source-package-v2.json");
    std::fs::write(&header_path, &preliminary.header).map_err(|error| error.to_string())?;
    std::fs::write(&wrapper_path, &wrappers).map_err(|error| error.to_string())?;
    let has_native = source
        .signatures
        .iter()
        .any(|signature| matches!(signature.binding, NativeBinding::Symbol(_)));
    let mut native_digests = BTreeMap::new();
    let mut reference_inputs = None;
    let mut consumption = crate::native_consumption::prepare(package);
    let analyze_consumption = crate::native_consumption::supported(package, &mut consumption);
    if has_native {
        for (index, source_path) in package.manifest.native.sources.iter().enumerate() {
            let output = directory.join(format!("source-{index}.o"));
            let mut command = toolchain.command();
            let dependencies = directory.join(format!("source-{index}.d"));
            let ir = directory.join(format!("source-{index}.ll"));
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
                .arg(&dependencies);
            command.args(source_language_flags(source_path));
            add_package_flags(&mut command, package, false)?;
            if package.manifest.native.headers.is_empty() {
                command.arg("-include").arg(&native_contract_path);
            } else {
                for header in &package.manifest.native.headers {
                    command
                        .arg("-include")
                        .arg(package.manifest.root.join(header));
                }
            }
            command
                .arg(package.manifest.root.join(source_path))
                .arg("-o")
                .arg(if analyze_consumption { &ir } else { &output });
            crate::compile_cache::compile(
                &toolchain,
                command,
                directory,
                if analyze_consumption { &ir } else { &output },
                "compile source SDK object",
            )?;
            if analyze_consumption {
                let mut backend = toolchain.command();
                backend.args(["-c", "-O0"]).arg(&ir).arg("-o").arg(&output);
                run(backend, "compile analyzed SDK IR")?;
                crate::native_consumption::analyze(package, &mut consumption, &ir, &output)?;
            }
            crate::native_fingerprint::include_inputs(
                &package.manifest.root,
                &toolchain.sysroot,
                directory,
                &std::fs::read_to_string(dependencies).map_err(|error| error.to_string())?,
                &mut interface.package.build_inputs,
            )?;
            native_digests.insert(
                format!("compiled:{index}"),
                sha256(&std::fs::read(&output).map_err(|error| error.to_string())?),
            );
        }
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
        let provider_name = format!(
            "provider-{}.dylib",
            &sha256(package.manifest.name.as_bytes())[..24]
        );
        let index = crate::native_binding::ReferenceIndex::prepare(
            package,
            application_root,
            inputs,
            directory,
            &toolchain,
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
        let (plan, binding_inputs) = index
            .bind(source, directory, &toolchain, &provider_name, package)
            .map_err(|error| {
                format!(
                    "native scope `{}` / target aarch64-apple-darwin / declarations [{}]: {error}",
                    package.manifest.name,
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
        let library_name = format!(
            "source-{}.dylib",
            &sha256(package.manifest.name.as_bytes())[..24]
        );
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
                    });
                }
            }
        }
    }
    crate::source_receipt::fingerprint(
        &package.manifest,
        mode,
        &source.module_graph,
        &checked,
        inputs,
        &mut interface.package.build_inputs,
    )?;
    let mut generated = generate(interface.clone(), &native_bytes)?;
    std::fs::write(&header_path, &generated.header).map_err(|error| error.to_string())?;
    std::fs::write(&table_path, &generated.c_table).map_err(|error| error.to_string())?;
    std::fs::write(&descriptor_path, &generated.wire).map_err(|error| error.to_string())?;
    let mut cache_hit = false;
    let mut pending_cache = None;
    let library = if has_native {
        let library_name = format!(
            "source-{}.dylib",
            &sha256(package.manifest.name.as_bytes())[..24]
        );
        let library = directory.join(&library_name);
        let mut command = toolchain.command();
        command.args([
            "-dynamiclib",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-Wcast-function-type-strict",
            "-Wincompatible-function-pointer-types-strict",
            "-Wl,-t",
            "-fno-builtin",
            "-fno-lto",
        ]);
        command
            .arg("-install_name")
            .arg(format!("@rpath/{library_name}"));
        command
            .arg("-I")
            .arg(rinzu_compiler::standard_library::resource_root()?.join("include"));
        command.arg("-I").arg(&package.manifest.root);
        add_package_flags(&mut command, package, false)?;
        command
            .arg("-I")
            .arg(directory)
            .arg(&wrapper_path)
            .arg(&table_path);
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
        for path in runtime_paths {
            command
                .arg("-Xlinker")
                .arg("-rpath")
                .arg("-Xlinker")
                .arg(path);
        }
        command.arg("-o").arg(&library);
        let first_link = command.output().map_err(|error| error.to_string())?;
        if !first_link.status.success() {
            return Err(format!(
                "link source SDK facade failed:\n{}",
                String::from_utf8_lossy(&first_link.stderr)
            ));
        }
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
        crate::source_receipt::fingerprint(
            &package.manifest,
            mode,
            &source.module_graph,
            &checked,
            inputs,
            &mut interface.package.build_inputs,
        )?;
        generated = generate(
            interface,
            &serde_json::to_vec(&native_digests).map_err(|error| error.to_string())?,
        )?;
        std::fs::write(&header_path, &generated.header).map_err(|error| error.to_string())?;
        std::fs::write(&table_path, &generated.c_table).map_err(|error| error.to_string())?;
        std::fs::write(&descriptor_path, &generated.wire).map_err(|error| error.to_string())?;
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
            let final_link = command.output().map_err(|error| error.to_string())?;
            if !final_link.status.success() {
                return Err(format!(
                    "link final source SDK facade failed:\n{}",
                    String::from_utf8_lossy(&final_link.stderr)
                ));
            }
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
        Some(library)
    } else {
        None
    };
    if !has_native {
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
    }
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
    if let Some((cache, key)) = completed_cache {
        sealed_files.insert(
            "source-input-receipt.json".into(),
            std::fs::read(directory.join("source-input-receipt.json"))
                .map_err(|error| error.to_string())?,
        );
        input_snapshot.verify()?;
        crate::source_package_cache::store(&cache, &key, &sealed_files, None)?;
    }
    if let Some((cache, key, files, facade)) = pending_cache {
        input_snapshot.verify()?;
        crate::source_package_cache::store(&cache, &key, &files, facade.as_deref())?;
    }
    std::fs::write(directory.join("source-cache-report.json"), serde_json::to_vec(&serde_json::json!({ "schema": "rinzu.source-cache-report-v4", "hit": cache_hit, "inputFingerprint": generated.descriptor.interface.package.build_inputs[rinzu_bindgen::compilation::DIGEST_KEY], "descriptorDigest": sha256(&generated.wire) })).map_err(|error| error.to_string())?).map_err(|error| error.to_string())?;
    let facade_digest = library
        .as_ref()
        .map(|path| std::fs::read(path).map(|bytes| sha256(&bytes)))
        .transpose()
        .map_err(|error| error.to_string())?;
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
    })
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
fn abi_type(ty: &SourceAbiType) -> String {
    ty.canonical_name()
}

fn source_type(ty: &rinzu_compiler::Type) -> String {
    use rinzu_compiler::Type;
    if let Some(scalar) = ty.scalar() {
        return scalar.name().to_owned();
    }
    match ty {
        Type::FunctionReference(reference) if reference.error_bound.is_some() => ty.to_string(),
        Type::FunctionReference(reference) => format!(
            "{}({})->{}",
            if reference.supplant { "supplant" } else { "fn" },
            reference
                .parameters
                .iter()
                .map(source_type)
                .collect::<Vec<_>>()
                .join(","),
            source_type(&reference.result)
        ),
        Type::Future(result) => format!("Future<{}>", source_type(result)),
        Type::List(element) => format!("List<{}>", source_type(element)),
        Type::Map(key, value) => format!("Map<{},{}>", source_type(key), source_type(value)),
        _ => ty.to_string(),
    }
}

fn native_callback_sites(
    source: &SourceSdkInterface,
    function: &str,
) -> Result<Vec<CallbackSiteRecord>, String> {
    let mut sites = Vec::new();
    let mut append = |signature: &rinzu_compiler::SourceNativeSignature,
                      callbacks: &[rinzu_compiler::CallbackArgumentPlan]|
     -> Result<(), String> {
        for callback in callbacks {
            let parameter = signature
                .parameters
                .get(callback.index)
                .filter(|ty| matches!(ty, SourceAbiType::Callback { .. }))
                .ok_or_else(|| {
                    format!(
                        "callback site has no checked parameter: {}:{}",
                        signature.name, callback.index
                    )
                })?;
            sites.push(CallbackSiteRecord {
                native_function: signature.name.clone(),
                parameter: callback.index,
                source_type: abi_type(parameter),
                capture: callback.capture.as_ref().map(ToString::to_string),
                targets: callback
                    .targets
                    .iter()
                    .map(|target| CodeRecord {
                        declaration: target.clone(),
                        version: 1,
                        fallback: source
                            .lowered_functions
                            .iter()
                            .find(|body| body.name == *target)
                            .and_then(|body| body.callback_fallback.as_ref())
                            .map(fallback_record),
                    })
                    .collect(),
            });
        }
        Ok(())
    };
    for operation in source
        .lowered_functions
        .iter()
        .filter(|body| body.name == function)
        .flat_map(|body| &body.blocks)
        .flat_map(|block| &block.operations)
    {
        match &operation.kind {
            rinzu_compiler::OperationKind::PrepareNativeCall {
                signature,
                callbacks,
                ..
            } => append(signature, callbacks)?,
            rinzu_compiler::OperationKind::ReserveSupplant {
                targets, callbacks, ..
            } if !callbacks.is_empty() => {
                for target in targets {
                    let signature = source
                        .lowered_native_signatures
                        .iter()
                        .find(|signature| signature.name == *target && signature.is_supplant)
                        .ok_or_else(|| {
                            format!(
                                "supplant callback site has no checked native declaration: {target}"
                            )
                        })?;
                    append(signature, callbacks)?;
                }
            }
            _ => {}
        }
    }
    Ok(sites)
}

fn interface(
    package: &ResolvedSdkPackage,
    source: &SourceSdkInterface,
    contract: &NativeContract,
    application_root: &Path,
    inputs: &NativeCliInputs,
) -> Result<SourceInterface, String> {
    let _timing = rinzu_compiler::timing::span("native.interface").detail(&package.manifest.name);
    let mut types = BTreeMap::new();
    fn collect_type(
        ty: &SourceAbiType,
        contract: &NativeContract,
        schemas: &BTreeMap<String, Vec<(String, rinzu_compiler::AbiScalar)>>,
        types: &mut BTreeMap<String, TypeRecord>,
    ) {
        let identity = abi_type(ty);
        if types.contains_key(&identity) {
            return;
        }
        let (c_type, size, alignment, bytes_length, fields) = match ty {
            SourceAbiType::Unit => ("void".to_owned(), 0, 1, None, Vec::new()),
            SourceAbiType::Scalar(scalar) => (
                scalar.c_type().to_owned(),
                scalar.size(),
                scalar.size(),
                None,
                Vec::new(),
            ),
            SourceAbiType::Record(name) => {
                let layout = &contract.records[name];
                let fields = schemas[name]
                    .iter()
                    .zip(&layout.offsets)
                    .map(|((name, scalar), offset)| FieldLayout {
                        name: name.clone(),
                        c_type: scalar.c_type().to_owned(),
                        offset: offset.to_string(),
                    })
                    .collect();
                (
                    layout.c_type.clone(),
                    layout.size,
                    layout.alignment,
                    None,
                    fields,
                )
            }
            SourceAbiType::String => ("const char *".to_owned(), 8, 8, None, Vec::new()),
            SourceAbiType::Bytes(length) => (
                "const uint8_t *".to_owned(),
                8,
                8,
                Some(length.to_string()),
                Vec::new(),
            ),
            SourceAbiType::ByteArray => ("const uint8_t *".to_owned(), 8, 8, None, Vec::new()),
            SourceAbiType::ByteArrayResult => (
                "const struct __rinzu_byte_array_result *".to_owned(),
                8,
                8,
                None,
                Vec::new(),
            ),
            SourceAbiType::Callback { parameters, result } => {
                for ty in parameters {
                    collect_type(ty, contract, schemas, types);
                }
                collect_type(result, contract, schemas, types);
                ("void *".to_owned(), 8, 8, None, Vec::new())
            }
            SourceAbiType::Opaque(_) | SourceAbiType::UserData(_) => {
                ("void *".to_owned(), 8, 8, None, Vec::new())
            }
        };
        types.insert(
            identity.clone(),
            TypeRecord {
                identity: identity.clone(),
                canonical: identity,
                c_type,
                size: size.to_string(),
                alignment: alignment.to_string(),
                bytes_length,
                fields,
            },
        );
    }
    for (name, schema) in &source.types {
        let ty = match schema.kind.as_str() {
            "native-record" => SourceAbiType::Record(name.clone()),
            "opaque" => SourceAbiType::Opaque(name.clone()),
            _ => continue,
        };
        collect_type(&ty, contract, &source.native_records, &mut types);
    }
    let mut functions = Vec::new();
    for signature in &source.signatures {
        for ty in signature
            .parameters
            .iter()
            .chain(std::iter::once(&signature.result))
        {
            collect_type(ty, contract, &source.native_records, &mut types);
        }
        let source_type = signature.canonical_type();
        let (symbol, service) = match &signature.binding {
            NativeBinding::Symbol(symbol) => (Some(symbol.clone()), None),
            NativeBinding::Runtime(function) => (None, Some(function.service_id().to_owned())),
        };
        let callbacks = signature
            .parameters
            .iter()
            .filter_map(|ty| {
                if matches!(ty, SourceAbiType::Callback { .. }) {
                    Some(CallbackRecord {
                        source_type: abi_type(ty),
                        foreign_to_main: false,
                        lifetime: "current-native-call".to_owned(),
                        gate: "admission-close-settle-v1".to_owned(),
                        origin: "actual-caller-domain".to_owned(),
                        parent_scope: "actual-caller-scope".to_owned(),
                        target: "aarch64-apple-darwin".to_owned(),
                        code_version: 1,
                        fallback: None,
                    })
                } else {
                    None
                }
            })
            .collect();
        functions.push(FunctionRecord {
            identity: signature.name.clone(),
            error_contract: None,
            source_type,
            kind: if signature.is_async {
                FunctionKind::Async
            } else if signature.is_supplant {
                FunctionKind::Supplant
            } else {
                FunctionKind::Native
            },
            effects: if signature.is_supplant {
                vec!["requires-owner".to_owned()]
            } else {
                Vec::new()
            },
            parameter_requirements: signature.parameters.iter().map(abi_type).collect(),
            symbol,
            service,
            c_prototype: contract
                .prototypes
                .get(&signature.name)
                .cloned()
                .unwrap_or_default(),
            conversion_plan: Vec::new(),
            callbacks,
            callback_sites: Vec::new(),
            code: None,
        });
    }
    for function in &source.functions {
        let facts = &function.facts;
        let effects = [
            ("may-fail", facts.may_fail),
            ("may-await", facts.may_await),
            ("may-spawn", facts.may_spawn),
            ("worker-safe", facts.worker_safe),
            ("requires-owner", facts.requires_owner),
            ("cleanup-safe", facts.cleanup_safe),
        ]
        .into_iter()
        .filter(|(_, present)| *present)
        .map(|(name, _)| name.to_owned())
        .collect();
        functions.push(FunctionRecord {
            identity: function.declaration.name.clone(),
            error_contract: Some(rinzu_bindgen::source_package::ErrorContractRecord {
                declared: function.declaration.error_contract.is_some(),
                types: function.error_types.clone(),
            }),
            source_type: format!(
                "{}({})->{}",
                if facts.is_async { "async" } else { "fn" },
                function
                    .parameter_types
                    .iter()
                    .map(source_type)
                    .collect::<Vec<_>>()
                    .join(","),
                source_type(&function.result_type)
            ),
            kind: if facts.is_async {
                FunctionKind::Async
            } else {
                FunctionKind::Sync
            },
            effects,
            parameter_requirements: function.parameter_types.iter().map(source_type).collect(),
            symbol: None,
            service: None,
            c_prototype: String::new(),
            conversion_plan: Vec::new(),
            callbacks: Vec::new(),
            callback_sites: native_callback_sites(source, &function.declaration.name)?,
            code: Some(CodeRecord {
                declaration: function.declaration.name.clone(),
                version: 1,
                fallback: function.callback_fallback.as_ref().map(fallback_record),
            }),
        });
    }
    let mut build_inputs =
        crate::native_fingerprint::normalize(&package.manifest.root, &contract.fingerprint)?;
    crate::native_fingerprint::source_inputs(&package.manifest, source, &mut build_inputs)?;
    static COMPILER_DIGEST: std::sync::OnceLock<Result<String, String>> =
        std::sync::OnceLock::new();
    let compiler_digest = COMPILER_DIGEST
        .get_or_init(|| {
            std::env::current_exe()
                .map_err(|error| error.to_string())
                .and_then(|path| {
                    std::fs::read(path)
                        .map(|bytes| sha256(&bytes))
                        .map_err(|error| error.to_string())
                })
        })
        .clone()?;
    build_inputs.insert("rinzu.compiler-binary".to_owned(), compiler_digest);
    build_inputs.extend(crate::toolchain_bundle::build_inputs()?);
    build_inputs.insert(
        "rinzu.native-data-contract".into(),
        rinzu_abi::NATIVE_DATA_CONTRACT_VERSION.to_string(),
    );
    if rinzu_compiler::standard_library::is_standard_manifest(&package.manifest)? {
        build_inputs.insert(
            "rinzu.standard-package".into(),
            package.manifest.name.clone(),
        );
        build_inputs.insert("rinzu.standard-package-version".into(), "0.1.0".into());
    }

    let abi_header =
        rinzu_compiler::standard_library::resource_root()?.join("include/rinzu/runtime.h");
    build_inputs.insert(
        "rinzu.abi-header".to_owned(),
        sha256(&std::fs::read(abi_header).map_err(|error| error.to_string())?),
    );
    let completion_header = rinzu_compiler::standard_library::resource_root()?
        .join("include/rinzu/native_completion.h");
    build_inputs.insert(
        "rinzu.completion-header".to_owned(),
        sha256(&std::fs::read(completion_header).map_err(|error| error.to_string())?),
    );

    for path in package
        .manifest
        .native
        .sources
        .iter()
        .chain(&package.manifest.native.headers)
        .chain(&package.manifest.native.objects)
    {
        build_inputs.insert(
            format!("sdk:{}", path.display()),
            sha256(
                &std::fs::read(package.manifest.root.join(path))
                    .map_err(|error| error.to_string())?,
            ),
        );
    }
    let source_options: BTreeMap<_, _> = package
        .manifest
        .native
        .sources
        .iter()
        .map(|path| {
            (
                path.to_string_lossy().into_owned(),
                source_language_flags(path),
            )
        })
        .collect();
    build_inputs.insert(
        "sdk.source-options".to_owned(),
        serde_json::to_string(&source_options).map_err(|error| error.to_string())?,
    );
    for path in &inputs.objects {
        build_inputs.insert(
            format!("cli:{}", path.display()),
            sha256(&std::fs::read(application_root.join(path)).map_err(|error| error.to_string())?),
        );
    }
    build_inputs.insert(
        "cli.search-paths".to_owned(),
        serde_json::to_string(&inputs.search_paths).map_err(|error| error.to_string())?,
    );
    build_inputs.insert(
        "cli.libraries".to_owned(),
        serde_json::to_string(&inputs.libraries).map_err(|error| error.to_string())?,
    );
    build_inputs.insert(
        "sdk.libraries".to_owned(),
        serde_json::to_string(&package.manifest.native.libraries)
            .map_err(|error| error.to_string())?,
    );
    build_inputs.insert(
        "sdk.frameworks".to_owned(),
        serde_json::to_string(&package.manifest.native.frameworks)
            .map_err(|error| error.to_string())?,
    );
    let exports: Vec<_> = source
        .modules
        .iter()
        .flat_map(|(module, exports)| exports.keys().map(move |name| format!("{module}.{name}")))
        .collect();
    let public_targets: BTreeSet<_> = source
        .modules
        .values()
        .flat_map(|exports| exports.values())
        .collect();
    let constants = source
        .constants
        .iter()
        .filter(|(name, _)| public_targets.contains(name))
        .map(|(name, value)| (name.clone(), value.value.to_string()))
        .collect();
    let provided_header_digest = if package.manifest.native.headers.is_empty() {
        None
    } else {
        let headers: BTreeMap<_, _> = package
            .manifest
            .native
            .headers
            .iter()
            .map(|path| {
                std::fs::read(package.manifest.root.join(path))
                    .map(|bytes| (path.to_string_lossy().into_owned(), sha256(&bytes)))
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<_, _>>()?;
        Some(sha256(
            &serde_json::to_vec(&headers).map_err(|error| error.to_string())?,
        ))
    };
    Ok(SourceInterface {
        package: PackageIdentity {
            package: package.manifest.name.clone(),
            modules: source.modules.keys().cloned().collect(),
            exports,
            language: format!("rinzu-{}", package.manifest.language),
            target: "aarch64-apple-darwin".to_owned(),
            data_layout: "e-m:o-i64:64-i128:128-n32:64-S128".to_owned(),
            toolchain: format!("rinzu-{}", env!("CARGO_PKG_VERSION")),
            native_compiler: contract.fingerprint["clang.version"].clone(),
            build_inputs,
        },
        types: types.into_values().collect(),
        functions,
        backend: BackendRecord {
            native_data_contract: rinzu_abi::NATIVE_DATA_CONTRACT_VERSION,
            callback_rule: 1,
            template_layout: rinzu_abi::TEMPLATE_LAYOUT_VERSION,
            service_registry: rinzu_abi::CLOSED_SERVICE_REGISTRY_VERSION,
            page_size: "16384".to_owned(),
            entry_stride: "32".to_owned(),
            control_stride: "32".to_owned(),
            bridge_abi: "arm64-x16-v1".to_owned(),
            owner_dispatcher: "macos-runloop-v1".to_owned(),
        },
        constants,
        constant_types: source
            .constants
            .iter()
            .filter(|(name, _)| public_targets.contains(name))
            .map(|(name, value)| (name.clone(), value.scalar.name().to_owned()))
            .collect(),
        public_bindings: source
            .modules
            .iter()
            .flat_map(|(module, exports)| {
                exports.iter().map(move |(name, target)| {
                    let identity = match source.type_aliases.get(target) {
                        Some(rinzu_compiler::Type::Named(name))
                            if !name.starts_with("$rinzu_record_")
                                && !name.starts_with("$rinzu_closure_") =>
                        {
                            name.clone()
                        }
                        Some(rinzu_compiler::Type::Native { name, .. }) => name.clone(),
                        _ => target.clone(),
                    };
                    (format!("{module}.{name}"), identity)
                })
            })
            .collect(),
        source_templates: source
            .templates
            .iter()
            .map(|(name, schema)| {
                (
                    name.clone(),
                    SourceTemplateRecord {
                        interface_constraints: schema.interface_constraints.clone(),
                        interfaces: schema.interfaces.clone(),
                        error_contract: schema.error_contract.clone(),
                        methods: schema.methods.clone(),
                        owner_type_parameters: schema.owner_type_parameters.clone(),
                        private_fields: schema.private_fields.clone(),
                        kind: schema.kind.clone(),
                        type_parameters: schema.type_parameters.clone(),
                        signature: schema.signature.clone(),
                        const_fields: schema.const_fields.clone(),
                        fields: schema.fields.clone(),
                        variants: schema.variants.clone(),
                    },
                )
            })
            .collect(),
        source_values: source
            .values
            .iter()
            .map(|(name, ty)| (name.clone(), source_type(ty)))
            .collect(),
        source_types: source
            .types
            .iter()
            .map(|(name, schema)| {
                (
                    name.clone(),
                    SourceTypeRecord {
                        private_fields: schema.private_fields.clone(),
                        kind: schema.kind.clone(),
                        const_fields: schema.const_fields.clone(),
                        fields: schema
                            .fields
                            .iter()
                            .map(|(name, ty)| (name.clone(), ty.to_string()))
                            .collect(),
                        variants: schema
                            .variants
                            .iter()
                            .map(|(name, fields)| {
                                (
                                    name.clone(),
                                    fields
                                        .iter()
                                        .map(|(name, ty)| (name.clone(), ty.to_string()))
                                        .collect(),
                                )
                            })
                            .collect(),
                    },
                )
            })
            .chain(
                source
                    .signatures
                    .iter()
                    .flat_map(|signature| {
                        signature
                            .parameters
                            .iter()
                            .chain(std::iter::once(&signature.result))
                    })
                    .filter_map(|ty| {
                        if let SourceAbiType::Opaque(name) = ty {
                            Some((
                                name.clone(),
                                SourceTypeRecord {
                                    private_fields: Vec::new(),
                                    kind: "opaque".into(),
                                    const_fields: Vec::new(),
                                    fields: Vec::new(),
                                    variants: Vec::new(),
                                },
                            ))
                        } else {
                            None
                        }
                    }),
            )
            .collect(),
        provided_header_digest,
    })
}

fn fallback_record(value: &rinzu_compiler::FallbackConstant) -> FallbackRecord {
    use rinzu_compiler::FallbackConstant as F;
    let (kind, scalar, value, bytes_length) = match value {
        F::Unit => ("unit", None, String::new(), None),
        F::Integer { scalar, value } => (
            "integer",
            Some(scalar.name().to_owned()),
            value.to_string(),
            None,
        ),
        F::Float { scalar, bits } => (
            "float-bits",
            Some(scalar.name().to_owned()),
            bits.to_string(),
            None,
        ),
        F::Bool(value) => ("boolean", None, value.to_string(), None),
        F::String(value) => ("string", None, value.clone(), None),
        F::Bytes { prefix, length } => (
            "bytes",
            None,
            prefix.iter().map(|byte| format!("{byte:02x}")).collect(),
            Some(length.to_string()),
        ),
    };
    FallbackRecord {
        kind: kind.to_owned(),
        scalar,
        value,
        bytes_length,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    mod supplant_capture_fixture {
        include!("../tests/fixtures/native_supplant_capture_sdk/setup.rs");
    }

    #[test]
    fn supplant_descriptor_retains_and_validates_private_capture_contracts() {
        let root =
            std::env::temp_dir().join(format!("rinzu-supplant-descriptor-{}", std::process::id()));
        supplant_capture_fixture::create(&root);
        let (_, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
        let package = artifact
            .sdk_packages
            .iter()
            .find(|package| package.manifest.name == "captures")
            .unwrap();
        let directory = root.join("generated");
        std::fs::create_dir_all(&directory).unwrap();
        let built = build(
            package,
            &root.join("app"),
            &NativeCliInputs::default(),
            &directory,
        )
        .unwrap();
        let descriptor = rinzu_bindgen::source_package::validate_source_descriptor(
            &std::fs::read(&built.descriptor).unwrap(),
        )
        .unwrap();
        let model = descriptor.interface;
        let wrapper = model
            .functions
            .iter()
            .find(|function| function.identity == "captures::exercise")
            .unwrap();
        assert_eq!(wrapper.callback_sites.len(), 1);
        let site = &wrapper.callback_sites[0];
        assert_eq!(site.parameter, 0);
        assert_eq!(site.source_type, "fn(Int)->Int");
        assert_eq!(
            model
                .functions
                .iter()
                .find(|function| function.identity == site.native_function)
                .unwrap()
                .kind,
            FunctionKind::Supplant
        );
        let capture = site.capture.as_ref().unwrap();
        assert!(model.source_types.contains_key(capture));
        assert!(
            model
                .source_types
                .keys()
                .any(|name| name.ends_with("Counter"))
        );
        for target in &site.targets {
            assert!(
                model
                    .functions
                    .iter()
                    .any(|function| function.code.as_ref() == Some(target))
            );
        }
        for mutation in 0..3 {
            let mut bad = model.clone();
            match mutation {
                0 => {
                    bad.source_types.remove(capture);
                }
                1 => {
                    bad.functions
                        .iter_mut()
                        .find(|function| function.identity == "captures::exercise")
                        .unwrap()
                        .callback_sites[0]
                        .parameter = 1
                }
                2 => {
                    bad.functions
                        .iter_mut()
                        .find(|function| function.identity == "captures::exercise")
                        .unwrap()
                        .callback_sites[0]
                        .capture = None
                }
                _ => unreachable!(),
            }
            assert!(
                generate_source_package(bad, b"native").is_err(),
                "mutation {mutation}"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    mod native_length_fixture {
        include!("../tests/fixtures/native_length_sdk/setup.rs");
    }
    mod native_opaque_fixture {
        include!("../tests/fixtures/native_opaque_sdk/setup.rs");
    }

    #[test]
    fn ordinary_file_interface_reuses_scalar_string_bytes_and_callback_glue() {
        let root = std::env::temp_dir().join(format!(
            "rinzu-ordinary-glue-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let _scratch = crate::verify_capture::TemporaryDirectory(root.clone());
        let entry = root.join("entry.rinzu");
        let source = rinzu_compiler::format_source(&rinzu_compiler::SourceFile::new(
            rinzu_compiler::FileId(1), &entry,
            "native{fn scalar(x:CInt)->CInt symbol \"rn_scalar\";fn text(x:String)->Int symbol \"rn_text\";fn bytes(x:Bytes(4))->Int symbol \"rn_bytes\";fn invoke(callback:fn(CInt)->CInt)->CInt symbol \"rn_invoke\";}fn main(){}",
        )).unwrap();
        std::fs::write(&entry, source).unwrap();
        let mut context = rinzu_compiler::check_input(
            &entry,
            &rinzu_compiler::CompilationOptions {
                language: None,
                target: "aarch64-apple-darwin".into(),
            },
            &NativeCliInputs::default(),
        )
        .unwrap();
        // Exercise the same checked provider plan used by ordinary CLI inputs.
        std::fs::write(root.join("fixture.c"), "#include <stdint.h>\n#include <string.h>\nint rn_scalar(int x){return x+2;}\nint64_t rn_text(const char *x){return (int64_t)strlen(x);}\nint64_t rn_bytes(const uint8_t *x){return x[0]+x[3];}\nint rn_invoke(int (*callback)(int)){return callback(40);}\n").unwrap();
        context.metadata.native.sources.push("fixture.c".into());
        let package = ResolvedSdkPackage {
            manifest: context.metadata,
            source: context.artifact.source_interface,
            native_imports: Vec::new(),
        };
        let output = root.join("generated");
        let built = build(&package, &root, &NativeCliInputs::default(), &output).unwrap();
        let descriptor = rinzu_bindgen::source_package::validate_source_descriptor(
            &built.artifact.expected_wire,
        )
        .unwrap();
        assert!(descriptor.interface.package.modules.is_empty());
        assert!(descriptor.interface.package.exports.is_empty());
        assert!(!root.join("rinzu.toml").exists());
        assert!(!root.join(".rinzu").exists());
        let loaded =
            rinzu_codegen::source_package::LoadedSourcePackages::load(&[built.artifact]).unwrap();
        let address = |name: &str| {
            loaded
                .imports()
                .values()
                .find(|value| value.identity == name)
                .unwrap()
                .glue_address()
        };
        // SAFETY: the checked fixture prototypes and all inputs match these C ABIs;
        // the generated facade and borrowed bytes remain alive through each call.
        unsafe {
            let scalar: unsafe extern "C" fn(i32) -> i32 = std::mem::transmute(address("scalar"));
            let text: unsafe extern "C" fn(*const std::ffi::c_char) -> i64 =
                std::mem::transmute(address("text"));
            let bytes: unsafe extern "C" fn(*const u8) -> i64 =
                std::mem::transmute(address("bytes"));
            let invoke: unsafe extern "C" fn(extern "C" fn(i32) -> i32) -> i32 =
                std::mem::transmute(address("invoke"));
            extern "C" fn callback(x: i32) -> i32 {
                x + 2
            }
            assert_eq!(scalar(40), 42);
            assert_eq!(text(c"native".as_ptr()), 6);
            assert_eq!(bytes([20_u8, 0, 0, 22].as_ptr()), 42);
            assert_eq!(invoke(callback), 42);
        }
    }

    #[test]
    fn revised_metadata_uses_manifest_language_const_fields_and_semantic_revisions() {
        let root =
            std::env::temp_dir().join(format!("rinzu-revised-metadata-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("rinzu.toml"),"[package]\nname=\"tooling\"\nlanguage=\"0.5\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n[native]\nsources=[\"api.c\"]\n").unwrap();
        std::fs::write(root.join("src/api.rinzu"),"class Data {const limit:Int,value:Int fn create()->Data{return Data(limit:10,value:0);}} native {fn echo(value:CInt)->CInt symbol \"echo\";} export Data;export echo;").unwrap();
        std::fs::write(root.join("api.c"), "int echo(int value){return value;}").unwrap();
        let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
        let source = artifact.source_interface.unwrap();
        let package = rinzu_compiler::ResolvedSdkPackage {
            manifest,
            source: Some(source),
            native_imports: vec![],
        };
        std::fs::create_dir_all(root.join("generated")).unwrap();
        let built = build(
            &package,
            &root,
            &NativeCliInputs::default(),
            &root.join("generated"),
        )
        .unwrap();
        let descriptor = rinzu_bindgen::source_package::validate_source_descriptor(
            &built.artifact.expected_wire,
        )
        .unwrap();
        assert_eq!(descriptor.interface.package.language, "rinzu-0.5");
        assert_eq!((descriptor.major, descriptor.minor), (2, 6));
        let inputs = &descriptor.interface.package.build_inputs;
        assert_eq!(
            inputs["rinzu.language-semantics"],
            rinzu_compiler::LANGUAGE_SEMANTICS_REVISION
        );
        assert_eq!(
            inputs["rinzu.private-runtime-abi"],
            rinzu_codegen::BASELINE_RUNTIME_ABI_REVISION
        );
        assert!(
            descriptor
                .interface
                .source_types
                .values()
                .any(|s| s.kind == "state" && s.const_fields == [0] && s.fields.len() == 2)
        );
        let mut changed = descriptor.interface;
        changed
            .package
            .build_inputs
            .insert("rinzu.language-semantics".into(), "previous".into());
        let changed =
            rinzu_bindgen::source_package::generate_source_package(changed, b"native").unwrap();
        assert_ne!(
            descriptor.semantic_digest,
            changed.descriptor.semantic_digest
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn compiled_jit_opaque_reservations_keep_owner_roots_and_release_null_error_ledgers() {
        let root =
            std::env::temp_dir().join(format!("rinzu-jit-native-opaque-{}", std::process::id()));
        native_opaque_fixture::create(&root);
        std::fs::write(root.join("app/src/main.rinzu"), r#"
            import objects;
            fn badSeed() -> Int { let bad = CInt8(999); return 42; }
            fn work() -> Int {
                let value = objects.measure(42);
                var order = 0;
                try {
                    try { let missing = objects.failure(); } finally { order = order * 10 + 1; }
                } catch [nullError: Error, ...nullErrorRest] { order = order * 10 + 2; if nullError.message != "native value is NULL" { panic("wrongError"); } }
                finally { order = order * 10 + 3; }
                if order != 123 { panic("wrongFinally"); }
                try { let skipped = objects.measure(badSeed()); } catch argumentError {}
                return value;
            }
            fn execute() -> Int { return work(); }
            fn unhandled() -> Int { let missing = objects.failure(); return 999; }
            fn main() { try { if execute() != 42 { panic("wrong"); } if false { let unused = unhandled(); } } catch unexpectedEntryErrors { panic("unexpectedEntryFailure"); } }
        "#).unwrap();
        let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
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
        // SAFETY: v2 load and target Clang validated these exact CSize(CInt)
        // and Unit() fixture prototypes. Facades are pinned for process lifetime.
        let stats: unsafe extern "C" fn(i32) -> u64 =
            unsafe { std::mem::transmute(address("object_stats")) };
        // SAFETY: this checked fixture cleanup has no parameters or result.
        let cleanup: unsafe extern "C" fn() =
            unsafe { std::mem::transmute(address("object_fixture_cleanup")) };
        let options = rinzu_codegen::BaselineOptions {
            collect_every_allocation: true,
            ..Default::default()
        };
        let run = rinzu_codegen::run_baseline_jit_with_package_bindings(
            &artifact.ir,
            "execute",
            options,
            None,
            Some(loaded),
        )
        .unwrap();
        let verify = |run: &rinzu_codegen::BaselineRun| {
            assert_eq!(run.result, Ok(42), "{run:?}");
            assert_eq!(run.caught_errors.len(), 2);
            assert_eq!(
                run.caught_errors[0].primary.cause,
                rinzu_runtime::failure::FailureCause::Error(
                    rinzu_runtime::failure::ErrorKind::NullNativeValue
                )
            );
            assert_eq!(
                run.caught_errors[1].primary.cause,
                rinzu_runtime::failure::FailureCause::Error(
                    rinzu_runtime::failure::ErrorKind::Conversion
                )
            );
            let calls = run.native_calls;
            assert_eq!(
                [
                    calls.prepared,
                    calls.returned,
                    calls.closed,
                    calls.settled,
                    calls.read,
                    calls.released
                ],
                [39; 6]
            );
            assert_eq!(calls.callback_entries, 32);
            assert_eq!(calls.copied_bytes, 128);
            assert!(calls.is_baseline() && calls.abandoned == 0);
            assert_eq!(run.heap_objects, 0);
            assert!(run.collection_count > 64);
            assert_eq!(run.published_code_versions, run.retired_code_versions);
            assert_eq!(run.rwx_mappings, 0);
        };
        verify(&run);
        let artifacts = built
            .source_packages
            .iter()
            .map(|package| package.artifact.clone())
            .collect::<Vec<_>>();
        let origin = std::thread::current().id();
        let workers = (0..8)
            .map(|_| {
                let ir = artifact.ir.clone();
                let artifacts = artifacts.clone();
                std::thread::spawn(move || {
                    assert_ne!(origin, std::thread::current().id());
                    let packages =
                        rinzu_codegen::source_package::LoadedSourcePackages::load(&artifacts)
                            .unwrap();
                    rinzu_codegen::run_baseline_jit_with_package_bindings(
                        &ir,
                        "work",
                        options,
                        None,
                        Some(packages),
                    )
                    .unwrap()
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            verify(&worker.join().unwrap());
        }
        // SAFETY: physical Owners have finished; these are explicit fixture
        // controls, never an implicit native destructor or GC finalizer.
        unsafe {
            assert_eq!(stats(0), 9);
            assert_eq!(stats(1), 9);
            assert_eq!(stats(2), 9);
            assert_eq!(stats(3), 0);
            assert_eq!(stats(4), 18);
            cleanup();
        }
        let uncaught = rinzu_codegen::run_baseline_jit_with_package_bindings(
            &artifact.ir,
            "unhandled",
            options,
            None,
            Some(built.load_source_packages().unwrap()),
        )
        .unwrap();
        assert_eq!(
            uncaught.result,
            Err(rinzu_codegen::BaselineFailure::Error(
                rinzu_runtime::failure::ErrorKind::NullNativeValue
            ))
        );
        assert_eq!(
            uncaught.failure_details.as_ref().unwrap().primary.cause,
            rinzu_runtime::failure::FailureCause::Error(
                rinzu_runtime::failure::ErrorKind::NullNativeValue
            )
        );
        assert_eq!(uncaught.native_calls.prepared, 1);
        assert_eq!(uncaught.native_calls.released, 1);
        assert!(
            uncaught.native_calls.is_baseline() && uncaught.heap_objects == 0,
            "{uncaught:?}"
        );
        // SAFETY: the failed call has returned, with no successful opaque value.
        unsafe {
            assert_eq!(stats(0), 0);
            assert_eq!(stats(4), 1);
            cleanup();
        }
        for limit in [1, 3, 5, 10, 20, 100, run.poll_count] {
            let cancelled = rinzu_codegen::run_baseline_jit_with_package_bindings(
                &artifact.ir,
                "execute",
                rinzu_codegen::BaselineOptions {
                    cancel_after_polls: Some(limit),
                    ..options
                },
                None,
                Some(built.load_source_packages().unwrap()),
            )
            .unwrap();
            let calls = cancelled.native_calls;
            assert_eq!(
                [
                    calls.returned,
                    calls.closed,
                    calls.settled,
                    calls.read,
                    calls.released
                ],
                [calls.prepared; 5]
            );
            assert!(calls.is_baseline() && calls.abandoned == 0 && cancelled.heap_objects == 0);
            // SAFETY: cancellation occurred at a managed poll after real return;
            // explicit test cleanup owns the safe fixture's remaining entities.
            unsafe {
                assert_eq!(stats(0), u64::from(calls.prepared >= 1));
                assert_eq!(stats(1), u64::from(calls.prepared >= 36));
                assert_eq!(stats(2), u64::from(calls.prepared >= 3));
                assert_eq!(stats(3), stats(0) - stats(1));
                cleanup();
            }
        }
        std::fs::remove_dir_all(built.directory).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn compiled_jit_native_ledger_uses_the_exact_plan_on_main_and_physical_workers() {
        let root = std::env::temp_dir().join(format!(
            "rinzu-compiled-native-ledger-{}",
            std::process::id()
        ));
        native_length_fixture::create(&root);
        std::fs::write(
            root.join("app/src/main.rinzu"),
            r#"
            import measure;
            fn work() -> Int { return measure.measure(b"A\xff", "日本語\u{0}🙂"); }
            fn execute() -> Int { return work(); }
            fn main() { if execute() != 13 { panic("failed"); } }
        "#,
        )
        .unwrap();
        let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
        let built = crate::native_package::build_with_native_inputs(
            &artifact.sdk_packages,
            &manifest.root,
            &artifact.native_inputs,
        )
        .unwrap();
        let options = rinzu_codegen::BaselineOptions {
            collect_every_allocation: true,
            ..Default::default()
        };
        let run = rinzu_codegen::run_baseline_jit_with_package_bindings(
            &artifact.ir,
            "execute",
            options,
            None,
            Some(built.load_source_packages().unwrap()),
        )
        .unwrap();
        let verify = |run: &rinzu_codegen::BaselineRun| {
            assert_eq!(run.result, Ok(13), "{run:?}");
            assert_eq!(run.heap_objects, 0);
            assert!(run.collection_count > 190 && run.root_publications > 10);
            assert_eq!(run.published_code_versions, run.retired_code_versions);
            let ledger = run.native_calls;
            assert_eq!(
                (
                    ledger.prepared,
                    ledger.returned,
                    ledger.closed,
                    ledger.settled,
                    ledger.read,
                    ledger.released
                ),
                (5, 5, 5, 5, 5, 5)
            );
            assert_eq!(ledger.callback_entries, 96);
            assert_eq!(ledger.copied_bytes, 416);
            assert!(ledger.is_baseline() && ledger.abandoned == 0);
            assert_eq!(run.rwx_mappings, 0);
        };
        verify(&run);
        let artifacts = built
            .source_packages
            .iter()
            .map(|package| package.artifact.clone())
            .collect::<Vec<_>>();
        let owner = std::thread::current().id();
        let workers = (0..8)
            .map(|_| {
                let ir = artifact.ir.clone();
                let artifacts = artifacts.clone();
                std::thread::spawn(move || {
                    assert_ne!(std::thread::current().id(), owner);
                    let packages =
                        rinzu_codegen::source_package::LoadedSourcePackages::load(&artifacts)
                            .unwrap();
                    rinzu_codegen::run_baseline_jit_with_package_bindings(
                        &ir,
                        "work",
                        options,
                        None,
                        Some(packages),
                    )
                    .unwrap()
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            verify(&worker.join().unwrap());
        }
        let mut cancelled_after_return = false;
        for limit in [1, 3, 5, 7, 9, 15, 20, run.poll_count] {
            let run = rinzu_codegen::run_baseline_jit_with_package_bindings(
                &artifact.ir,
                "execute",
                rinzu_codegen::BaselineOptions {
                    cancel_after_polls: Some(limit),
                    ..options
                },
                None,
                Some(built.load_source_packages().unwrap()),
            )
            .unwrap();
            assert!(
                run.native_calls.is_baseline() && run.heap_objects == 0,
                "{run:?}"
            );
            let ledger = run.native_calls;
            assert_eq!(
                (
                    ledger.returned,
                    ledger.closed,
                    ledger.settled,
                    ledger.read,
                    ledger.released
                ),
                (
                    ledger.prepared,
                    ledger.prepared,
                    ledger.prepared,
                    ledger.prepared,
                    ledger.prepared
                )
            );
            assert!(ledger.abandoned == 0 && ledger.callback_entries.is_multiple_of(32));
            cancelled_after_return |= run.result == Err(rinzu_codegen::BaselineFailure::Cancelled)
                && ledger.callback_entries > 0;
        }
        assert!(cancelled_after_return);
        std::fs::remove_dir_all(built.directory).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_native_ledger_keeps_main_and_worker_roots_through_physical_return() {
        let root =
            std::env::temp_dir().join(format!("rinzu-source-native-ledger-{}", std::process::id()));
        native_length_fixture::create(&root);
        let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
        let built = crate::native_package::build_with_native_inputs(
            &artifact.sdk_packages,
            &manifest.root,
            &artifact.native_inputs,
        )
        .unwrap();
        let run = rinzu_codegen::run_source_async(
            &artifact.ir,
            "main",
            rinzu_codegen::SourceAsyncOptions {
                gc_stress: true,
                source_packages: Some(built.load_source_packages().unwrap()),
                record_transport_evidence: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(run.result, 0);
        assert!(run.resources_returned, "{run:?}");
        assert_eq!(run.worker_returns, 8);
        assert_eq!(run.worker_heaps.len(), 8);
        assert_eq!(run.caught_errors.len(), 1);
        assert!(
            run.heap.moved_objects > 100 && run.heap.peak_frames >= 4,
            "{run:?}"
        );
        assert!(
            run.worker_heaps
                .iter()
                .all(|worker| worker.heap.collections > 190
                    && worker.heap.moved_objects > 100
                    && worker.heap.objects == 0
                    && worker.heap.roots == 0)
        );
        let ledger = run.native_calls;
        assert_eq!(
            (
                ledger.prepared,
                ledger.returned,
                ledger.closed,
                ledger.settled,
                ledger.read,
                ledger.released
            ),
            (46, 46, 46, 46, 46, 46)
        );
        assert_eq!(ledger.abandoned, 0);
        assert_eq!(ledger.callback_entries, 9 * 3 * 32);
        assert_eq!(ledger.copied_bytes, 9 * 32 * 13);
        assert!(ledger.is_baseline());
        assert!(
            run.transport_evidence
                .iter()
                .all(|credit| credit.released_at.is_some())
        );
        std::fs::write(
            root.join("app/src/main.rinzu"),
            r#"
            import measure;
            fn work() -> Int {
                let measured = measure.measure(b"A\xff", "日本語\u{0}🙂");
                var iteration = 0;
                while iteration < 1000 { iteration = iteration + 1; }
                return measured;
            }
            async fn main() { let returned = await spawn work(); }
        "#,
        )
        .unwrap();
        let (_, cancellation_artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
        let mut cancelled_after_native_return = false;
        for limit in [0, 1, 3, 5, 10, 20, 100] {
            let cancelled = rinzu_codegen::run_source_async_report(
                &cancellation_artifact.ir,
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
            assert_eq!(
                cancelled.failure_details.as_ref().unwrap().primary.cause,
                rinzu_runtime::failure::FailureCause::Cancelled
            );
            assert!(cancelled.resources_returned, "limit={limit}: {cancelled:?}");
            let ledger = cancelled.native_calls;
            assert_eq!(
                (
                    ledger.returned,
                    ledger.closed,
                    ledger.settled,
                    ledger.read,
                    ledger.released
                ),
                (
                    ledger.prepared,
                    ledger.prepared,
                    ledger.prepared,
                    ledger.prepared,
                    ledger.prepared
                )
            );
            assert!(ledger.abandoned == 0 && ledger.is_baseline());
            assert_eq!(ledger.callback_entries % 32, 0);
            cancelled_after_native_return |= ledger.callback_entries > 0;
            assert_eq!(cancelled.worker_returns, 1);
            assert!(
                cancelled
                    .transport_evidence
                    .iter()
                    .all(|credit| credit.released_at.is_some())
            );
        }
        assert!(cancelled_after_native_return);
        // The source signature must still match the facade even if typed IR was altered.
        let mut changed = artifact.ir.clone();
        changed
            .source_native_signatures
            .iter_mut()
            .find(|signature| signature.name.ends_with("::invoke"))
            .unwrap()
            .parameters[1] = SourceAbiType::Bytes(8);
        let rejected = rinzu_codegen::run_source_async(
            &changed,
            "main",
            rinzu_codegen::SourceAsyncOptions {
                gc_stress: true,
                source_packages: Some(built.load_source_packages().unwrap()),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(
            format!("{rejected:#}").contains("signature disagrees"),
            "{rejected:#}"
        );
        std::fs::remove_dir_all(built.directory).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn callback_sites_keep_resolved_private_lengths_targets_and_exact_public_fn_types() {
        let root =
            std::env::temp_dir().join(format!("rinzu-callback-artifact-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("rinzu.toml"), "[package]\nname = \"callback\"\nlanguage = \"0.2\"\nkind = \"sdk\"\nentry = \"src/api.rinzu\"\n[native]\nsources = [\"api.c\"]\n").unwrap();
        std::fs::write(root.join("api.c"), "uint64_t invoke(uint64_t (*cb)(const unsigned char *), const unsigned char *data) { return cb(data); }\n").unwrap();
        let source = |n| {
            format!(
                r#"
            let privateWidth: CSize = {n};
            native {{ fn invoke(cb: fn(Bytes(privateWidth)) -> CSize, data: Bytes(privateWidth)) -> CSize symbol "invoke"; }}
            fn measure(data: Bytes(privateWidth)) -> CSize {{ let alias: fn(Bytes(privateWidth)) -> CSize = byteLength; return invoke(alias, data); }}
            fn four(data: Bytes(privateWidth)) -> CSize fallback 0 {{ return byteLength(data); }}
            fn withFallback(data: Bytes(privateWidth)) -> CSize {{ return invoke(four, data); }}
            fn apply(callback: fn(Bytes(privateWidth)) -> CSize, data: Bytes(privateWidth)) -> CSize {{ return invoke(callback, data); }}
            export measure; export apply; export withFallback;
        "#
            )
        };
        let mut descriptors = Vec::new();
        for n in [4, 8] {
            std::fs::write(root.join("src/api.rinzu"), source(n)).unwrap();
            let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
            let package = ResolvedSdkPackage {
                manifest,
                source: artifact.source_interface,
                native_imports: Vec::new(),
            };
            let directory = root.join(format!("generated-{n}"));
            std::fs::create_dir_all(&directory).unwrap();
            let built = build(&package, &root, &NativeCliInputs::default(), &directory).unwrap();
            let descriptor = rinzu_bindgen::source_package::validate_source_descriptor(
                &std::fs::read(&built.descriptor).unwrap(),
            )
            .unwrap();
            let expected = format!("fn(Bytes({n}))->CUInt64");
            let find = |name| {
                descriptor
                    .interface
                    .functions
                    .iter()
                    .find(|f| f.identity == name)
                    .unwrap()
            };
            let apply = find("apply");
            assert_eq!(
                apply.source_type,
                format!("fn({expected},Bytes({n}))->CUInt64")
            );
            assert_eq!(apply.parameter_requirements[0], expected);
            assert_eq!(apply.callback_sites[0].parameter, 0);
            assert_eq!(apply.callback_sites[0].source_type, expected);
            let measure = find("measure");
            assert_eq!(
                measure.callback_sites[0].targets[0],
                CodeRecord {
                    declaration: format!("rinzu.core::byteLength:{n}"),
                    version: 1,
                    fallback: None
                }
            );
            assert_eq!(
                find("withFallback").callback_sites[0].targets[0],
                find("four").code.clone().unwrap()
            );
            assert_eq!(
                find("four")
                    .code
                    .as_ref()
                    .unwrap()
                    .fallback
                    .as_ref()
                    .unwrap()
                    .value,
                "0"
            );
            assert!(descriptor.interface.constants.is_empty());
            assert!(
                !String::from_utf8(std::fs::read(&built.descriptor).unwrap())
                    .unwrap()
                    .contains("privateWidth")
            );
            let header = std::fs::read_to_string(&built.contract).unwrap();
            assert!(
                header.contains("uint64_t (*a0)(const uint8_t * p0)"),
                "{header}"
            );
            assert!(header.contains("const uint8_t * a1"), "{header}");
            let loaded =
                rinzu_codegen::source_package::LoadedSourcePackages::load(&[built.artifact])
                    .unwrap();
            let stubs = rinzu_codegen::source_native_abi::NativeAbiStubs::new(
                &package.source.as_ref().unwrap().signatures,
            )
            .unwrap();
            let import = loaded
                .imports()
                .values()
                .find(|import| import.identity == "invoke")
                .unwrap();
            let mut heap = rinzu_runtime::heap::DomainHeap::new(n);
            let mut data = rinzu_runtime::data::ManagedData::new(n);
            let mut roots = rinzu_runtime::data::ManagedRoots::default();
            let callback = rinzu_runtime::callback_entry::LengthCallback::prepare(
                &mut heap,
                &mut data,
                &mut roots,
                rinzu_runtime::callback_entry::LengthKind::Bytes(n),
            );
            let input = vec![255_u8; n as usize];
            assert_eq!(
                // SAFETY: the validated facade owns this exact callback/Bytes(N)
                // prototype; input and callback remain pinned on the Origin thread.
                unsafe {
                    stubs.invoke(
                        "invoke",
                        import.glue_address() as usize,
                        &[callback.entry() as usize as u64, input.as_ptr() as u64],
                    )
                }
                .unwrap(),
                n
            );
            assert_eq!(callback.evidence().prepared_inputs, 1);
            assert_eq!(callback.evidence().body_entries, 1);
            drop(callback);
            assert_eq!(heap.object_count(), 0);
            assert_eq!(data.root_count(), 0);
            descriptors.push(descriptor);
        }
        assert_ne!(
            descriptors[0].semantic_digest,
            descriptors[1].semantic_digest
        );
        assert_ne!(descriptors[0].import_keys, descriptors[1].import_keys);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn generated_facade_owns_typed_imports_for_source_archive_and_shared_inputs() {
        for kind in ["c", "m", "o", "a", "dylib"] {
            let root = std::env::temp_dir()
                .join(format!("rinzu-source-build-{}-{kind}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("src")).unwrap();
            std::fs::write(
                root.join("src/api.rinzu"),
                "native { fn add(a: CInt, b: CInt) -> CInt symbol \"api_add\"; } export add;",
            )
            .unwrap();
            std::fs::write(root.join("api.h"), "int api_add(int a, int b);\n").unwrap();
            let source_name = if kind == "m" { "api.m" } else { "api.c" };
            std::fs::write(
                root.join(source_name),
                "int api_add(int a, int b) { return a + b; }\n",
            )
            .unwrap();
            let native = match kind {
                "c" | "m" => format!("sources = [\"{source_name}\"]"),
                "o" | "a" => {
                    let mut command = TargetClang::resolve().unwrap().command();
                    command
                        .arg("-c")
                        .arg(root.join(source_name))
                        .arg("-o")
                        .arg(root.join("api.o"));
                    run(command, "fixture object").unwrap();
                    if kind == "a" {
                        let mut command = std::process::Command::new("ar");
                        command
                            .arg("rcs")
                            .arg(root.join("api.a"))
                            .arg(root.join("api.o"));
                        run(command, "fixture archive").unwrap();
                    }
                    format!("objects = [\"api.{kind}\"]")
                }
                _ => {
                    let library = root.join("api.dylib");
                    let mut command = TargetClang::resolve().unwrap().command();
                    command
                        .arg("-dynamiclib")
                        .arg(root.join(source_name))
                        .arg("-install_name")
                        .arg(&library)
                        .arg("-o")
                        .arg(&library);
                    run(command, "fixture shared library").unwrap();
                    "objects = [\"api.dylib\"]".to_owned()
                }
            };
            std::fs::write(root.join("rinzu.toml"), format!("[package]\nname = \"api\"\nlanguage = \"0.2\"\nkind = \"sdk\"\nentry = \"src/api.rinzu\"\n[native]\nheaders = [\"api.h\"]\n{native}\n")).unwrap();
            let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
            let package = ResolvedSdkPackage {
                manifest,
                source: artifact.source_interface,
                native_imports: Vec::new(),
            };
            let directory = root.join("generated");
            std::fs::create_dir_all(&directory).unwrap();
            let built = build(&package, &root, &NativeCliInputs::default(), &directory).unwrap();
            let loaded =
                rinzu_codegen::source_package::LoadedSourcePackages::load(&[built.artifact])
                    .unwrap();
            assert_eq!(loaded.imports().len(), 1);
            let address = loaded.imports().values().next().unwrap().glue_address();
            // SAFETY: fixture's validated contract is precisely int(int, int).
            let add: unsafe extern "C" fn(i32, i32) -> i32 =
                unsafe { std::mem::transmute(address) };
            // SAFETY: the validated fixture accepts any two int32_t values.
            assert_eq!(unsafe { add(17, 25) }, 42, "{kind}");
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn record_callback_library_only_and_service_only_builds() {
        let cases = [
            (
                "record",
                "native { record Pair { x: CInt, y: CDouble } fn read(pair: Pair, callback: fn(CInt) -> CInt) -> CInt symbol \"read_pair\"; } export Pair; export read;",
                "sources = [\"api.c\"]\nheaders = [\"api.h\"]",
                "#include <stdint.h>\nstruct Pair { int32_t x; double y; }; int32_t read_pair(const struct Pair *pair, int32_t (*callback)(int32_t)) { return callback(pair->x); }\n",
            ),
            (
                "library",
                "native { fn pid() -> CInt symbol \"getpid\"; } export pid;",
                "libraries = [\"System\"]",
                "",
            ),
            (
                "service",
                "import rinzu.timer;\nasync fn timer(delay: Int) -> Unit { await rinzu.timer.sleep(delay); }\nnative {  } export timer;",
                "",
                "",
            ),
        ];
        for (name, source, flags, c_source) in cases {
            let root = std::env::temp_dir()
                .join(format!("rinzu-source-build-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("src")).unwrap();
            std::fs::write(root.join("src/api.rinzu"), source).unwrap();
            std::fs::write(root.join("api.c"), if name == "record" { "int32_t read_pair(const struct Pair *pair, int32_t (*callback)(int32_t)) { return callback(pair->x); }\n" } else { c_source }).unwrap();
            if name == "record" {
                std::fs::write(root.join("api.h"), "#include <stdint.h>\nstruct Pair { int32_t x; double y; }; int32_t read_pair(const struct Pair *pair, int32_t (*callback)(int32_t));\n").unwrap();
            }
            std::fs::write(root.join("rinzu.toml"), format!("[package]\nname = \"{name}\"\nlanguage = \"0.2\"\nkind = \"sdk\"\nentry = \"src/api.rinzu\"\n[native]\n{flags}\n")).unwrap();
            let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
            let package = ResolvedSdkPackage {
                manifest,
                source: artifact.source_interface,
                native_imports: Vec::new(),
            };
            let directory = root.join("generated");
            std::fs::create_dir_all(&directory).unwrap();
            let built = build(&package, &root, &NativeCliInputs::default(), &directory).unwrap();
            assert_eq!(built.artifact.library.is_none(), name == "service");
            rinzu_codegen::source_package::LoadedSourcePackages::load(&[built.artifact]).unwrap();
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn generic_error_constructor_metadata_survives_source_package_generation() {
        let root =
            std::env::temp_dir().join(format!("rinzu-generic-errors-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("rinzu.toml"), "[package]\nname=\"genericerrors\"\nlanguage=\"0.5\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n").unwrap();
        let mut digests = Vec::new();
        for payload in ["payload?:T", "payload:Option<T>"] {
            std::fs::write(
                root.join("src/api.rinzu"),
                format!("error Failed<T>{{{payload}}}export Failed;"),
            )
            .unwrap();
            let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
            let package = ResolvedSdkPackage {
                manifest,
                source: artifact.source_interface,
                native_imports: vec![],
            };
            let directory = root.join("generated");
            std::fs::create_dir_all(&directory).unwrap();
            let built = build(&package, &root, &NativeCliInputs::default(), &directory).unwrap();
            let descriptor = rinzu_bindgen::source_package::validate_source_descriptor(
                &built.artifact.expected_wire,
            )
            .unwrap();
            assert_eq!(
                descriptor.interface.source_templates["Failed"].kind,
                "error"
            );
            assert_eq!(
                descriptor.interface.source_templates["Failed"].fields[4].1,
                "Option<T>"
            );
            digests.push(descriptor.semantic_digest);
            rinzu_codegen::source_package::LoadedSourcePackages::load(&[built.artifact]).unwrap();
        }
        assert_ne!(digests[0], digests[1]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn checkout_location_and_timestamps_do_not_change_source_interface() {
        let mut descriptors = Vec::new();
        for index in 0..2 {
            let root = std::env::temp_dir().join(format!(
                "rinzu-source-digest-{}-{index}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("src")).unwrap();
            std::fs::write(
                root.join("src/api.rinzu"),
                "native { fn answer() -> CInt symbol \"answer\"; } export answer;",
            )
            .unwrap();
            std::fs::write(root.join("api.h"), "int answer(void);\n").unwrap();
            std::fs::write(root.join("value header.h"), "#define ANSWER 42\n").unwrap();
            std::fs::write(
                root.join("api.c"),
                "#include \"value header.h\"\nint answer(void) { return ANSWER; }\n",
            )
            .unwrap();
            std::fs::write(root.join("rinzu.toml"), "[package]\nname = \"api\"\nlanguage = \"0.2\"\nkind = \"sdk\"\nentry = \"src/api.rinzu\"\n[native]\nheaders = [\"api.h\"]\nsources = [\"api.c\"]\ninclude-dirs = [\".\"]\n").unwrap();
            let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
            let package = ResolvedSdkPackage {
                manifest,
                source: artifact.source_interface,
                native_imports: Vec::new(),
            };
            let directory = root.join("generated");
            std::fs::create_dir_all(&directory).unwrap();
            let built = build(&package, &root, &NativeCliInputs::default(), &directory).unwrap();
            let descriptor = rinzu_bindgen::source_package::validate_source_descriptor(
                &built.artifact.expected_wire,
            )
            .unwrap();
            assert!(
                !String::from_utf8(built.artifact.expected_wire)
                    .unwrap()
                    .contains(root.to_str().unwrap())
            );
            std::fs::write(root.join("value header.h"), "#define ANSWER 43\n").unwrap();
            let changed = build(&package, &root, &NativeCliInputs::default(), &directory).unwrap();
            let changed = rinzu_bindgen::source_package::validate_source_descriptor(
                &changed.artifact.expected_wire,
            )
            .unwrap();
            assert_ne!(descriptor.semantic_digest, changed.semantic_digest);
            assert_ne!(
                descriptor.native_artifact_digest,
                changed.native_artifact_digest
            );
            assert_eq!(descriptor.import_keys, changed.import_keys);
            descriptors.push(descriptor);
            std::fs::write(root.join("src/api.rinzu"), "// changed after checking\n").unwrap();
            let error = build(&package, &root, &NativeCliInputs::default(), &directory)
                .err()
                .unwrap();
            assert!(error.contains("changed after checking"), "{error}");
            std::fs::remove_dir_all(root).unwrap();
        }
        assert_eq!(descriptors[0], descriptors[1]);
    }
    #[test]
    fn selected_library_content_invalidates_digest_and_rpath_resolves_facade_dependency() {
        let root =
            std::env::temp_dir().join(format!("rinzu-selected-library-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("native")).unwrap();
        std::fs::write(
            root.join("src/api.rinzu"),
            "native { fn answer() -> CInt symbol \"answer\"; } export answer;",
        )
        .unwrap();
        std::fs::write(root.join("rinzu.toml"), "[package]\nname = \"api\"\nlanguage = \"0.2\"\nkind = \"sdk\"\nentry = \"src/api.rinzu\"\n[native]\nsearch-paths = [\"native\"]\nlibraries = [\"answer\"]\n").unwrap();
        let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
        let package = ResolvedSdkPackage {
            manifest,
            source: artifact.source_interface,
            native_imports: Vec::new(),
        };
        let mut descriptors = Vec::new();
        let mut final_artifact = None;
        for answer in [42, 43] {
            let source = root.join("native/answer.c");
            std::fs::write(
                &source,
                format!("int answer(void) {{ return {answer}; }}\n"),
            )
            .unwrap();
            let mut command = TargetClang::resolve().unwrap().command();
            command
                .arg("-dynamiclib")
                .arg(&source)
                .arg("-install_name")
                .arg("@rpath/libanswer.dylib")
                .arg("-o")
                .arg(root.join("native/libanswer.dylib"));
            run(command, "fixture library").unwrap();
            let directory = root.join(format!("generated-{answer}"));
            std::fs::create_dir_all(&directory).unwrap();
            let built = build(&package, &root, &NativeCliInputs::default(), &directory).unwrap();
            let descriptor = rinzu_bindgen::source_package::validate_source_descriptor(
                &built.artifact.expected_wire,
            )
            .unwrap();
            assert!(
                descriptor
                    .interface
                    .package
                    .build_inputs
                    .contains_key("link:sdk:native/libanswer.dylib")
            );
            descriptors.push(descriptor);
            final_artifact = Some(built.artifact);
        }
        assert_ne!(
            descriptors[0].semantic_digest,
            descriptors[1].semantic_digest
        );
        assert_ne!(
            descriptors[0].native_artifact_digest,
            descriptors[1].native_artifact_digest
        );
        assert_eq!(descriptors[0].import_keys, descriptors[1].import_keys);
        let loaded =
            rinzu_codegen::source_package::LoadedSourcePackages::load(&[final_artifact.unwrap()])
                .unwrap();
        let address = loaded.imports().values().next().unwrap().glue_address();
        // SAFETY: the fixture contract is exactly int(void), checked before publication.
        let answer: unsafe extern "C" fn() -> i32 = unsafe { std::mem::transmute(address) };
        // SAFETY: validated zero-argument function with a scalar result.
        assert_eq!(unsafe { answer() }, 43);
        std::fs::remove_dir_all(root).unwrap();
    }
}
