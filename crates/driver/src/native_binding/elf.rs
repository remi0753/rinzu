use super::*;
use crate::platform::linux::elf;
use object::{ObjectKind, SegmentFlags, SymbolFlags};
use object::{ObjectSegment, elf as constants};

impl ReferenceIndex {
    pub(super) fn elf_link_input(
        &mut self,
        selected: PathBuf,
        name: &str,
        trace: &str,
        sysroot: &Path,
    ) -> Result<PathBuf, String> {
        let selected = selected.canonicalize().map_err(|error| error.to_string())?;
        let bytes = std::fs::read(&selected).map_err(|error| error.to_string())?;
        if bytes.starts_with(b"\x7fELF") || bytes.starts_with(b"!<arch>\n") {
            return Ok(selected);
        }
        // glibc's unversioned libc.so and libm.so are linker scripts. Track
        // the script and use the versioned implementation reported by ld.
        let prefix = format!("lib{name}.so.");
        let paths: BTreeSet<_> = trace
            .lines()
            .filter_map(|line| Path::new(line).canonicalize().ok())
            .filter(|path| {
                path.file_name()
                    .and_then(|leaf| leaf.to_str())
                    .is_some_and(|leaf| leaf.starts_with(&prefix))
                    && system_image(path, sysroot)
            })
            .collect();
        if !system_image(&selected, sysroot) || paths.len() != 1 {
            return Err(format!(
                "ELF library `{name}` has no unique system image behind {}",
                selected.display()
            ));
        }
        self.inputs.insert(
            format!("native.elf-script:{}", selected.display()),
            sha256(&bytes),
        );
        Ok(paths.into_iter().next().unwrap())
    }

    pub(super) fn elf_object(&mut self, bytes: &[u8], identity: &str) -> Result<(), String> {
        let file = elf::parse(bytes, ObjectKind::Relocatable)
            .map_err(|error| format!("native object {identity}: {error}"))?;
        for symbol in file
            .symbols()
            .filter(|symbol| symbol.is_global() && symbol.is_definition())
        {
            let name = symbol.name().map_err(|error| error.to_string())?;
            let executable = symbol.kind() == object::SymbolKind::Text
                && symbol
                    .section_index()
                    .and_then(|index| file.section_by_index(index).ok())
                    .is_some_and(|section| section.kind() == object::SectionKind::Text);
            self.insert(
                name.into(),
                Definition {
                    identity: format!("{identity}:symbol:{name}:x86_64-unknown-linux-gnu"),
                    owner: None,
                    root: None,
                    executable,
                },
            );
        }
        Ok(())
    }

    pub(super) fn elf_input(
        &mut self,
        path: &Path,
        bytes: &[u8],
        identity: &str,
    ) -> Result<(), String> {
        let file = object::File::parse(bytes)
            .map_err(|error| format!("ELF input {}: {error}", path.display()))?;
        match file.kind() {
            ObjectKind::Relocatable => {
                elf::parse(bytes, ObjectKind::Relocatable)
                    .map_err(|error| format!("native object {}: {error}", path.display()))?;
                self.object(bytes, &format!("object:{identity}"))
            }
            ObjectKind::Dynamic => {
                let image = elf::image_info(path, bytes)?;
                let key = format!("{}:{}", image.path, image.identity());
                self.inspected_images
                    .insert(key.clone(), image.sha256.clone());
                self.inputs.insert(
                    format!("native.elf:{key}"),
                    image.soname.clone().unwrap_or_default(),
                );
                if !self.inputs_only {
                    self.elf_exports(bytes, &image)?;
                }
                Ok(())
            }
            _ => Err(format!(
                "expected an ELF object or shared library: {}",
                path.display()
            )),
        }
    }

    fn elf_exports(&mut self, data: &[u8], image: &NativeProviderImage) -> Result<(), String> {
        let (file, _, _) = elf::shared(data)?;
        let endian = file.endian();
        let versions = file
            .elf_section_table()
            .versions(endian, data)
            .map_err(|error| error.to_string())?;
        let text: Vec<_> = file.segments().filter(|segment| {
            matches!(segment.flags(), SegmentFlags::Elf { p_flags, .. } if p_flags.contains(constants::PF_X))
        }).map(|segment| (segment.address(), segment.size())).collect();
        for symbol in file
            .dynamic_symbols()
            .filter(|symbol| symbol.is_definition() && symbol.is_global())
        {
            let SymbolFlags::Elf { st_info, st_other } = symbol.flags() else {
                continue;
            };
            if !matches!(
                st_other.visibility(),
                constants::STV_DEFAULT | constants::STV_PROTECTED
            ) || versions
                .as_ref()
                .is_some_and(|table| table.version_index(endian, symbol.index()).is_hidden())
            {
                continue;
            }
            let name = symbol.name().map_err(|error| error.to_string())?;
            let binding = if symbol.is_weak() { "weak" } else { "global" };
            let executable = st_info.st_type() == constants::STT_FUNC
                && text.iter().any(|(base, size)| {
                    symbol
                        .address()
                        .checked_sub(*base)
                        .is_some_and(|offset| offset < *size)
                });
            self.insert(
                name.into(),
                Definition {
                    identity: format!(
                        "{}:{}:{binding}:{name}:{}",
                        image.path,
                        image.identity(),
                        symbol.index().0
                    ),
                    owner: Some(image.clone()),
                    root: Some(image.clone()),
                    executable,
                },
            );
        }
        Ok(())
    }

    pub(super) fn bind_elf(
        &self,
        source: &SourceNativeInterface,
        toolchain: &TargetClang,
        package: &ResolvedSdkPackage,
        selections: Vec<(&rinzu_compiler::SourceNativeSignature, &Definition)>,
        output: ProviderOutput<'_>,
    ) -> Result<(NativeBindingPlan, BTreeMap<String, String>), String> {
        let ProviderOutput {
            directory,
            name: provider_name,
            record: record_provider,
        } = output;
        let mut inputs = self.inputs.clone();
        let mut images = Vec::new();
        for path in &self.link_paths {
            let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
            if elf::parse(&bytes, ObjectKind::Dynamic).is_ok() {
                let mut image = elf::image_info(path, &bytes)?;
                // System dependencies retain their absolute selected image. Explicit
                // package/CLI DSOs travel alongside the provider under their SONAME.
                if !system_image(path, &toolchain.sysroot) {
                    let name = image.soname.as_deref().ok_or_else(|| {
                        format!("bundled ELF input requires SONAME: {}", path.display())
                    })?;
                    if name == provider_name
                        || images
                            .iter()
                            .any(|image: &NativeProviderImage| image.path == name)
                    {
                        return Err(format!("duplicate ELF SONAME: {name}"));
                    }
                    std::fs::write(directory.join(name), &bytes)
                        .map_err(|error| error.to_string())?;
                    image.path = name.to_owned();
                    image.bundled = true;
                }
                images.push(image);
            }
        }
        if images
            .iter()
            .filter_map(|image| image.soname.as_deref())
            .collect::<BTreeSet<_>>()
            .len()
            != images.iter().filter(|image| image.soname.is_some()).count()
        {
            return Err("ELF reference scope selects multiple images with the same SONAME".into());
        }
        let provider = if self.link_paths.iter().any(|path| {
            path.extension()
                .is_some_and(|ext| matches!(ext.to_str(), Some("o" | "a")))
        }) {
            let companion = directory.join("native-addresses.c");
            let mut anchors = String::from("#include <stdint.h>\n");
            if package.manifest.native.headers.is_empty() {
                anchors.push_str("#include \"native-contract.h\"\n");
            }
            for path in &package.manifest.native.headers {
                let header = spelling(&package.manifest.root.join(path))?;
                if header.contains(['"', '\\']) {
                    return Err("native provider header cannot be encoded".into());
                }
                anchors.push_str(&format!("#include \"{header}\"\n"));
            }
            let mut local = BTreeSet::new();
            for (signature, definition) in &selections {
                if definition.owner.is_none()
                    && let NativeBinding::Symbol(symbol) = &signature.binding
                    && local.insert(symbol)
                {
                    anchors.push_str(&format!(
                        "__attribute__((visibility(\"default\"))) uintptr_t {}(void) {{ return (uintptr_t)&{symbol}; }}\n",
                        address_getter(symbol)
                    ));
                }
            }
            std::fs::write(&companion, anchors).map_err(|error| error.to_string())?;
            let output = directory.join(provider_name);
            let mut command = provider_source(toolchain, package, directory, provider_name)?;
            if let Some(key) = record_provider {
                *key = record_provider_input(&mut command, toolchain, directory);
            }
            command.arg("-Wl,-t,-Bsymbolic-functions");
            for path in &self.link_paths {
                if path.extension().is_some_and(|ext| ext == "a") {
                    command
                        .arg("-Wl,--whole-archive")
                        .arg(path)
                        .arg("-Wl,--no-whole-archive");
                } else {
                    command.arg(path);
                }
            }
            let link = rinzu_compiler::timing::span("native.package.link.provider")
                .detail(&package.manifest.name);
            let linked = crate::cli_diagnostics::tool_output(
                command.arg("-o").arg(&output).output(),
                Some(&output),
                "ELF native provider linking",
                "clang",
            )?;
            drop(link);
            let trace = String::from_utf8(linked.stdout).map_err(|error| error.to_string())?;
            inputs.extend(crate::native_fingerprint::link_inputs(
                &package.manifest.root,
                &toolchain.sysroot,
                &trace,
            )?);
            self.verify_inputs()?;
            let bytes = std::fs::read(&output).map_err(|error| error.to_string())?;
            let (_, needed, _) = elf::shared(&bytes)?;
            for needed in needed {
                if images
                    .iter()
                    .any(|image| image.soname.as_deref() == Some(needed.as_str()))
                {
                    continue;
                }
                let paths: BTreeSet<_> = trace
                    .lines()
                    .map(Path::new)
                    .filter_map(|path| path.canonicalize().ok())
                    .filter(|path| path.file_name().is_some_and(|name| name == needed.as_str()))
                    .collect();
                if paths.len() != 1
                    || !paths
                        .iter()
                        .all(|path| system_image(path, &toolchain.sysroot))
                {
                    return Err(format!(
                        "ELF provider dependency `{needed}` has no unique explicit or system link input"
                    ));
                }
                let path = paths.first().unwrap();
                let image = crate::native_image::image_info(path)?;
                if image.soname.as_deref() != Some(needed.as_str()) {
                    return Err(format!(
                        "ELF provider dependency SONAME does not match `{needed}`"
                    ));
                }
                images.push(image);
            }
            let mut provider = elf::image_info(&output, &bytes)?;
            provider.path = provider_name.into();
            provider.bundled = true;
            images.push(provider);
            Some(images.len() - 1)
        } else {
            None
        };
        let mut slots = Vec::with_capacity(selections.len());
        for (signature, definition) in selections {
            let NativeBinding::Symbol(symbol) = &signature.binding else {
                unreachable!()
            };
            let image = if let Some(owner) = &definition.owner {
                images.iter().position(|image| {
                    image.build_id == owner.build_id && image.sha256 == owner.sha256
                })
            } else {
                provider
            }
            .ok_or_else(|| {
                format!(
                    "{}: missing ELF provider for `{symbol}`",
                    declaration_position(source, &signature.name)
                )
            })?;
            let owner = &images[image];
            slots.push(NativeBindingSlot {
                declaration: signature.name.clone(),
                symbol: symbol.clone(),
                resolver: definition.owner.is_none().then(|| address_getter(symbol)),
                signature: signature.canonical_type(),
                definition: definition.identity.clone(),
                image,
                owner_path: owner.path.clone(),
                owner_uuid: String::new(),
                owner_build_id: owner.build_id.clone(),
                owner_sha256: owner.sha256.clone(),
            });
        }
        let plan = NativeBindingPlan {
            version: 3,
            target: toolchain.target.triple().into(),
            images,
            slots,
        };
        plan.validate()?;
        Ok((plan, inputs))
    }
}

fn system_image(path: &Path, sysroot: &Path) -> bool {
    ["usr/lib", "usr/lib64", "lib", "lib64"]
        .iter()
        .any(|prefix| {
            path.starts_with(Path::new("/").join(prefix)) || path.starts_with(sysroot.join(prefix))
        })
}
