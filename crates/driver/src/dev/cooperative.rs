//! Fresh-process cooperative development restart; no live heap/code migration.
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::Instant;

use rinzu_compiler::{Manifest, SourceCache, SourceSnapshot};
use serde_json::{Value, json};

use super::DevOptions;
use super::supervisor::{OwnedChild, interrupts};

const REPORT_LIMIT: usize = 16 * 1024 * 1024;
const HISTORY_BYTES: u64 = 64 * 1024 * 1024;
const HISTORY_GENERATIONS: usize = 32;

fn private_directory(path: &Path) -> Result<(), String> {
    fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .map_err(|error| error.to_string())
}

fn random_nonce() -> Result<String, String> {
    let mut bytes = [0_u8; 16];
    fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|e| e.to_string())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn private_json(path: &Path, value: &Value) -> Result<(), String> {
    let temporary = path.with_extension("tmp");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temporary)
        .map_err(|e| format!("save dev receipt: {e}"))?;
    let result = (|| {
        let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        drop(file);
        fs::rename(&temporary, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn read_json(path: &Path, limit: usize) -> Result<Value, String> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("private observation must be a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err("private observation exceeds budget".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

fn signal_ready(status: &Value, pid: u32, nonce: &str) -> bool {
    status["schema"] == "rinzu.dev-signal-status"
        && status["version"] == 1
        && status["processId"] == pid
        && status["nonce"] == nonce
        && status["enabled"] == true
}

fn acknowledged(report: &Value, pid: u32, nonce: &str, status: ExitStatus) -> bool {
    if !status.success()
        || report["schema"] != "rinzu.diagnostic-report"
        || report["version"] != 1
        || report["processId"] != pid
        || report["developmentNonce"] != nonce
        || report["phase"] != "finished"
        || !report["driverError"].is_null()
        || report["exitCode"] != 0
    {
        return false;
    }
    let outcome = &report["outcome"];
    if outcome["settlement"] != "confirmed" || !outcome["packetError"].is_null() {
        return false;
    }
    let failure = &outcome["failure"];
    if failure.is_null() {
        return outcome["status"] == "pass";
    }
    failure["cleanup"].as_array().is_some_and(Vec::is_empty)
        && failure["omittedCleanup"].as_u64().unwrap_or(0) == 0
        && failure["primary"]["cause"] == "Shutdown"
}

#[derive(Default)]
struct InputWatcher {
    paths: BTreeSet<PathBuf>,
    source_roots: BTreeSet<PathBuf>,
    previous: BTreeMap<PathBuf, Option<(usize, u64)>>,
    cache: SourceCache,
}
impl InputWatcher {
    fn check(&mut self, package: &Path) -> bool {
        let mut snapshot = SourceSnapshot::new(&mut self.cache, BTreeMap::new());
        let result = rinzu_compiler::check_package_with_sources(package, &mut snapshot);
        let mut paths = snapshot
            .input_receipts()
            .into_iter()
            .map(|receipt| receipt.0)
            .collect::<BTreeSet<_>>();
        match result {
            Ok((manifest, artifact)) => {
                paths.extend(artifact.module_graph.iter().flat_map(|module| {
                    module
                        .imports
                        .iter()
                        .filter_map(|import| import.requested_path.clone())
                }));
                let mut roots = BTreeSet::new();
                for manifest in std::iter::once(&manifest).chain(
                    artifact
                        .sdk_packages
                        .iter()
                        .map(|package| &package.manifest),
                ) {
                    add_manifest_inputs(manifest, &mut paths, &mut roots);
                }
                if !snapshot.disk_is_current() {
                    eprintln!("dev preflight inputs changed during check; old run retained");
                    return false;
                }
                self.paths = paths;
                self.source_roots = roots;
                drop(snapshot);
                self.cache.retain_paths(&self.paths);
                eprintln!("dev preflight accepted");
                true
            }
            Err(errors) => {
                self.paths.extend(paths);
                crate::print_diagnostics(errors);
                eprintln!("dev preflight failed; old run retained");
                false
            }
        }
    }
    fn scan(&mut self) -> Result<bool, String> {
        let mut paths = self.paths.clone();
        let mut visited = BTreeSet::new();
        for root in &self.source_roots {
            collect(root, &mut paths, &mut visited)?;
        }
        if paths.len() > 4096 {
            return Err("dev watch input count exceeds 4096".into());
        }
        let mut current = BTreeMap::new();
        let mut total = 0_usize;
        for path in paths {
            let fingerprint = match fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&path)
            {
                Ok(file) => {
                    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
                        return Err(format!(
                            "dev input is not a regular file: {}",
                            path.display()
                        ));
                    }
                    let mut bytes = Vec::new();
                    file.take(REPORT_LIMIT as u64 + 1)
                        .read_to_end(&mut bytes)
                        .map_err(|e| e.to_string())?;
                    total = total.saturating_add(bytes.len());
                    if bytes.len() > REPORT_LIMIT || total > HISTORY_BYTES as usize {
                        return Err("dev watch byte budget exceeded".into());
                    }
                    let canonical = path.canonicalize().map_err(|e| e.to_string())?;
                    let mut identity = canonical.as_os_str().as_encoded_bytes().to_vec();
                    identity.push(0);
                    identity.extend_from_slice(&bytes);
                    Some((bytes.len(), rinzu_compiler::fnv1a64(&identity)))
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(format!("read dev input {}: {error}", path.display())),
            };
            current.insert(path, fingerprint);
        }
        let changed = current != self.previous;
        self.previous = current;
        Ok(changed)
    }
}
fn add_manifest_inputs(
    manifest: &Manifest,
    paths: &mut BTreeSet<PathBuf>,
    roots: &mut BTreeSet<PathBuf>,
) {
    paths.insert(manifest.root.join("rinzu.toml"));
    paths.insert(manifest.root.join("rinzu.lock"));
    paths.extend(
        manifest
            .native
            .sources
            .iter()
            .chain(&manifest.native.headers)
            .chain(&manifest.native.objects)
            .map(|path| manifest.root.join(path)),
    );
    roots.insert(manifest.root.join("src"));
}
fn collect(
    directory: &Path,
    paths: &mut BTreeSet<PathBuf>,
    visited: &mut BTreeSet<PathBuf>,
) -> Result<(), String> {
    let canonical = match directory.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "read dev directory {}: {error}",
                directory.display()
            ));
        }
    };
    if !visited.insert(canonical) {
        return Ok(());
    }
    for entry in fs::read_dir(directory)
        .map_err(|e| format!("read dev directory {}: {e}", directory.display()))?
    {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            collect(&path, paths, visited)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "rinzu")
        {
            paths.insert(path);
        }
        if paths.len() > 4096 {
            return Err("dev watch input count exceeds 4096".into());
        }
    }
    Ok(())
}

struct Records {
    root: PathBuf,
    completed: VecDeque<PathBuf>,
    next: u64,
}
impl Records {
    fn new() -> Result<Self, String> {
        let root = std::env::temp_dir().join(format!(
            "rinzu-dev-{}-{}",
            std::process::id(),
            random_nonce()?
        ));
        private_directory(&root)?;
        eprintln!("dev session={}", root.display());
        Ok(Self {
            root,
            completed: VecDeque::new(),
            next: 1,
        })
    }
    fn start(&mut self, package: &Path, options: &DevOptions) -> Result<Run, String> {
        let id = self.next;
        self.next = self.next.checked_add(1).ok_or("dev generation exhausted")?;
        let directory = self.root.join(format!("generation-{id:06}"));
        private_directory(&directory)?;
        let nonce = random_nonce()?;
        let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
        command
            .arg("run")
            .arg(package)
            .arg("--diagnostic-report")
            .arg(directory.join("report.json"))
            .arg("--")
            .args(&options.program_arguments)
            .env("RINZU_DEV_NONCE", &nonce)
            .env("RINZU_DEV_SIGNAL_STATUS", directory.join("signals.json"));
        if options.null_stdin {
            command.stdin(Stdio::null());
        }
        let process = OwnedChild::spawn(&mut command, true)?;
        let mut run = Run {
            process,
            id,
            nonce,
            directory,
            events: Vec::new(),
            omitted: 0,
            requested: false,
            notified: None,
            timed_out: false,
            forced: false,
            completed: false,
            ack: false,
            blocked: None,
            capture_error: None,
        };
        run.event(json!({"event":"started","input":"fresh child check","liveReload":false}))?;
        eprintln!("dev started generation={} pid={}", id, run.process.pid());
        Ok(run)
    }
    fn finish(&mut self, run: Run) -> Result<(), String> {
        self.completed.push_back(run.directory.clone());
        drop(run);
        self.prune()
    }
    fn prune(&mut self) -> Result<(), String> {
        while self.completed.len() > HISTORY_GENERATIONS
            || self
                .completed
                .iter()
                .map(|path| directory_bytes(path))
                .collect::<Result<Vec<_>, _>>()?
                .iter()
                .sum::<u64>()
                > HISTORY_BYTES
        {
            let Some(path) = self.completed.pop_front() else {
                break;
            };
            fs::remove_dir_all(&path).map_err(|e| format!("prune dev history: {e}"))?;
            eprintln!(
                "dev history omitted {} (32 generations / 64 MiB budget)",
                path.display()
            );
        }
        Ok(())
    }
}
fn directory_bytes(directory: &Path) -> Result<u64, String> {
    let mut total = 0_u64;
    for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let metadata = entry
            .map_err(|e| e.to_string())?
            .metadata()
            .map_err(|e| e.to_string())?;
        total = total.saturating_add(metadata.len());
    }
    Ok(total)
}
struct Run {
    process: OwnedChild,
    id: u64,
    nonce: String,
    directory: PathBuf,
    events: Vec<Value>,
    omitted: usize,
    requested: bool,
    notified: Option<Instant>,
    timed_out: bool,
    forced: bool,
    completed: bool,
    ack: bool,
    blocked: Option<String>,
    capture_error: Option<String>,
}
impl Run {
    fn event(&mut self, event: Value) -> Result<(), String> {
        if self.events.len() == 256 {
            self.events.remove(0);
            self.omitted += 1;
        }
        self.events.push(event);
        let saved = private_json(
            &self.directory.join("receipt.json"),
            &json!({"schema":"rinzu.dev-restart","version":1,"generation":self.id,"nonce":self.nonce,"processId":self.process.pid(),"events":self.events,"omittedEvents":self.omitted,"acknowledged":self.ack,"forced":self.forced,"completed":self.completed,"liveReload":false,"processMemory":crate::diagnostics::process_memory(),"captureError":self.capture_error}),
        );
        if let Err(error) = saved {
            eprintln!("dev private receipt error: {error}; owned stop policy remains active");
            self.capture_error = Some(error);
            self.ack = false;
        }
        Ok(())
    }
    fn block(&mut self, reason: String) -> Result<(), String> {
        if self.blocked.as_ref() != Some(&reason) {
            eprintln!("dev blocked: {reason}; old ownership retained");
            self.event(json!({"event":"blocked","reason":reason}))?;
            self.blocked = Some(reason);
        }
        Ok(())
    }
    fn request(&mut self) -> Result<(), String> {
        if self.notified.is_some() {
            return Ok(());
        }
        let ready = read_json(&self.directory.join("signals.json"), 4096)
            .is_ok_and(|status| signal_ready(&status, self.process.pid(), &self.nonce));
        if !ready {
            return self.block("signal registration missing, disabled or identity mismatch".into());
        }
        self.process.notify()?;
        self.notified = Some(Instant::now());
        self.blocked = None;
        self.event(json!({"event":"notification-sent","signal":"SIGTERM","acknowledged":false}))?;
        eprintln!("dev notified generation={} once", self.id);
        Ok(())
    }
    fn finish(&mut self, status: ExitStatus) -> Result<(), String> {
        let report = read_json(&self.directory.join("report.json"), REPORT_LIMIT);
        self.ack = self.capture_error.is_none()
            && report
                .as_ref()
                .is_ok_and(|report| acknowledged(report, self.process.pid(), &self.nonce, status));
        self.completed = true;
        self.event(json!({"event":"waited","exitCode":status.code(),"signal":status.signal(),"acknowledged":self.ack,"settlement":if self.ack {"confirmed"} else {"unconfirmed"},"observationError":report.err()}))?;
        eprintln!(
            "dev stopped generation={} acknowledged={} forced={}",
            self.id, self.ack, self.forced
        );
        Ok(())
    }
}

pub(super) fn run(package: PathBuf, options: DevOptions) -> Result<(), String> {
    // SAFETY: isatty inspects the inherited stdin descriptor without changing it.
    if !options.null_stdin && unsafe { libc::isatty(libc::STDIN_FILENO) } == 1 {
        return Err("cooperative dev with inherited TTY stdin is unsupported; use --stdin null or pipe/file input".into());
    }
    let mut watcher = InputWatcher::default();
    if !watcher.check(&package) {
        return Err("initial dev preflight failed".into());
    }
    watcher.scan()?;
    let mut records = Records::new()?;
    let mut run = records.start(&package, &options)?;
    let mut pending = false;
    let mut exiting = false;
    let mut watcher_error = None;
    loop {
        if interrupts() > 0 && !exiting {
            exiting = true;
            pending = false;
            run.requested = true;
            run.event(json!({"event":"parent-stop-requested"}))?;
        }
        if interrupts() > 1 && !run.forced {
            run.forced = true;
            run.event(json!({"event":"emergency-force","settlement":"unconfirmed"}))?;
            run.process.force()?;
        }
        if !exiting {
            match watcher.scan() {
                Ok(true) => {
                    eprintln!("dev inputs changed");
                    let valid = watcher.check(&package);
                    if valid {
                        pending = true;
                        run.requested = run.process.poll()?.is_none();
                        run.event(
                            json!({"event":"valid-edit","coalesced":run.notified.is_some()}),
                        )?;
                    } else {
                        if run.notified.is_none() {
                            pending = false;
                        }
                        run.event(json!({"event":"invalid-edit","oldRunRetained":!run.completed}))?;
                    }
                    // Baseline includes any newly consumed dependency inputs.
                    if let Err(error) = watcher.scan() {
                        watcher_error = Some(error);
                    }
                }
                Ok(false) => {}
                Err(error) => watcher_error = Some(error),
            }
            if let Some(error) = &run.capture_error {
                watcher_error = Some(format!("private receipt capture failed: {error}"));
            }
            if let Some(error) = &watcher_error {
                exiting = true;
                pending = false;
                run.requested = true;
                run.block(format!("watcher error: {error}; requesting owned stop"))?;
            }
        }
        if let Some(status) = run.process.poll()? {
            if !run.completed {
                run.finish(status)?;
                if run.requested && !run.ack && !run.forced {
                    pending = false;
                }
            }
            if run.process.group_alive()? {
                run.block(
                    "child exited with remaining owned process group; settlement unconfirmed"
                        .into(),
                )?;
            } else if exiting {
                let confirmed = run.ack;
                records.finish(run)?;
                return if let Some(error) = watcher_error {
                    Err(error)
                } else if confirmed {
                    Ok(())
                } else {
                    Err("development stopped; settlement unconfirmed".into())
                };
            } else if pending && (run.ack || !run.requested || run.forced) {
                if watcher.check(&package) {
                    watcher.scan()?;
                    records.finish(run)?;
                    run = records.start(&package, &options)?;
                } else {
                    run.block("latest input failed after stop; awaiting a new edit".into())?;
                }
                pending = false;
            }
        } else if run.requested && (pending || exiting) {
            run.request()?;
            if run
                .notified
                .is_some_and(|started| started.elapsed() >= options.shutdown_timeout)
                && !run.timed_out
            {
                run.timed_out = true;
                run.event(json!({"event":"timeout","milliseconds":options.shutdown_timeout.as_millis(),"settlement":"unconfirmed"}))?;
                if options.force_on_timeout {
                    run.forced = true;
                    run.event(json!({"event":"timeout-force","settlement":"unconfirmed"}))?;
                    run.process.force()?;
                } else {
                    run.block(
                        "shutdown timeout; old run continues, no replacement started".into(),
                    )?;
                }
            }
        }
        std::thread::sleep(options.poll.min(std::time::Duration::from_millis(200)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolved_import_spelling_detects_same_bytes_symlink_retarget() {
        let directory =
            crate::verify_capture::TemporaryDirectory::new("cooperative-path-watch").unwrap();
        let source = directory.0.join("src");
        fs::create_dir(&source).unwrap();
        let source = source.canonicalize().unwrap();
        fs::write(
            directory.0.join("rinzu.toml"),
            "[package]\nname=\"watch\"\nlanguage=\"0.5\"\n",
        )
        .unwrap();
        let write = |path: &Path, text: &str| {
            let file = rinzu_compiler::SourceFile::new(rinzu_compiler::FileId(1), path, text);
            fs::write(path, rinzu_compiler::format_source(&file).unwrap()).unwrap();
        };
        write(
            &source.join("main.rinzu"),
            "import \"./alias.rinzu\" as model;fn main(){let n:Int=model.value();}",
        );
        let first = source.join("first.rinzu");
        let second = source.join("second.rinzu");
        write(&first, "fn value()->Int{return 1;}export value;");
        write(&second, "fn value()->Int{return 1;}export value;");
        let alias = source.join("alias.rinzu");
        std::os::unix::fs::symlink(&first, &alias).unwrap();
        let mut watcher = InputWatcher::default();
        assert!(watcher.check(&directory.0));
        assert!(watcher.paths.contains(&alias));
        assert!(watcher.scan().unwrap());
        assert!(!watcher.scan().unwrap());
        fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(&second, &alias).unwrap();
        assert!(watcher.scan().unwrap());
        assert!(watcher.check(&directory.0));
        watcher.scan().unwrap();
        assert!(!watcher.scan().unwrap());
    }

    #[test]
    fn acknowledgement_requires_private_identity_settlement_cleanup_and_wait() {
        let pid = 42;
        let nonce = "0123456789abcdef0123456789abcdef";
        let report = json!({"schema":"rinzu.diagnostic-report","version":1,"processId":pid,"developmentNonce":nonce,"phase":"finished","driverError":null,"exitCode":0,"outcome":{"status":"pass","settlement":"confirmed","failure":null}});
        assert!(acknowledged(&report, pid, nonce, ExitStatus::from_raw(0)));
        for (pointer, value) in [
            ("/phase", json!("prepared")),
            ("/developmentNonce", json!("old")),
            ("/processId", json!(43)),
            ("/outcome/settlement", json!("unknown")),
            ("/driverError", json!("save failed")),
        ] {
            let mut bad = report.clone();
            *bad.pointer_mut(pointer).unwrap() = value;
            assert!(!acknowledged(&bad, pid, nonce, ExitStatus::from_raw(0)));
        }
        assert!(!acknowledged(
            &report,
            pid,
            nonce,
            ExitStatus::from_raw(256)
        ));
        let mut shutdown = report.clone();
        shutdown["outcome"]["status"] = json!("fail");
        shutdown["outcome"]["failure"] =
            json!({"primary":{"cause":"Shutdown"},"cleanup":[],"omittedCleanup":0});
        assert!(acknowledged(&shutdown, pid, nonce, ExitStatus::from_raw(0)));
        shutdown["outcome"]["failure"]["cleanup"] = json!([{"cause":"Error(Conversion)"}]);
        assert!(!acknowledged(
            &shutdown,
            pid,
            nonce,
            ExitStatus::from_raw(0)
        ));
    }
    #[test]
    fn registration_requires_current_enabled_identity() {
        let status = json!({"schema":"rinzu.dev-signal-status","version":1,"processId":42,"nonce":"new","enabled":true});
        assert!(signal_ready(&status, 42, "new"));
        assert!(!signal_ready(&status, 42, "old"));
        assert!(!signal_ready(&status, 43, "new"));
        let mut disabled = status;
        disabled["enabled"] = json!(false);
        assert!(!signal_ready(&disabled, 42, "new"));
    }

    #[test]
    fn private_observations_refuse_fifo_symlink_and_oversized_files() {
        let root =
            std::env::temp_dir().join(format!("rinzu-dev-reader-{}", random_nonce().unwrap()));
        fs::create_dir(&root).unwrap();
        let file = root.join("report.json");
        fs::write(&file, b"{\"x\":123}").unwrap();
        assert!(read_json(&file, 2).is_err());
        let link = root.join("link");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(read_json(&link, 4096).is_err());
        let fifo = root.join("fifo");
        let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: name is a valid terminated path in this test's scratch directory.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(read_json(&fifo, 4096).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn completed_history_enforces_both_generation_and_byte_budgets() {
        let mut records = Records::new().unwrap();
        for id in 0..33 {
            let path = records.root.join(format!("generation-{id}"));
            fs::create_dir(&path).unwrap();
            records.completed.push_back(path);
        }
        records.prune().unwrap();
        assert_eq!(records.completed.len(), 32);
        assert!(!records.root.join("generation-0").exists());
        let latest = records.completed.back().unwrap();
        let file = fs::File::create(latest.join("large.json")).unwrap();
        file.set_len(HISTORY_BYTES + 1).unwrap(); // sparse budget vector
        records.prune().unwrap();
        assert!(records.completed.is_empty());
        fs::remove_dir_all(records.root).unwrap();
    }

    #[test]
    fn relative_sdk_native_inputs_are_watched_from_the_sdk_root() {
        let root =
            std::env::temp_dir().join(format!("rinzu-dev-watch-{}", random_nonce().unwrap()));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("rinzu.toml"), "[package]\nname=\"watch\"\nlanguage=\"0.5\"\nkind=\"sdk\"\n[native]\nsources=[\"api.c\"]\nheaders=[\"api.h\"]\nobjects=[\"api.o\"]\n").unwrap();
        fs::write(root.join("api.c"), "int value(void) { return 1; }\n").unwrap();
        fs::write(root.join("api.h"), "int value(void);\n").unwrap();
        let manifest = rinzu_compiler::load_manifest(&root.join("rinzu.toml")).unwrap();
        let mut watcher = InputWatcher::default();
        add_manifest_inputs(&manifest, &mut watcher.paths, &mut watcher.source_roots);
        assert!(watcher.scan().unwrap());
        assert!(!watcher.scan().unwrap());
        fs::write(root.join("api.c"), "int value(void) { return 2; }\n").unwrap();
        assert!(watcher.scan().unwrap());
        fs::write(root.join("api.h"), "long value(void);\n").unwrap();
        assert!(watcher.scan().unwrap());
        fs::write(root.join("api.o"), b"opaque object change vector").unwrap();
        assert!(watcher.scan().unwrap());
        fs::write(root.join("rinzu.lock"), b"opaque lock change vector").unwrap();
        assert!(watcher.scan().unwrap());
        fs::remove_dir_all(root).unwrap();
    }
}
