//! Static explicit-reference index and private exact-provider plan.
//! Tool output is data. No user image is loaded or executed while checking.
use crate::native_clang::TargetClang;
pub use crate::native_image::image_info;
use crate::native_image::{ImageProbe, inspection_arch};
use object::{Object, ObjectSection, ObjectSymbol};
use rinzu_bindgen::native_binding::{NativeBindingPlan, NativeBindingSlot, NativeProviderImage};
use rinzu_bindgen::source_package::sha256;
use rinzu_compiler::ast::{NativeBinding, NativeMember};
use rinzu_compiler::{NativeCliInputs, PackageKind, ResolvedSdkPackage, SourceNativeInterface};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug)]
struct Definition {
    identity: String,
    owner: Option<NativeProviderImage>,
    root: Option<NativeProviderImage>,
    executable: bool,
}

#[derive(Clone, Default)]
pub struct ReferenceIndex {
    inputs_only: bool,
    inspected_images: BTreeMap<String, Option<String>>,
    inspection_tool: Option<String>,
    definitions: BTreeMap<String, BTreeMap<String, Definition>>,
    pub link_paths: Vec<PathBuf>,
    pub inputs: BTreeMap<String, String>,
    pub runtime_paths: BTreeSet<PathBuf>,
    files: BTreeMap<PathBuf, (String, String)>,
}

#[derive(Clone)]
pub struct ReferenceCheck {
    package: ResolvedSdkPackage,
    application_root: PathBuf,
    cli: NativeCliInputs,
    directory: PathBuf,
    inputs: BTreeMap<String, String>,
    paths: Vec<PathBuf>,
    images: BTreeSet<PathBuf>,
}

impl ReferenceCheck {
    pub fn new(
        package: &ResolvedSdkPackage,
        application_root: &Path,
        cli: &NativeCliInputs,
        directory: &Path,
        index: &ReferenceIndex,
    ) -> Self {
        Self {
            package: ResolvedSdkPackage {
                manifest: package.manifest.clone(),
                native_imports: Vec::new(),
                source: None,
            },
            application_root: application_root.into(),
            cli: cli.clone(),
            directory: directory.into(),
            inputs: index.inputs.clone(),
            paths: index.link_paths.clone(),
            images: index
                .inspected_images
                .keys()
                .filter_map(|identity| {
                    identity
                        .rsplit_once(':')
                        .map(|(path, _)| PathBuf::from(path))
                })
                .collect(),
        }
    }
    pub fn verify_with_toolchain(&self, toolchain: &TargetClang) -> Result<(), String> {
        Self::verify_all([self], toolchain)
    }

    pub fn verify_all<'a>(
        checks: impl IntoIterator<Item = &'a Self>,
        toolchain: &TargetClang,
    ) -> Result<(), String> {
        let checks: Vec<_> = checks.into_iter().collect();
        let mut images = ImageProbe::default();
        images.prime(
            checks
                .iter()
                .flat_map(|check| check.images.iter().map(PathBuf::as_path)),
        )?;
        for check in checks {
            check.verify_with_images(toolchain, &mut images)?;
        }
        Ok(())
    }

    fn verify_with_images(
        &self,
        toolchain: &TargetClang,
        images: &mut ImageProbe,
    ) -> Result<(), String> {
        let _timing = rinzu_compiler::timing::span("native.references.verify");
        let current = ReferenceIndex::prepare_with_mode(
            &self.package,
            &self.application_root,
            &self.cli,
            &self.directory,
            toolchain,
            true,
            images,
        )?;
        if current.inputs != self.inputs || current.link_paths != self.paths {
            return Err(
                "native reference selection changed before publication; check again".into(),
            );
        }
        Ok(())
    }
}

pub(super) fn run(mut command: Command, action: &str) -> Result<String, String> {
    let _timing = rinzu_compiler::timing::span("native.command").detail(action);
    let result = command
        .output()
        .map_err(|error| format!("{action}: {error}"))?;
    if !result.status.success() {
        return Err(format!(
            "{action}: {}",
            String::from_utf8_lossy(&result.stderr)
        ));
    }
    String::from_utf8(result.stdout).map_err(|error| error.to_string())
}

// Only immutable inspection results are reused. Every caller first reads a
// fresh image UUID/content digest, and each index selects a fresh toolchain.
// Keep the cache bounded for long-lived development/editor processes.
fn inspect_image(
    command: Command,
    image: &NativeProviderImage,
    inputs_only: bool,
    tool: Option<&str>,
) -> Result<String, String> {
    let cache = inspection_cache();
    let key = tool.map(|tool| {
        serde_json::to_string(&(image, inputs_only, tool)).expect("image identity is serializable")
    });
    if let Some(key) = &key
        && let Some(output) = cache.lock().ok().and_then(|cache| cache.get(key).cloned())
    {
        let _hit = rinzu_compiler::timing::span("native.image.inspection.hit");
        return Ok(output);
    }
    let output = run(command, "inspect explicit native exports/re-exports")?;
    cache_inspection(image, inputs_only, tool, &output);
    Ok(output)
}

fn inspection_cache() -> &'static std::sync::Mutex<BTreeMap<String, String>> {
    static CACHE: std::sync::Mutex<BTreeMap<String, String>> =
        std::sync::Mutex::new(BTreeMap::new());
    &CACHE
}

fn cache_inspection(
    image: &NativeProviderImage,
    inputs_only: bool,
    tool: Option<&str>,
    output: &str,
) {
    let cache = inspection_cache();
    if let Some(tool) = tool {
        let key = serde_json::to_string(&(image, inputs_only, tool))
            .expect("image identity is serializable");
        const LIMIT: usize = 16 * 1024 * 1024;
        if output.len() <= LIMIT
            && let Ok(mut cache) = cache.lock()
        {
            let replaced = cache.get(&key).map_or(0, |value| key.len() + value.len());
            if cache
                .iter()
                .map(|(key, value)| key.len() + value.len())
                .sum::<usize>()
                - replaced
                + key.len()
                + output.len()
                > LIMIT
            {
                cache.clear();
            }
            if key.len() + output.len() <= LIMIT {
                cache.insert(key, output.to_owned());
            }
        }
    }
}

fn dependency_report(report: &str) -> String {
    let mut dependencies = String::new();
    let mut selected = false;
    for line in report.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('-') && trimmed.ends_with(':') {
            selected = trimmed == "-linked_dylibs:";
        }
        if selected {
            dependencies.push_str(line);
            dependencies.push('\n');
        }
    }
    dependencies
}

fn physical(path: &Path) -> Result<String, String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err(format!(
            "native input is not a regular file: {}",
            path.display()
        ));
    }
    Ok(format!("{}:{}", metadata.dev(), metadata.ino()))
}

fn spelling(path: &Path) -> Result<String, String> {
    let text = path.to_str().ok_or("native input path must be UTF-8")?;
    if text.contains(['\0', '\n', '\r']) {
        return Err("native input path contains a control character".into());
    }
    Ok(text.into())
}

pub fn declaration_position(source: &SourceNativeInterface, name: &str) -> String {
    let declaration = source
        .syntax
        .native_blocks
        .iter()
        .flat_map(|block| &block.members)
        .find_map(|member| match member {
            NativeMember::Function(function) if function.name == name => Some(function),
            _ => None,
        });
    declaration.map_or_else(
        || name.into(),
        |function| {
            format!(
                "{}:{}",
                source
                    .source_files
                    .get(&function.range.file_id.0)
                    .map_or("<source>", String::as_str),
                function.range.start_byte
            )
        },
    )
}

pub fn validate_signatures(source: &SourceNativeInterface) -> Result<(), String> {
    let mut symbols = BTreeMap::new();
    for signature in &source.signatures {
        let NativeBinding::Symbol(symbol) = &signature.binding else {
            continue;
        };
        if let Some(previous) = symbols.insert(symbol, signature)
            && (previous.parameters != signature.parameters || previous.result != signature.result)
        {
            return Err(format!(
                "{}: native symbol `{symbol}` has incompatible ABI declarations `{}` ({}) and `{}` ({}) at {}; target aarch64-apple-darwin",
                declaration_position(source, &previous.name),
                previous.name,
                previous.canonical_type(),
                signature.name,
                signature.canonical_type(),
                declaration_position(source, &signature.name)
            ));
        }
    }
    Ok(())
}

pub fn cli_belongs_to(
    package: &ResolvedSdkPackage,
    application_root: &Path,
) -> Result<bool, String> {
    Ok(package.manifest.kind == PackageKind::Application
        || package
            .manifest
            .root
            .canonicalize()
            .map_err(|error| error.to_string())?
            == application_root
                .canonicalize()
                .map_err(|error| error.to_string())?)
}

impl ReferenceIndex {
    pub fn for_cli(
        root: &Path,
        inputs: &NativeCliInputs,
        directory: &Path,
    ) -> Result<Self, String> {
        let package = ResolvedSdkPackage {
            manifest: rinzu_compiler::Manifest {
                root: root.into(),
                name: "cli-reference-scope".into(),
                language: "0.5".into(),
                kind: PackageKind::Application,
                entry: PathBuf::new(),
                dependencies: Vec::new(),
                exports: Default::default(),
                native: Default::default(),
            },
            native_imports: Vec::new(),
            source: None,
        };
        Self::prepare(&package, root, inputs, directory, &TargetClang::resolve()?)
    }
    fn insert(&mut self, symbol: String, definition: Definition) {
        self.definitions
            .entry(symbol)
            .or_default()
            .entry(definition.identity.clone())
            .or_insert(definition);
    }

    fn object(&mut self, bytes: &[u8], identity: &str) -> Result<(), String> {
        if self.inputs_only {
            // The full index already validated the object. Its fresh byte
            // digest remains part of the inputs compared before publication.
            return Ok(());
        }
        let file = object::File::parse(bytes)
            .map_err(|error| format!("invalid native object {identity}: {error}"))?;
        if file.format() != object::BinaryFormat::MachO
            || file.architecture() != object::Architecture::Aarch64
            || file.kind() != object::ObjectKind::Relocatable
        {
            return Err(format!(
                "unsupported native object/target {identity}: {:?}/{:?}/{:?}; expected Mach-O arm64 object for aarch64-apple-darwin",
                file.format(),
                file.architecture(),
                file.kind()
            ));
        }
        verify_mac_target(&file, identity)?;
        for symbol in file
            .symbols()
            .filter(|symbol| symbol.is_definition() && symbol.is_global())
        {
            let name = symbol.name().map_err(|error| error.to_string())?;
            let Some(name) = name.strip_prefix('_') else {
                continue;
            };
            let executable = symbol
                .section_index()
                .and_then(|index| file.section_by_index(index).ok())
                .is_some_and(|section| section.kind() == object::SectionKind::Text);
            self.insert(
                name.into(),
                Definition {
                    identity: format!("{identity}:symbol:{name}:aarch64-apple-darwin"),
                    owner: None,
                    root: None,
                    executable,
                },
            );
        }
        Ok(())
    }

    fn add_path(
        &mut self,
        supplied: &Path,
        label: &str,
        seen: &mut BTreeSet<String>,
        sysroot: &Path,
        images: &mut ImageProbe,
    ) -> Result<(), String> {
        let path = supplied
            .canonicalize()
            .map_err(|error| format!("native input {}: {error}", supplied.display()))?;
        let physical = physical(&path)?;
        if !seen.insert(physical.clone()) {
            return Ok(());
        }
        let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
        let digest = sha256(&bytes);
        self.files
            .insert(path.clone(), (physical.clone(), digest.clone()));
        let input_identity = if label.starts_with("compiled:") {
            format!("{label}:{digest}")
        } else {
            format!("{}:{digest}", spelling(&path)?)
        };
        self.inputs
            .insert(format!("native.reference:{label}"), input_identity.clone());
        self.link_paths.push(path.clone());
        if bytes.starts_with(b"!<arch>\n") {
            let archive = object::read::archive::ArchiveFile::parse(bytes.as_slice())
                .map_err(|error| error.to_string())?;
            for member in archive.members() {
                let member = member.map_err(|error| error.to_string())?;
                let name = std::str::from_utf8(member.name()).map_err(|error| error.to_string())?;
                let data = member
                    .data(bytes.as_slice())
                    .map_err(|error| error.to_string())?;
                let (offset, _) = member.file_range();
                self.object(
                    data,
                    &format!("archive:{}:{digest}:member:{offset}:{name}", path.display()),
                )?;
            }
        } else if path.extension().is_some_and(|extension| extension == "tbd") {
            let runtime = stub_runtime(&path, sysroot)?;
            let root = images.get(&runtime)?;
            self.shared(&root, &root, &mut BTreeSet::new(), images)?;
        } else {
            let file = object::File::parse(bytes.as_slice())
                .map_err(|error| format!("unsupported native input {}: {error}", path.display()))?;
            if file.format() != object::BinaryFormat::MachO
                || file.architecture() != object::Architecture::Aarch64
            {
                return Err(format!(
                    "unsupported native target {}: {:?}/{:?}; expected Mach-O arm64 for aarch64-apple-darwin",
                    path.display(),
                    file.format(),
                    file.architecture()
                ));
            }
            verify_mac_target(&file, &path.display().to_string())?;
            match file.kind() {
                object::ObjectKind::Relocatable => {
                    self.object(&bytes, &format!("object:{input_identity}"))?
                }
                object::ObjectKind::Dynamic => {
                    self.runtime_paths.insert(path.parent().unwrap().into());
                    let root = images.get(&path)?;
                    self.shared(&root, &root, &mut BTreeSet::new(), images)?;
                }
                kind => {
                    return Err(format!(
                        "unsupported native input kind {kind:?}: {}",
                        path.display()
                    ));
                }
            }
        }
        Ok(())
    }

    fn shared(
        &mut self,
        image: &NativeProviderImage,
        root: &NativeProviderImage,
        visited: &mut BTreeSet<String>,
        images: &mut ImageProbe,
    ) -> Result<(), String> {
        let identity = format!("{}:{}", image.path, image.uuid);
        if !visited.insert(identity.clone()) {
            return Ok(());
        }
        if let Some(digest) = self.inspected_images.get(&identity) {
            if digest != &image.sha256 {
                return Err("native image changed during reference inspection; check again".into());
            }
            // Framework re-export graphs overlap. Definitions are keyed by
            // physical image/export identity and already keep the first root,
            // so walking that image again cannot add a distinct candidate.
            return Ok(());
        }
        self.inspected_images
            .insert(identity.clone(), image.sha256.clone());
        let path = Path::new(&image.path);
        let mut command = Command::new("xcrun");
        command.args(["dyld_info", "-arch", inspection_arch(path), "-dependents"]);
        if !self.inputs_only {
            command.args(["-exports", "-segments", "-function_starts"]);
        }
        command.arg(path);
        let output = if !self.inputs_only
            && let Some(report) = images.report(image)
        {
            let _hit = rinzu_compiler::timing::span("native.image.inspection.batch.hit");
            report.to_owned()
        } else {
            inspect_image(
                command,
                image,
                self.inputs_only,
                self.inspection_tool.as_deref(),
            )?
        };
        if !self.inputs_only {
            // A current full inspection proves the dependency graph as well.
            // Keep only that slim section for later fresh-UUID verification.
            cache_inspection(
                image,
                true,
                self.inspection_tool.as_deref(),
                &dependency_report(&output),
            );
        }
        let mut section = "";
        let mut exports = Vec::new();
        let mut starts = BTreeSet::new();
        let mut base = None;
        let mut reexports = Vec::new();
        for line in output.lines().map(str::trim) {
            if line.starts_with('-') && line.ends_with(':') {
                section = line;
                continue;
            }
            let fields: Vec<_> = line.split_whitespace().collect();
            if section == "-linked_dylibs:" && fields.first() == Some(&"re-export") {
                let dependent = line.strip_prefix("re-export").unwrap().trim();
                reexports.push(resolve_reexport(path, dependent)?);
            } else if section == "-segments:" && fields.get(1) == Some(&"__TEXT") {
                base = fields.first().and_then(|value| number(value));
            } else if section == "-function_starts:"
                && let Some(address) = fields.first().and_then(|value| number(value))
            {
                starts.insert(address);
            } else if section == "-exports:"
                && fields.first().is_some_and(|value| value.starts_with("0x"))
                && fields.len() >= 2
            {
                // Re-export aliases and resolver/stub records require a distinct proof.
                // Accept ordinary address exports (including weak annotations) only.
                let address = number(fields[0]).ok_or("invalid export offset")?;
                let Some(symbol) = fields[1].strip_prefix('_') else {
                    continue;
                };
                let ordinary = fields[2..]
                    .iter()
                    .all(|field| matches!(*field, "[weak-def]" | "[weak]" | "(weak-def)"));
                exports.push((symbol.to_owned(), address, ordinary));
            }
        }
        let base = if self.inputs_only {
            0
        } else {
            base.ok_or_else(|| format!("native image has no readable __TEXT: {}", image.path))?
        };
        self.inputs.insert(
            format!("native.image:{identity}"),
            image
                .sha256
                .clone()
                .unwrap_or_else(|| "system-shared-cache".into()),
        );
        if let Some(digest) = &image.sha256 {
            self.files
                .insert(path.into(), (physical(path)?, digest.clone()));
        }
        for (symbol, offset, ordinary) in exports {
            self.insert(
                symbol.clone(),
                Definition {
                    identity: format!("shared:{identity}:export:{symbol}:aarch64-apple-darwin"),
                    owner: Some(image.clone()),
                    root: Some(root.clone()),
                    executable: ordinary
                        && base
                            .checked_add(offset)
                            .is_some_and(|address| starts.contains(&address)),
                },
            );
        }
        for dependent in reexports {
            let dependent = images.get(&dependent)?;
            self.shared(&dependent, root, visited, images)?;
        }
        Ok(())
    }

    pub fn prepare(
        package: &ResolvedSdkPackage,
        application_root: &Path,
        inputs: &NativeCliInputs,
        directory: &Path,
        toolchain: &TargetClang,
    ) -> Result<Self, String> {
        // Cached paths are advisory query hints, never reference/definition
        // inputs. Query their CURRENT UUIDs together; selection and re-export
        // discovery still walk the current package's explicit native scope.
        let hints = serde_json::to_vec(&("rinzu.native-image-hints-v1", &toolchain.fingerprint))
            .ok()
            .and_then(|key| {
                crate::source_package_cache::directory(
                    &package.manifest.root,
                    application_root,
                    &key,
                )
                .ok()
            });
        let mut images = ImageProbe::default();
        if let Some((cache, key)) = &hints {
            let path = cache.join("image-paths.json");
            if std::fs::symlink_metadata(&path)
                .is_ok_and(|m| m.file_type().is_file() && m.len() <= 128 * 1024)
                && let Some(files) = crate::source_package_cache::completed(
                    cache,
                    key,
                    &BTreeSet::from(["image-paths.json".into()]),
                )
                && let Ok(paths) =
                    serde_json::from_slice::<Vec<PathBuf>>(&files["image-paths.json"])
                && !paths.is_empty()
                && paths.len() <= 256
            {
                if images
                    .prime_full(paths.iter().map(PathBuf::as_path))
                    .is_ok()
                {
                    let _hit = rinzu_compiler::timing::span("native.image.hints.hit")
                        .detail(&package.manifest.name);
                } else {
                    images = ImageProbe::default();
                }
            }
        }
        let index = Self::prepare_with_mode(
            package,
            application_root,
            inputs,
            directory,
            toolchain,
            false,
            &mut images,
        )?;
        if let Some((cache, key)) = hints {
            let paths: Vec<_> = index
                .inspected_images
                .keys()
                .filter_map(|identity| {
                    identity
                        .rsplit_once(':')
                        .map(|(path, _)| PathBuf::from(path))
                })
                .filter(|path| {
                    (path.starts_with("/usr/lib") || path.starts_with("/System/Library"))
                        && !path.is_file()
                })
                .collect();
            if paths.len() <= 256
                && let Ok(bytes) = serde_json::to_vec(&paths)
            {
                // Atomic data/receipt publication; invalid/stale hints merely
                // lose the batching optimization on the next preparation.
                let _ = crate::source_package_cache::store(
                    &cache,
                    &key,
                    &BTreeMap::from([("image-paths.json".into(), bytes)]),
                    None,
                );
            }
        }
        Ok(index)
    }

    fn prepare_with_mode(
        package: &ResolvedSdkPackage,
        application_root: &Path,
        inputs: &NativeCliInputs,
        directory: &Path,
        toolchain: &TargetClang,
        inputs_only: bool,
        images: &mut ImageProbe,
    ) -> Result<Self, String> {
        // Revalidation needs the selected paths, image UUIDs, content digests
        // and re-export graph. Rebuilding every export/function index adds no
        // evidence once those identities are unchanged.
        let mut index = Self {
            inputs_only,
            inspection_tool: Some(sha256(
                &serde_json::to_vec(&toolchain.fingerprint).map_err(|error| error.to_string())?,
            )),
            ..Self::default()
        };
        let mut seen = BTreeSet::new();
        for position in 0..package.manifest.native.sources.len() {
            index.add_path(
                &directory.join(format!("source-{position}.o")),
                &format!("compiled:{position}"),
                &mut seen,
                &toolchain.sysroot,
                images,
            )?;
        }
        for path in &package.manifest.native.objects {
            index.add_path(
                &package.manifest.root.join(path),
                &format!("owner-object:{}", path.display()),
                &mut seen,
                &toolchain.sysroot,
                images,
            )?;
        }
        // CLI inputs belong to the application owner; an imported SDK has its own scope.
        let empty = NativeCliInputs::default();
        let inputs = if cli_belongs_to(package, application_root)? {
            inputs
        } else {
            &empty
        };
        for path in &inputs.objects {
            index.add_path(
                &application_root.join(path),
                &format!("cli-object:{}", path.display()),
                &mut seen,
                &toolchain.sysroot,
                images,
            )?;
        }
        let mut searches = package
            .manifest
            .native
            .search_paths
            .iter()
            .map(|path| package.manifest.root.join(path))
            .collect::<Vec<_>>();
        searches.extend(
            inputs
                .search_paths
                .iter()
                .map(|path| application_root.join(path)),
        );
        let mut libraries = package.manifest.native.libraries.clone();
        libraries.extend(inputs.libraries.iter().cloned());
        let mut frameworks = package.manifest.native.frameworks.clone();
        let mut framework_searches = Vec::new();
        for tool in &package.manifest.native.config_tools {
            for pair in crate::native_config::resolve(tool)?.flags.chunks_exact(2) {
                match pair[0].as_str() {
                    "-L" => searches.push(pair[1].clone().into()),
                    "-l" => libraries.push(pair[1].clone()),
                    "-F" => framework_searches.push(pair[1].clone()),
                    "-framework" => frameworks.push(pair[1].clone()),
                    _ => (),
                }
            }
        }
        std::fs::write(
            directory.join("native-selection.c"),
            "/* static linker selection only */\n",
        )
        .map_err(|error| error.to_string())?;
        let selections: Vec<_> = libraries
            .iter()
            .map(|name| ("library", name))
            .chain(frameworks.iter().map(|name| ("framework", name)))
            .collect();
        if !selections.is_empty() {
            let mut command = toolchain.command();
            command
                .args(["-dynamiclib", "-Wl,-t"])
                .arg(directory.join("native-selection.c"));
            for search in &searches {
                command.arg("-L").arg(search);
            }
            for search in &framework_searches {
                command.arg("-F").arg(search);
            }
            for (kind, name) in &selections {
                if *kind == "library" {
                    command.arg(format!("-l{name}"));
                } else {
                    command.arg("-framework").arg(name);
                }
            }
            command
                .arg("-o")
                .arg(directory.join("native-selection.dylib"));
            // All selections use the same search paths and target. A single
            // linker trace still proves each explicit name unambiguously.
            let trace = run(
                command,
                "resolve explicit native libraries/frameworks for aarch64-apple-darwin",
            )?;
            for (kind, name) in selections {
                let selected: BTreeSet<_> = trace
                    .lines()
                    .filter_map(|line| {
                        let path = line.rfind(".a(").map_or(line, |offset| &line[..offset + 2]);
                        let path = PathBuf::from(path);
                        let stem = path.file_stem()?.to_str()?;
                        let matches = if kind == "library" {
                            stem == format!("lib{name}")
                        } else {
                            stem == name && path.to_str()?.contains(&format!("/{name}.framework/"))
                        };
                        matches.then_some(path)
                    })
                    .collect();
                if selected.len() != 1 {
                    return Err(format!(
                        "native {kind} `{name}` has {} unambiguous linker-selected inputs; target aarch64-apple-darwin",
                        selected.len()
                    ));
                }
                let selected = selected.into_iter().next().unwrap();
                index.add_path(
                    &selected,
                    &format!("{kind}:{name}"),
                    &mut seen,
                    &toolchain.sysroot,
                    images,
                )?;
            }
        }
        let trace = index
            .link_paths
            .iter()
            .filter(|path| !path.extension().is_some_and(|extension| extension == "o"))
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        index.inputs.extend(crate::native_fingerprint::link_inputs(
            &package.manifest.root,
            &toolchain.sysroot,
            &trace,
        )?);
        let generated_root = directory
            .canonicalize()
            .map_err(|error| error.to_string())?;
        for (path, (_, digest)) in &index.files {
            if !path.starts_with(&generated_root) {
                index
                    .inputs
                    .insert(format!("native.file:{}", path.display()), digest.clone());
            }
        }
        Ok(index)
    }

    pub fn bind(
        self,
        source: &SourceNativeInterface,
        directory: &Path,
        toolchain: &TargetClang,
        provider_name: &str,
        package: &ResolvedSdkPackage,
    ) -> Result<(NativeBindingPlan, BTreeMap<String, String>), String> {
        let mut selections = Vec::new();
        for signature in &source.signatures {
            let NativeBinding::Symbol(symbol) = &signature.binding else {
                continue;
            };
            let candidates = self.definitions.get(symbol);
            let count = candidates.map_or(0, BTreeMap::len);
            if count != 1 {
                let diagnostic = if count == 0 {
                    "unresolved"
                } else {
                    "duplicate definitions"
                };
                return Err(format!(
                    "{}: {diagnostic} native symbol `{symbol}` for declaration `{}`; {count} definitions; explicit candidates [{}]; inputs [{}]; target aarch64-apple-darwin",
                    declaration_position(source, &signature.name),
                    signature.name,
                    candidates
                        .into_iter()
                        .flat_map(|values| values.keys())
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", "),
                    self.link_paths
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            let definition = candidates.unwrap().values().next().unwrap();
            if !definition.executable {
                return Err(format!(
                    "{}: native symbol `{symbol}` is not a verified executable function: {}; target aarch64-apple-darwin",
                    declaration_position(source, &signature.name),
                    definition.identity
                ));
            }
            selections.push((signature, definition));
        }
        self.verify_inputs()?;
        let mut inputs = self.inputs.clone();
        let local = selections
            .iter()
            .any(|(_, definition)| definition.owner.is_none());
        // Validate all supplied objects and their implementation dependencies even if unused.
        let has_objects = self.link_paths.iter().any(|path| {
            path.extension()
                .is_some_and(|extension| extension == "o" || extension == "a")
        });
        let provider = if local || has_objects {
            let output = directory.join(provider_name);
            let mut anchors = String::from("#include <stdint.h>\n");
            let headers = &package.manifest.native.headers;
            if headers.is_empty() {
                anchors.push_str("#include \"native-contract.h\"\n");
            }
            for header in headers {
                let header = spelling(&package.manifest.root.join(header))?;
                if header.contains(['"', '\\', '\n', '\r']) {
                    return Err("native provider header cannot be encoded".into());
                }
                anchors.push_str(&format!("#include \"{header}\"\n"));
            }
            let mut names = BTreeSet::new();
            for (signature, definition) in &selections {
                if definition.owner.is_none() {
                    let NativeBinding::Symbol(symbol) = &signature.binding else {
                        unreachable!()
                    };
                    if names.insert(symbol) {
                        anchors.push_str(&format!(
                            "uintptr_t {}(void) {{ return (uintptr_t)&{symbol}; }}\n",
                            address_getter(symbol)
                        ));
                    }
                }
            }
            std::fs::write(directory.join("native-addresses.c"), anchors)
                .map_err(|error| error.to_string())?;
            let mut command = toolchain.command();
            command
                .args([
                    "-dynamiclib",
                    "-fno-builtin",
                    "-fno-lto",
                    "-Wl,-t",
                    "-Wl,-fatal_warnings",
                ])
                .arg("-install_name")
                .arg(format!("@rpath/{provider_name}"));
            command
                .arg("-I")
                .arg(directory)
                .arg(directory.join("native-addresses.c"));
            crate::source_native_package::add_package_flags(&mut command, package, false)?;
            for path in &self.link_paths {
                if path.extension().is_some_and(|extension| extension == "a") {
                    command
                        .args(["-Xlinker", "-force_load", "-Xlinker"])
                        .arg(path);
                } else {
                    command.arg(path);
                }
            }
            for path in &self.runtime_paths {
                command.args(["-Xlinker", "-rpath", "-Xlinker"]).arg(path);
            }
            command.arg("-o").arg(&output);
            let trace = run(
                command,
                "link exact native provider (including implementation dependencies)",
            )?;
            self.verify_inputs()?;
            inputs.extend(crate::native_fingerprint::link_inputs(
                directory,
                &toolchain.sysroot,
                &trace,
            )?);
            let mut provider = image_info(&output)?;
            let mut exported = Self {
                inspection_tool: self.inspection_tool.clone(),
                ..Self::default()
            };
            exported.shared(
                &provider,
                &provider,
                &mut BTreeSet::new(),
                &mut ImageProbe::default(),
            )?;
            for (signature, definition) in &selections {
                if definition.owner.is_none() {
                    let NativeBinding::Symbol(symbol) = &signature.binding else {
                        unreachable!()
                    };
                    if !exported
                        .definitions
                        .get(&address_getter(symbol))
                        .is_some_and(|definitions| {
                            definitions.len() == 1
                                && definitions.values().all(|definition| definition.executable)
                        })
                    {
                        return Err(format!(
                            "{}: selected native object definition `{symbol}` has no verified provider-owned address getter; target aarch64-apple-darwin",
                            declaration_position(source, &signature.name)
                        ));
                    }
                }
            }
            provider.path = provider_name.into();
            provider.bundled = true;
            Some(provider)
        } else {
            self.validate_link(directory)?;
            None
        };
        let mut plan = NativeBindingPlan {
            version: 1,
            target: "aarch64-apple-darwin".into(),
            images: Vec::new(),
            slots: Vec::new(),
        };
        for (signature, definition) in selections {
            let NativeBinding::Symbol(symbol) = &signature.binding else {
                unreachable!()
            };
            let root = definition
                .root
                .as_ref()
                .or(provider.as_ref())
                .ok_or("native provider was not linked")?;
            let owner = definition
                .owner
                .as_ref()
                .or(provider.as_ref())
                .ok_or("native definition has no owner")?;
            let image = plan
                .images
                .iter()
                .position(|image| image == root)
                .unwrap_or_else(|| {
                    plan.images.push(root.clone());
                    plan.images.len() - 1
                });
            plan.slots.push(NativeBindingSlot {
                declaration: signature.name.clone(),
                symbol: symbol.clone(),
                resolver: definition.owner.is_none().then(|| address_getter(symbol)),
                signature: signature.canonical_type(),
                definition: definition.identity.clone(),
                image,
                owner_path: owner.path.clone(),
                owner_uuid: owner.uuid.clone(),
                owner_sha256: owner.sha256.clone(),
            });
        }
        plan.validate()?;
        Ok((plan, inputs))
    }

    fn verify_inputs(&self) -> Result<(), String> {
        for (path, (identity, digest)) in &self.files {
            if physical(path)? != *identity
                || sha256(&std::fs::read(path).map_err(|error| error.to_string())?) != *digest
            {
                return Err(format!(
                    "native input changed after indexing: {}",
                    path.display()
                ));
            }
        }
        Ok(())
    }

    pub fn validate_link(&self, directory: &Path) -> Result<(), String> {
        self.verify_inputs()?;
        let toolchain = TargetClang::resolve()?;
        let mut command = toolchain.command();
        command
            .args(["-dynamiclib", "-Wl,-fatal_warnings"])
            .arg(directory.join("native-selection.c"));
        for path in &self.link_paths {
            if path.extension().is_some_and(|extension| extension == "a") {
                command
                    .args(["-Xlinker", "-force_load", "-Xlinker"])
                    .arg(path);
            } else {
                command.arg(path);
            }
        }
        for path in &self.runtime_paths {
            command.args(["-Xlinker", "-rpath", "-Xlinker"]).arg(path);
        }
        command
            .arg("-o")
            .arg(directory.join("explicit-inputs.dylib"));
        run(
            command,
            "validate explicit native implementation dependencies",
        )?;
        self.verify_inputs()
    }
}

/// The private initializer validates the entire immutable transaction first.
/// Re-loading an already pinned facade may confirm identical pointers; it cannot rebind.
pub fn initializer(plan: &NativeBindingPlan, digest: &str) -> String {
    let mut source = format!(
        "static pthread_mutex_t __rinzu_native_bind_lock = PTHREAD_MUTEX_INITIALIZER;\nstatic int __rinzu_native_bound;\nint {}(const char *digest, const uintptr_t *addresses, size_t count) {{\nif (!digest || strcmp(digest, \"{digest}\") || !addresses || count != {}) return 0;\nfor (size_t i = 0; i < count; ++i) if (!addresses[i]) return 0;\nif (pthread_mutex_lock(&__rinzu_native_bind_lock)) return 0;\nif (__rinzu_native_bound) {{\nint same = 1;\n",
        rinzu_bindgen::native_binding::INITIALIZER,
        plan.slots.len()
    );
    for index in 0..plan.slots.len() {
        source.push_str(&format!(
            "same = same && (uintptr_t)__rinzu_native_slot_{index} == addresses[{index}];\n"
        ));
    }
    source.push_str("pthread_mutex_unlock(&__rinzu_native_bind_lock); return same; }\n");
    for index in 0..plan.slots.len() {
        source.push_str(&format!("__rinzu_native_slot_{index} = (__typeof__(__rinzu_native_slot_{index}))addresses[{index}];\n"));
    }
    source.push_str(
        "__rinzu_native_bound = 1; pthread_mutex_unlock(&__rinzu_native_bind_lock); return 1;\n}\n",
    );
    source
}

fn number(text: &str) -> Option<u64> {
    u64::from_str_radix(text.strip_prefix("0x")?, 16).ok()
}

fn address_getter(symbol: &str) -> String {
    format!(
        "__rinzu_native_address_{}",
        &sha256(symbol.as_bytes())[..24]
    )
}

fn verify_mac_target(file: &object::File<'_>, identity: &str) -> Result<(), String> {
    let object::File::MachO64(macho) = file else {
        return Err("native target requires Mach-O 64".into());
    };
    if let Some((version, _)) = macho.build_version().map_err(|error| error.to_string())? {
        if version.platform.get(macho.endian()) == object::macho::PLATFORM_MACOS {
            return Ok(());
        }
    } else {
        let mut commands = macho
            .macho_load_commands()
            .map_err(|error| error.to_string())?;
        while let Some(command) = commands.next().map_err(|error| error.to_string())? {
            if command.cmd() == object::macho::LC_VERSION_MIN_MACOSX {
                return Ok(());
            }
        }
    }
    Err(format!(
        "native target mismatch or absent macOS platform: {identity}; expected aarch64-apple-darwin"
    ))
}

fn resolve_reexport(root: &Path, name: &str) -> Result<PathBuf, String> {
    let path = if let Some(relative) = name.strip_prefix("@loader_path/") {
        root.parent()
            .ok_or("native image has no parent")?
            .join(relative)
    } else {
        PathBuf::from(name)
    };
    if !path.is_absolute() {
        return Err(format!(
            "unresolved re-export path `{name}` from {}; @rpath fallback is not verified",
            root.display()
        ));
    }
    Ok(path.canonicalize().unwrap_or(path))
}

fn stub_runtime(path: &Path, sysroot: &Path) -> Result<PathBuf, String> {
    // Only the selected Apple SDK stub may name an OS shared-cache image.
    if !path.starts_with(sysroot) {
        return Err(format!(
            "unsupported non-SDK native stub {}",
            path.display()
        ));
    }
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let root_document = text.split("\n---").next().unwrap_or(&text);
    let names: BTreeSet<_> = root_document
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("install-name:")
                .map(|name| name.trim().trim_matches(['\'', '"']).to_owned())
        })
        .collect();
    if names.len() != 1 {
        return Err("native SDK stub has no unambiguous install-name (TAPI YAML required)".into());
    }
    let name = names.into_iter().next().unwrap();
    if !name.starts_with("/usr/lib/") && !name.starts_with("/System/Library/") {
        return Err("native SDK stub names a non-system image".into());
    }
    Ok(name.into())
}

#[cfg(test)]
mod report_tests {
    #[test]
    fn full_inspection_dependencies_keep_reexports_and_exclude_other_sections() {
        let full = "/usr/lib/one [arm64e]:\n-uuid:\n01234567-89AB-CDEF-0123-456789ABCDEF\n-linked_dylibs:\n  re-export /usr/lib/two\n  regular /usr/lib/three\n-exports:\n  0x1234 _one\n-function_starts:\n  0x1234\n";
        assert_eq!(
            super::dependency_report(full),
            "-linked_dylibs:\n  re-export /usr/lib/two\n  regular /usr/lib/three\n"
        );
    }
}
