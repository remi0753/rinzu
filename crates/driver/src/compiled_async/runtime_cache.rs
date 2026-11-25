//! Reuse the signed image after proving the inputs to the Darwin link.
use crate::native_clang::TargetClang;
use crate::native_package::BuiltNativePackages;
use crate::platform::macos::{closed_object, verify_code_signatures};
use crate::source_package_cache;
use rinzu_abi::target::Target;
use rinzu_bindgen::source_package::sha256;
use rinzu_compiler::timing;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const IMAGE: &str = "compiled-async.dylib";
const INPUTS: &str = "link-inputs.json";

const EXACT_SOURCE: &str = "runtime-sources.json";
const EXACT_INPUTS: &str = "runtime-inputs.json";
const EXACT_NAMESPACE: &str = "jit-run-runtime-v2";

pub(super) struct Pending {
    linked: Option<Entry>,
    exact: Option<Entry>,
}

impl Pending {
    pub(super) fn store(self) {
        if let Some(linked) = self.linked {
            linked.store("jit.runtime.cache.store");
        }
        if let Some(exact) = self.exact {
            exact.store("jit.runtime.exact.store");
        }
    }
}

struct Entry {
    directory: PathBuf,
    key: String,
    files: BTreeMap<String, Vec<u8>>,
}

impl Entry {
    fn store(self, phase: &'static str) {
        let mut event = timing::span(phase);
        event.set_counter(
            "bytes",
            self.files.values().map(|bytes| bytes.len() as u64).sum(),
        );
        let stored = source_package_cache::store(&self.directory, &self.key, &self.files, None);
        event.set_counter("stored", u64::from(stored.is_ok()));
    }
}

struct Cache {
    directory: PathBuf,
    key: String,
    search_roots: BTreeMap<PathBuf, Option<String>>,
    wire: Vec<u8>,
    plan: Vec<String>,
}

pub(super) struct Exact {
    directory: PathBuf,
    key: String,
}

pub(super) struct ExactCapture {
    entry: Exact,
    source: BTreeMap<String, String>,
    units: Vec<String>,
}

pub(super) struct RestoreInputs<'a> {
    pub(super) command: &'a Command,
    pub(super) library: &'a Path,
    pub(super) objects: &'a [PathBuf],
    pub(super) source: &'a Path,
    pub(super) resources: &'a Path,
    pub(super) toolchain: &'a TargetClang,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct ExactInputs {
    linked_key: Vec<u8>,
    plan: Vec<String>,
    units: Vec<String>,
}

impl Exact {
    pub(super) fn open(
        run: impl serde::Serialize,
        built: &BuiltNativePackages,
        resources: &Path,
        toolchain: &TargetClang,
    ) -> Option<Self> {
        if !eligible(built, toolchain) {
            return None;
        }
        let _timing = timing::span("jit.runtime.exact.key");
        let wire = serde_json::to_vec(&(
            run,
            rinzu_codegen::BASELINE_RUNTIME_ABI_REVISION,
            &toolchain.fingerprint,
            crate::compile_cache::environment()
                .into_iter()
                .collect::<Vec<_>>(),
            std::env::current_dir().ok()?,
            sdk_identities(built),
            search_roots(&toolchain.sysroot)?,
        ))
        .ok()?;
        let (directory, key) =
            source_package_cache::user_directory(resources, EXACT_NAMESPACE, &wire).ok()?;
        Some(Self { directory, key })
    }

    pub(super) fn restore(
        &self,
        inputs: RestoreInputs<'_>,
        unit_keys: impl FnOnce() -> Option<Vec<String>>,
    ) -> bool {
        let restored = self.try_restore(inputs, unit_keys).is_some();
        let _timing = timing::span(if restored {
            "jit.runtime.exact.hit"
        } else {
            "jit.runtime.exact.miss"
        });
        restored
    }

    fn try_restore(
        &self,
        inputs: RestoreInputs<'_>,
        unit_keys: impl FnOnce() -> Option<Vec<String>>,
    ) -> Option<()> {
        let RestoreInputs {
            command,
            library,
            objects,
            source,
            resources,
            toolchain,
        } = inputs;
        let mut timing = timing::span("jit.runtime.exact.lookup");
        let files = source_package_cache::completed(
            &self.directory,
            &self.key,
            &BTreeSet::from([EXACT_INPUTS.to_owned(), EXACT_SOURCE.to_owned()]),
        )?;
        let inputs: ExactInputs = serde_json::from_slice(&files[EXACT_INPUTS]).ok()?;
        timing.set_counter("bytes", files.values().map(|file| file.len() as u64).sum());
        drop(timing);
        let _verify = timing::span("jit.runtime.exact.verify");
        if inputs.units.len() != objects.len() {
            return None;
        }
        let saved: BTreeMap<String, String> = serde_json::from_slice(&files[EXACT_SOURCE]).ok()?;
        let names = BTreeSet::from([
            super::source::COMMON.to_owned(),
            super::source::PROGRAM.to_owned(),
            super::source::HEADER.to_owned(),
        ]);
        if saved.keys().cloned().collect::<BTreeSet<_>>() != names {
            return None;
        }
        let directory = source.parent()?;
        for (name, contents) in saved {
            std::fs::write(directory.join(name), contents).ok()?;
        }
        if unit_keys()? != inputs.units {
            return None;
        }
        // Clang checks the existence of object arguments even in print-only
        // mode. These files are never passed to a real linker or a loader.
        let plan = {
            let mut placeholders = ObjectPlaceholders(Vec::new());
            for object in objects {
                std::fs::File::create_new(object).ok()?;
                placeholders.0.push(object.clone());
            }
            linker_plan(command, library, objects, toolchain)
        }?;
        if plan != inputs.plan {
            return None;
        }

        let (directory, key) =
            source_package_cache::runtime_directory(resources, &inputs.linked_key).ok()?;
        restore_linked(&directory, &key, library).then_some(())
    }

    pub(super) fn capture(
        self,
        source: BTreeMap<String, String>,
        units: Vec<Option<String>>,
    ) -> Option<ExactCapture> {
        Some(ExactCapture {
            entry: self,
            source,
            units: units.into_iter().collect::<Option<_>>()?,
        })
    }
}

struct ObjectPlaceholders(Vec<PathBuf>);

impl Drop for ObjectPlaceholders {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl ExactCapture {
    fn prepare_store(self, link: &Cache) -> Option<Entry> {
        let inputs = ExactInputs {
            linked_key: link.wire.clone(),
            plan: link.plan.clone(),
            units: self.units,
        };
        Some(Entry {
            directory: self.entry.directory,
            key: self.entry.key,
            files: BTreeMap::from([
                (EXACT_INPUTS.into(), serde_json::to_vec(&inputs).ok()?),
                (EXACT_SOURCE.into(), serde_json::to_vec(&self.source).ok()?),
            ]),
        })
    }
}

fn eligible(built: &BuiltNativePackages, toolchain: &TargetClang) -> bool {
    toolchain.target == Target::Aarch64AppleDarwin
        && built.artifacts.is_empty()
        && !std::env::vars_os().any(|(name, _)| {
            let name = name.as_encoded_bytes();
            name.starts_with(b"DYLD_") || name.starts_with(b"LD_")
        })
}

fn sdk_identities(built: &BuiltNativePackages) -> Vec<(String, &Option<String>)> {
    built
        .source_packages
        .iter()
        .map(|package| {
            (
                sha256(&package.artifact.expected_wire),
                &package.artifact.facade_digest,
            )
        })
        .collect()
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct LinkInputs {
    files: BTreeMap<PathBuf, (PathBuf, String)>,
    missing: BTreeSet<PathBuf>,
}

impl LinkInputs {
    fn capture(data: &[u8], objects: &[PathBuf], library: &Path) -> Option<Self> {
        // Apple's dependency-info stream is an opcode followed by a NUL-terminated value.
        let mut data = data;
        let mut found = BTreeSet::new();
        let mut missing = BTreeSet::new();
        let mut output_seen = false;
        while let Some((&code, rest)) = data.split_first() {
            let end = rest.iter().position(|&byte| byte == 0)?;
            let value = std::str::from_utf8(&rest[..end]).ok()?;
            data = &rest[end + 1..];
            let path = Path::new(value);
            match code {
                0 if !value.is_empty() => {}
                0x40 if value.starts_with("(#)PROGRAM:ld ") => {}
                0x40 if path == library => output_seen = true,
                0x10 if path.is_absolute() => {
                    found.insert(path.to_owned());
                }
                0x11 if path.is_absolute() => {
                    missing.insert(path.to_owned());
                }
                _ => return None,
            }
        }
        if !output_seen || !objects.iter().all(|object| found.remove(object)) || found.is_empty() {
            return None;
        }
        let files = found
            .into_iter()
            .map(|path| {
                let identity = file_identity(&path)?;
                Some((path, identity))
            })
            .collect::<Option<_>>()?;
        let inputs = Self { files, missing };
        inputs.missing_is_current().then_some(inputs)
    }

    fn missing_is_current(&self) -> bool {
        self.missing
            .iter()
            .all(|path| matches!(path.try_exists(), Ok(false)))
    }

    fn is_current(&self) -> bool {
        self.missing_is_current()
            && self
                .files
                .iter()
                .all(|(path, identity)| file_identity(path).as_ref() == Some(identity))
    }
}

fn file_identity(path: &Path) -> Option<(PathBuf, String)> {
    let resolved = path.canonicalize().ok()?;
    let bytes = std::fs::read(&resolved).ok()?;
    Some((resolved, sha256(&bytes)))
}

impl Cache {
    fn for_command(
        command: &Command,
        library: &Path,
        objects: &[PathBuf],
        built: &BuiltNativePackages,
        resources: &Path,
        toolchain: &TargetClang,
    ) -> Option<Self> {
        if !eligible(built, toolchain) {
            return None;
        }
        let _timing = timing::span("jit.runtime.cache.key");
        let plan = linker_plan(command, library, objects, toolchain)?;
        let search_roots = search_roots(&toolchain.sysroot)?;
        let objects: Vec<_> = objects
            .iter()
            .map(|path| {
                let bytes = std::fs::read(path).ok()?;
                closed_object(&bytes).then(|| sha256(&bytes))
            })
            .collect::<Option<_>>()?;
        let wire = serde_json::to_vec(&(
            "rinzu.linked-jit-runtime-v1",
            rinzu_codegen::BASELINE_RUNTIME_ABI_REVISION,
            &toolchain.fingerprint,
            crate::compile_cache::environment()
                .into_iter()
                .collect::<Vec<_>>(),
            std::env::current_dir().ok()?,
            &plan,
            objects,
            sdk_identities(built),
            &search_roots,
        ))
        .ok()?;
        let (directory, key) = source_package_cache::runtime_directory(resources, &wire).ok()?;
        Some(Self {
            directory,
            key,
            search_roots,
            wire,
            plan,
        })
    }

    fn prepare_store(
        &self,
        library: &Path,
        objects: &[PathBuf],
        deps: &Path,
        sysroot: &Path,
    ) -> Option<Entry> {
        if search_roots(sysroot)? != self.search_roots || !verify_code_signatures(&[library]) {
            return None;
        }
        let inputs = LinkInputs::capture(&std::fs::read(deps).ok()?, objects, library)?;
        let files = BTreeMap::from([
            (IMAGE.into(), std::fs::read(library).ok()?),
            (INPUTS.into(), serde_json::to_vec(&inputs).ok()?),
        ]);
        Some(Entry {
            directory: self.directory.clone(),
            key: self.key.clone(),
            files,
        })
    }
}

fn restore_linked(directory: &Path, key: &str, library: &Path) -> bool {
    let mut event = timing::span("jit.runtime.cache.lookup");
    let names = BTreeSet::from([IMAGE.to_owned(), INPUTS.to_owned()]);
    let Some(files) = source_package_cache::completed(directory, key, &names) else {
        return false;
    };
    let Ok(inputs) = serde_json::from_slice::<LinkInputs>(&files[INPUTS]) else {
        return false;
    };
    if !inputs.is_current() || std::fs::write(library, &files[IMAGE]).is_err() {
        return false;
    }
    let signature = timing::span("jit.runtime.cache.codesign");
    let signed = verify_code_signatures(&[library]);
    drop(signature);
    if !signed {
        let _ = std::fs::remove_file(library);
        return false;
    }
    event.set_counter(
        "bytes",
        files.values().map(|bytes| bytes.len() as u64).sum(),
    );
    drop(event);
    let _hit = timing::span("jit.runtime.cache.hit");
    true
}

fn search_roots(sysroot: &Path) -> Option<BTreeMap<PathBuf, Option<String>>> {
    // ld may omit nonexistent search directories from its list of missing files.
    // Track their directory entries as well, including the host fallback roots.
    [sysroot.to_owned(), PathBuf::from("/")]
        .into_iter()
        .flat_map(|root| {
            [
                "usr/lib",
                "usr/local/lib",
                "Library/Frameworks",
                "System/Library/Frameworks",
            ]
            .into_iter()
            .map(move |name| root.join(name))
        })
        .map(|path| {
            let identity = match std::fs::read_dir(&path) {
                Ok(entries) => {
                    let mut names: Vec<_> = entries
                        .map(|entry| {
                            entry.map(|entry| entry.file_name().as_encoded_bytes().to_vec())
                        })
                        .collect::<Result<_, _>>()
                        .ok()?;
                    names.sort();
                    let wire = serde_json::to_vec(&(path.canonicalize().ok()?, names)).ok()?;
                    Some(sha256(&wire))
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(_) => return None,
            };
            Some((path, identity))
        })
        .collect()
}

fn copy_command(command: &Command) -> Command {
    let mut copy = Command::new(command.get_program());
    copy.args(command.get_args());
    for (name, value) in command.get_envs() {
        match value {
            Some(value) => {
                copy.env(name, value);
            }
            None => {
                copy.env_remove(name);
            }
        }
    }
    if let Some(cwd) = command.get_current_dir() {
        copy.current_dir(cwd);
    }
    copy
}

fn linker_plan(
    command: &Command,
    library: &Path,
    objects: &[PathBuf],
    toolchain: &TargetClang,
) -> Option<Vec<String>> {
    let output = copy_command(command).arg("-###").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = std::str::from_utf8(&output.stderr).ok()?;
    let mut commands = text
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('"'));
    let line = commands.next()?.strip_prefix('"')?.strip_suffix('"')?;
    if commands.next().is_some() {
        return None;
    }
    let args: Vec<_> = line.split("\" \"").collect();
    if args.iter().any(|arg| arg.contains(['\\', '"'])) {
        return None;
    }
    if Path::new(args[0]).canonicalize().ok()?.to_str()?
        != toolchain.fingerprint["tool.identity:ld"]
    {
        return None;
    }

    // Limit this proof to the fixed runtime link. Configured search paths,
    // response files, link scripts and additional inputs use the ordinary linker.
    let mut args_tail = args[1..].iter().copied();
    let mut sdk = false;
    let mut framework = false;
    let mut system = false;
    while let Some(arg) = args_tail.next() {
        match arg {
            "-demangle" | "-dynamic" | "-dylib" | "-adhoc_codesign" => {}
            "-lSystem" if !system => system = true,
            "-lto_library" | "-sdk_version" | "-macosx_version_min" => {
                args_tail.next()?;
            }
            "-arch" if args_tail.next()? == "arm64" => {}
            "-mllvm" if args_tail.next()? == "-enable-linkonceodr-outlining" => {}
            "-o" if Path::new(args_tail.next()?) == library => {}
            "-install_name" | "-dylib_install_name"
                if args_tail.next()? == "@rpath/compiled-async.dylib" => {}
            "-syslibroot"
                if !sdk
                    && Path::new(args_tail.next()?).canonicalize().ok()? == toolchain.sysroot =>
            {
                sdk = true
            }
            "-framework" if !framework && args_tail.next()? == "CoreFoundation" => framework = true,
            "-platform_version" if args_tail.next()? == "macos" => {
                args_tail.next()?;
                args_tail.next()?;
            }
            path if objects.iter().any(|object| object == Path::new(path)) => {}
            path if Path::new(path).is_absolute()
                && Path::new(path)
                    .file_name()
                    .is_some_and(|file| file == "libclang_rt.osx.a") => {}
            _ => return None,
        }
    }
    if !sdk || !framework || !system {
        return None;
    }
    let directory = library.parent()?.to_str()?;
    Some(
        args.into_iter()
            .map(|arg| arg.replace(directory, "@generated"))
            .collect(),
    )
}

fn run_link(command: &mut Command) -> std::io::Result<Output> {
    let _link = timing::span("jit.runtime.link");
    command.output()
}

pub(super) fn link(
    mut command: Command,
    library: &Path,
    objects: &[PathBuf],
    built: &BuiltNativePackages,
    resources: &Path,
    toolchain: &TargetClang,
    exact: Option<ExactCapture>,
) -> Result<Option<Pending>, String> {
    let cache = Cache::for_command(&command, library, objects, built, resources, toolchain);
    if let Some(cache) = &cache
        && restore_linked(&cache.directory, &cache.key, library)
    {
        return Ok(exact
            .and_then(|capture| capture.prepare_store(cache))
            .map(|exact| Pending {
                linked: None,
                exact: Some(exact),
            }));
    }
    let deps = library.with_extension("link-dependencies");
    let recorded = cache.as_ref().and_then(|_| {
        let _miss = timing::span("jit.runtime.cache.miss");
        let output = run_link(
            copy_command(&command)
                .args(["-Xlinker", "-dependency_info", "-Xlinker"])
                .arg(&deps),
        )
        .ok()?;
        output.status.success().then_some(output)
    });
    let has_record = recorded.is_some();
    let output = recorded.map_or_else(|| run_link(&mut command), Ok);
    crate::cli_diagnostics::tool_output(output, Some(library), "async runtime linking", "clang")?;
    Ok(cache.filter(|_| has_record).and_then(|cache| {
        let linked = cache.prepare_store(library, objects, &deps, &toolchain.sysroot)?;
        let exact = exact.and_then(|capture| capture.prepare_store(&cache));
        Some(Pending {
            linked: Some(linked),
            exact,
        })
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_receipt_checks_lookups_and_rejects_incomplete_records() {
        let root =
            std::env::temp_dir().join(format!("rinzu-jit-link-inputs-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let _cleanup = crate::verify_capture::TemporaryDirectory(root.clone());
        let library = root.join(IMAGE);
        let object = root.join("unit.o");
        let header = root.join("libCurrent.tbd");
        let alternate = root.join("libCurrent.dylib");
        std::fs::write(&header, b"original stub").unwrap();
        let mut record = b"\x00linker version\0".to_vec();
        for (code, path) in [
            (0x10, &object),
            (0x10, &header),
            (0x11, &alternate),
            (0x40, &library),
        ] {
            record.push(code);
            record.extend(path.as_os_str().as_encoded_bytes());
            record.push(0);
        }
        let objects = [object];
        let proof = LinkInputs::capture(&record, &objects, &library).unwrap();
        assert!(proof.is_current());
        assert!(LinkInputs::capture(&record[..record.len() - 1], &objects, &library).is_none());

        std::fs::write(&alternate, b"higher priority dylib").unwrap();
        assert!(!proof.is_current());
        std::fs::remove_file(&alternate).unwrap();
        assert!(proof.is_current());

        std::fs::write(&header, b"different stub").unwrap();
        assert!(!proof.is_current());
    }
}
