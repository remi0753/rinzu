use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::OsString;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

mod cooperative;
mod supervisor;
use supervisor::{OwnedChild, Signals, interrupts};

#[derive(Clone, Debug)]
pub struct DevOptions {
    pub poll: Duration,
    pub once: bool,
    pub cooperative: bool,
    pub shutdown_timeout: Duration,
    pub force_on_timeout: bool,
    pub null_stdin: bool,
    pub program_arguments: Vec<OsString>,
}

pub fn parse(arguments: impl Iterator<Item = String>) -> Result<(PathBuf, DevOptions), String> {
    let mut arguments = arguments;
    let mut path = None;
    let mut seen = BTreeSet::new();
    let mut options = DevOptions {
        poll: Duration::from_millis(200),
        once: false,
        cooperative: false,
        shutdown_timeout: Duration::from_millis(5000),
        force_on_timeout: false,
        null_stdin: false,
        program_arguments: Vec::new(),
    };
    while let Some(argument) = arguments.next() {
        if argument.starts_with("--") && !seen.insert(argument.clone()) {
            return Err(format!("duplicate dev option `{argument}`"));
        }
        match argument.as_str() {
            "--once" => options.once = true,
            "--poll-ms" | "--shutdown-timeout-ms" => {
                let value = arguments
                    .next()
                    .ok_or_else(|| format!("{argument} requires a number"))?;
                let millis = value
                    .parse::<u64>()
                    .map_err(|_| format!("invalid dev duration `{value}`"))?;
                if millis == 0 {
                    return Err(format!("{argument} must be positive"));
                }
                if argument == "--poll-ms" {
                    options.poll = Duration::from_millis(millis);
                } else {
                    options.shutdown_timeout = Duration::from_millis(millis);
                }
            }
            "--restart" => {
                options.cooperative = match arguments.next().as_deref() {
                    Some("immediate") => false,
                    Some("cooperative") => true,
                    _ => return Err("--restart requires immediate or cooperative".into()),
                }
            }
            "--on-timeout" => {
                if arguments.next().as_deref() != Some("force") {
                    return Err("--on-timeout requires force".into());
                }
                options.force_on_timeout = true;
            }
            "--stdin" => {
                if arguments.next().as_deref() != Some("null") {
                    return Err("--stdin requires null".into());
                }
                options.null_stdin = true;
            }
            _ if argument.starts_with('-') => {
                return Err(format!("unknown dev option `{argument}`"));
            }
            _ if path.is_none() => path = Some(PathBuf::from(argument)),
            _ => return Err(format!("unexpected dev argument `{argument}`")),
        }
    }
    let restart_options = [
        "--restart",
        "--shutdown-timeout-ms",
        "--on-timeout",
        "--stdin",
    ]
    .iter()
    .any(|option| seen.contains(*option));
    if options.once && restart_options {
        return Err("--once cannot be combined with restart options".into());
    }
    if !options.cooperative
        && ["--shutdown-timeout-ms", "--on-timeout", "--stdin"]
            .iter()
            .any(|option| seen.contains(*option))
    {
        return Err(
            "shutdown timeout, force and stdin options require --restart cooperative".into(),
        );
    }
    let path = path.map_or_else(|| std::env::current_dir().map_err(|e| e.to_string()), Ok)?;
    Ok((path, options))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ModuleSnapshot {
    hash: u64,
    imports: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Invalidation {
    pub changed: BTreeSet<String>,
    pub invalidated: BTreeSet<String>,
}

pub struct PackageWatcher {
    package: PathBuf,
    source_root: PathBuf,
    modules: BTreeMap<String, ModuleSnapshot>,
}

impl PackageWatcher {
    pub fn new(package: &Path) -> Result<Self, String> {
        let manifest_path = rinzu_compiler::find_manifest(package).map_err(|error| {
            crate::cli_diagnostics::emit(&[error.into_diagnostic()], |_| None);
            "input failed to check".to_owned()
        })?;
        let manifest = crate::input::load_manifest(&manifest_path)?;
        let source_root = manifest.root.join("src");
        let source_root = source_root.canonicalize().map_err(|error| {
            crate::cli_diagnostics::file_failure(
                &source_root,
                format!("cannot resolve source directory: {error}"),
                "make the package source directory readable",
            )
        })?;
        Ok(Self {
            source_root,
            package: manifest.root,
            modules: BTreeMap::new(),
        })
    }

    pub fn scan(&mut self) -> Result<Invalidation, String> {
        let mut current = read_modules(&self.source_root)?;
        let mut changed = BTreeSet::new();
        for (name, module) in &current {
            if self.modules.get(name).map(|old| old.hash) != Some(module.hash) {
                changed.insert(name.clone());
            }
        }
        for name in self.modules.keys() {
            if !current.contains_key(name) {
                changed.insert(name.clone());
            }
        }
        if changed.is_empty() {
            return Ok(Invalidation::default());
        }
        if let Ok((_, artifact)) = rinzu_compiler::check_package(&self.package) {
            for module in &artifact.module_graph {
                let Some(name) = module_name(&self.source_root, &module.path) else {
                    continue;
                };
                if let Some(snapshot) = current.get_mut(&name) {
                    snapshot.imports = module
                        .imports
                        .iter()
                        .flat_map(|import| [&import.target_path, &import.requested_path])
                        .filter_map(|path| path.as_deref())
                        .filter_map(|path| module_name(&self.source_root, path))
                        .collect();
                }
            }
        } else {
            // Preserve known edges while an edited graph is invalid. The new child
            // reports the frontend failure; the previous graph is not a typed result.
            for (name, snapshot) in &mut current {
                if let Some(previous) = self.modules.get(name) {
                    snapshot.imports.extend(previous.imports.iter().cloned());
                }
            }
        }
        let mut reverse = reverse_dependencies(&self.modules);
        for (dependency, dependents) in reverse_dependencies(&current) {
            reverse.entry(dependency).or_default().extend(dependents);
        }
        let mut invalidated = changed.clone();
        let mut pending = changed.iter().cloned().collect::<VecDeque<_>>();
        while let Some(module) = pending.pop_front() {
            if let Some(dependents) = reverse.get(&module) {
                for dependent in dependents {
                    if invalidated.insert(dependent.clone()) {
                        pending.push_back(dependent.clone());
                    }
                }
            }
        }
        self.modules = current;
        Ok(Invalidation {
            changed,
            invalidated,
        })
    }
}

pub fn run_dev(package: PathBuf, options: DevOptions) -> Result<(), String> {
    crate::input::require_package("dev", &package)?;
    let _signals = Signals::install()?;
    if options.cooperative {
        return cooperative::run(package, options);
    }
    let mut watcher = PackageWatcher::new(&package)?;
    let initial = watcher.scan()?;
    print_invalidation(&initial);
    let mut child = start_child(&package, &options)?;
    loop {
        if interrupts() > 0 {
            child.force()?;
            return Err(
                "development stopped; immediate termination, settlement unconfirmed".into(),
            );
        }
        if options.once {
            if let Some(status) = child.poll()? {
                if status.code().is_some() {
                    crate::PROGRAM_EXIT_STATUS.with(|exit| exit.set(u8::from(!status.success())));
                    return Ok(());
                }
                return Err(format!("development process exited with {status}"));
            }
            thread::sleep(options.poll);
            continue;
        }
        thread::sleep(options.poll);
        let invalidation = watcher.scan()?;
        if invalidation.invalidated.is_empty() {
            child.poll()?;
            continue;
        }
        print_invalidation(&invalidation);
        child.force()?;
        child = start_child(&package, &options)?;
    }
}

fn start_child(package: &Path, options: &DevOptions) -> Result<OwnedChild, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("locate rinzu development runner: {error}"))?;
    let child = OwnedChild::spawn(
        Command::new(executable)
            .arg("run")
            .arg(package)
            .arg("--")
            .args(&options.program_arguments),
        false,
    )?;
    eprintln!("dev started immediate pid={}", child.pid());
    Ok(child)
}

fn print_invalidation(invalidation: &Invalidation) {
    eprintln!(
        "dev changed=[{}] invalidated=[{}]",
        invalidation
            .changed
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(","),
        invalidation
            .invalidated
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(",")
    );
}

fn read_modules(source_root: &Path) -> Result<BTreeMap<String, ModuleSnapshot>, String> {
    let mut files = Vec::new();
    collect_sources(source_root, &mut files)?;
    let mut modules = BTreeMap::new();
    for path in files {
        let module = module_name(source_root, &path).unwrap();
        let source = fs::read(&path).map_err(|error| {
            crate::cli_diagnostics::file_failure(
                &path,
                format!("cannot read module: {error}"),
                "make the source file readable",
            )
        })?;
        let mut hasher = std::hash::DefaultHasher::new();
        source.hash(&mut hasher);
        let canonical = path
            .canonicalize()
            .map_err(|error| format!("resolve module {}: {error}", path.display()))?;
        canonical.hash(&mut hasher);
        modules.insert(
            module,
            ModuleSnapshot {
                hash: hasher.finish(),
                imports: parse_imports(source_root, &canonical, &source),
            },
        );
    }
    Ok(modules)
}

fn module_name(source_root: &Path, path: &Path) -> Option<String> {
    // Normalize `.` / `..` without resolving the final symlink spelling.
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    Some(
        normalized
            .strip_prefix(source_root)
            .ok()?
            .with_extension("")
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "::"),
    )
}

fn collect_sources(directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(directory)
        .map_err(|error| format!("read source directory {}: {error}", directory.display()))?
    {
        let entry = entry.map_err(|error| format!("read source entry: {error}"))?;
        let path = entry.path();
        if path.is_dir() {
            collect_sources(&path, files)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "rinzu")
        {
            files.push(path);
        }
    }
    files.sort();
    Ok(())
}

fn parse_imports(source_root: &Path, path: &Path, source: &[u8]) -> BTreeSet<String> {
    let Ok(syntax) =
        rinzu_compiler::SourceFile::from_utf8(rinzu_compiler::FileId(1), path, source.to_vec())
            .and_then(|file| rinzu_compiler::parse_source(&file))
    else {
        return BTreeSet::new();
    };
    syntax
        .imports
        .iter()
        .flat_map(|import| {
            let requested = if let Some(file) = &import.file {
                path.parent().unwrap().join(&file.path)
            } else {
                let local = source_root
                    .join(import.path.iter().collect::<PathBuf>())
                    .with_extension("rinzu");
                if !local.exists() {
                    return Vec::new();
                }
                local
            };
            [
                module_name(source_root, &requested),
                requested
                    .canonicalize()
                    .ok()
                    .and_then(|target| module_name(source_root, &target)),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
        })
        .collect()
}

fn reverse_dependencies(
    modules: &BTreeMap<String, ModuleSnapshot>,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut reverse = BTreeMap::<String, BTreeSet<String>>::new();
    for (module, snapshot) in modules {
        for dependency in &snapshot.imports {
            reverse
                .entry(dependency.clone())
                .or_default()
                .insert(module.clone());
        }
    }
    reverse
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_source(path: &Path, text: &str) {
        let file = rinzu_compiler::SourceFile::new(rinzu_compiler::FileId(1), path, text);
        fs::write(path, rinzu_compiler::format_source(&file).unwrap()).unwrap();
    }

    #[test]
    fn resolved_path_edges_invalidate_importers_after_edit_and_symlink_retarget() {
        let directory = crate::verify_capture::TemporaryDirectory::new("path-watch").unwrap();
        let source = directory.0.join("src");
        fs::create_dir_all(source.join("lib")).unwrap();
        fs::write(
            directory.0.join("rinzu.toml"),
            "[package]\nname=\"watch\"\nlanguage=\"0.5\"\n",
        )
        .unwrap();
        write_source(
            &source.join("main.rinzu"),
            "import \"./lib/facade.rinzu\" as facade;fn main(){let n:Int=facade.value();}",
        );
        write_source(
            &source.join("lib/facade.rinzu"),
            "import \"../alias.rinzu\" as model;fn value()->Int{return model.value();}export value;",
        );
        let first = source.join("first.rinzu");
        let second = source.join("second.rinzu");
        write_source(&first, "fn value()->Int{return 1;}export value;");
        write_source(&second, "fn value()->Int{return 1;}export value;");
        let alias = source.join("alias.rinzu");
        std::os::unix::fs::symlink(&first, &alias).unwrap();
        let mut watcher = PackageWatcher::new(&directory.0).unwrap();
        assert_eq!(watcher.scan().unwrap().changed.len(), 5);
        assert!(watcher.scan().unwrap().changed.is_empty());
        write_source(&first, "fn value()->Int{return 2;}export value;");
        let edited = watcher.scan().unwrap();
        assert_eq!(
            edited.changed,
            BTreeSet::from(["alias".into(), "first".into()])
        );
        assert_eq!(
            edited.invalidated,
            BTreeSet::from([
                "alias".into(),
                "first".into(),
                "lib::facade".into(),
                "main".into()
            ])
        );
        write_source(&second, "fn value()->Int{return 2;}export value;");
        watcher.scan().unwrap();
        fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(&second, &alias).unwrap();
        let retargeted = watcher.scan().unwrap();
        assert_eq!(retargeted.changed, BTreeSet::from(["alias".into()]));
        assert_eq!(
            retargeted.invalidated,
            BTreeSet::from(["alias".into(), "lib::facade".into(), "main".into()])
        );
        assert!(watcher.scan().unwrap().changed.is_empty());
    }

    #[test]
    fn dev_options_require_explicit_cooperative_mode_and_positive_unique_values() {
        for args in [
            vec!["--poll-ms", "0"],
            vec!["--once", "--once"],
            vec!["--once", "--restart", "cooperative"],
            vec!["--shutdown-timeout-ms", "50"],
            vec!["--on-timeout", "force"],
            vec!["--restart", "unknown"],
            vec!["--unknown"],
        ] {
            assert!(parse(args.into_iter().map(str::to_owned)).is_err());
        }
        let (path, options) = parse(
            [
                "app",
                "--restart",
                "cooperative",
                "--shutdown-timeout-ms",
                "50",
                "--stdin",
                "null",
                "--on-timeout",
                "force",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .unwrap();
        assert_eq!(path, PathBuf::from("app"));
        assert!(options.cooperative && options.force_on_timeout && options.null_stdin);
        assert_eq!(options.shutdown_timeout, Duration::from_millis(50));
    }

    #[test]
    fn content_changes_invalidate_the_reverse_dependency_closure() {
        let root = std::env::temp_dir().join(format!("rinzu-watch-test-{}", std::process::id()));
        let source = root.join("src");
        fs::create_dir_all(&source).unwrap();
        fs::write(
            root.join("rinzu.toml"),
            "[package]\nname = \"watch\"\nlanguage = \"0.1\"\n",
        )
        .unwrap();
        write_source(&source.join("main.rinzu"), "import math;fn main() {}");
        write_source(&source.join("math.rinzu"), "fn value() { let x = 1; }");
        write_source(&source.join("unused.rinzu"), "fn unused() {}");

        let mut watcher = PackageWatcher::new(&root).unwrap();
        let initial = watcher.scan().unwrap();
        assert_eq!(initial.invalidated.len(), 3);
        assert!(watcher.scan().unwrap().invalidated.is_empty());

        write_source(&source.join("math.rinzu"), "fn value() { let x = 2; }");
        let changed = watcher.scan().unwrap();
        assert_eq!(changed.changed, BTreeSet::from(["math".into()]));
        assert_eq!(
            changed.invalidated,
            BTreeSet::from(["main".into(), "math".into()])
        );
        fs::remove_dir_all(root).unwrap();
    }
}
