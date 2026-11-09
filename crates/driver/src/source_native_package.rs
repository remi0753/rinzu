use crate::native_clang::TargetClang;
use crate::native_contract::validate_package_with_toolchain;
use rinzu_bindgen::source_package::{SourceInterface, generate_source_package, sha256};
use rinzu_codegen::source_package::SourcePackageArtifact;
use rinzu_compiler::ast::NativeBinding;
use rinzu_compiler::{NativeCliInputs, ResolvedSdkPackage, SourceAbiType};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

mod interface;

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
    let mut interface = interface::build(package, source, &contract, application_root, inputs)?;
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
            let source = package.manifest.root.join(source_path);
            command
                .arg(&source)
                .arg("-o")
                .arg(if analyze_consumption { &ir } else { &output });
            crate::compile_cache::compile(
                &toolchain,
                command,
                directory,
                if analyze_consumption { &ir } else { &output },
                &source,
                "compile source SDK object",
            )?;
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
        let first_link = crate::cli_diagnostics::tool_output(
            command.output(),
            Some(&library),
            "source SDK linking",
            "clang",
        )?;
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
            let final_link = crate::cli_diagnostics::tool_output(
                command.output(),
                Some(&library),
                "source SDK linking",
                "clang",
            )?;
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
