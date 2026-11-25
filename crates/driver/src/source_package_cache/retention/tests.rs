use super::*;
use crate::source_package_cache::{completed, directory_in_namespace, store};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};

fn files(byte: u8) -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([
        ("code.bin".into(), vec![byte; 4096]),
        ("metadata".into(), vec![byte; 64]),
    ])
}

fn entry(root: &Path, name: &[u8]) -> (PathBuf, String) {
    directory_in_namespace(root, root, "jit-code-v1", name).unwrap()
}

fn published(root: &Path, name: &[u8], time: u64) -> (PathBuf, String) {
    let (directory, key) = entry(root, name);
    store(&directory, &key, &files(name[0]), None).unwrap();
    File::open(directory.join("receipt.json"))
        .unwrap()
        .set_times(
            std::fs::FileTimes::new()
                .set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(time)),
        )
        .unwrap();
    (directory, key)
}

#[test]
fn oldest_sealed_generations_are_reclaimed_by_count_and_total_bytes() {
    let root = crate::verify_capture::TemporaryDirectory::new("cache-retention").unwrap();
    let (first, _) = published(&root.0, b"a", 1);
    let (second, _) = published(&root.0, b"b", 2);
    let (third, key) = published(&root.0, b"c", 3);
    let namespace = third.parent().unwrap();
    let pending = entry(&root.0, b"pending").0;
    std::fs::write(pending.join(".pending-build"), b"in progress").unwrap();
    let foreign = namespace.join("foreign");
    std::fs::create_dir(&foreign).unwrap();
    let external = root.0.join("external");
    std::fs::create_dir(&external).unwrap();
    std::fs::write(external.join("sentinel"), b"untouched").unwrap();
    std::os::unix::fs::symlink(&external, namespace.join("d".repeat(64))).unwrap();
    let _lease = lock(&third, true).unwrap();
    collect(
        namespace,
        &third,
        Limits {
            generations: 2,
            bytes: u64::MAX,
        },
    )
    .unwrap();
    assert!(!first.exists());
    assert!(second.exists() && third.exists());
    let size = generation(third.clone()).unwrap().bytes;
    assert!(size > files(b'c').values().map(|value| value.len() as u64).sum());
    collect(
        namespace,
        &third,
        Limits {
            generations: usize::MAX,
            bytes: size,
        },
    )
    .unwrap();
    assert!(!second.exists());
    assert_eq!(generation(third.clone()).unwrap().bytes, size);
    assert!(pending.join(".pending-build").is_file());
    assert!(foreign.is_dir());
    assert_eq!(
        std::fs::read(external.join("sentinel")).unwrap(),
        b"untouched"
    );
    drop(_lease);
    assert_eq!(
        completed(&third, &key, &files(b'c').keys().cloned().collect()),
        Some(files(b'c'))
    );
    // An evicted key can be saved again even if its lookup preceded eviction.
    let first_key = first.file_name().unwrap().to_str().unwrap();
    store(&first, first_key, &files(b'a'), None).unwrap();
    assert_eq!(
        completed(&first, first_key, &files(b'a').keys().cloned().collect()),
        Some(files(b'a'))
    );
}

#[test]
fn normal_publication_enforces_namespace_limits_without_a_separate_cleanup_call() {
    let root = crate::verify_capture::TemporaryDirectory::new("cache-automatic-retention").unwrap();
    let mut saved = Vec::new();
    for index in 0..LIMITS.generations + 2 {
        saved.push(published(
            &root.0,
            format!("generation-{index}").as_bytes(),
            index as u64 + 1,
        ));
    }
    assert!(!saved[0].0.exists() && !saved[1].0.exists());
    assert!(
        saved[2..]
            .iter()
            .all(|(directory, _)| directory.join("receipt.json").is_file())
    );
    assert_eq!(
        std::fs::read_dir(root.0.join("jit-code-v1"))
            .unwrap()
            .count(),
        LIMITS.generations
    );

    let byte_root = root.0.join("byte-budget");
    std::fs::create_dir(&byte_root).unwrap();
    let (old, _) = published(&byte_root, b"old", 1);
    // Sparse padding exercises real byte accounting without allocating or
    // hashing a 256 MiB test buffer. Receipts and all regular members count.
    File::create(old.join("padding.bin"))
        .unwrap()
        .set_len(LIMITS.bytes)
        .unwrap();
    let (current, key) = published(&byte_root, b"new", 2);
    assert!(
        !old.exists(),
        "the byte budget must reclaim even with just two generations"
    );
    assert_eq!(
        completed(&current, &key, &files(b'n').keys().cloned().collect()),
        Some(files(b'n'))
    );
}

#[test]
fn oversized_republication_reclaims_abandoned_files_and_keeps_run_copies_valid() {
    let root = crate::verify_capture::TemporaryDirectory::new("cache-abandoned-files").unwrap();
    let (directory, key) = published(&root.0, b"old", 1);
    let names = files(b'o').keys().cloned().collect();
    let running = completed(&directory, &key, &names).unwrap();
    File::create(directory.join(".pending-abandoned"))
        .unwrap()
        .set_len(LIMITS.bytes)
        .unwrap();
    assert!(store(&directory, &key, &files(b'n'), None).is_err());
    assert!(
        !directory.exists(),
        "the protected current key must not defeat the byte budget"
    );
    assert_eq!(running, files(b'o'));
    store(&directory, &key, &files(b'n'), None).unwrap();
    assert_eq!(completed(&directory, &key, &names), Some(files(b'n')));
}

struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn worker(root: &Path, mode: &str) -> Worker {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "source_package_cache::retention::tests::cache_process_worker",
            "--nocapture",
        ])
        .env("RINZU_RETENTION_WORKER", root)
        .env("RINZU_RETENTION_MODE", mode)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(child.stdout.as_mut().unwrap());
    loop {
        let mut line = String::new();
        assert!(
            stdout.read_line(&mut line).unwrap() > 0,
            "worker exited before attempting cache access"
        );
        if line.contains("CACHE_WORKER_READY") {
            break;
        }
    }
    // The worker writes no further output, so a pipe cannot block its progress.
    Worker(child)
}

#[test]
fn cache_process_worker() {
    let Some(root) = std::env::var_os("RINZU_RETENTION_WORKER").map(PathBuf::from) else {
        return;
    };
    let mode = std::env::var("RINZU_RETENTION_MODE").unwrap();
    let (directory, key) = entry(&root, if mode == "collect" { b"new" } else { b"shared" });
    if mode == "collect" {
        let lease = open_lock(&directory).unwrap();
        assert!(
            matches!(lease.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
            "collection acquired an exclusive lease while a reader was still copying"
        );
    }
    println!("CACHE_WORKER_READY");
    std::io::stdout().flush().unwrap();
    if mode == "collect" {
        store(&directory, &key, &files(b'n'), None).unwrap();
        let _lease = lock(&directory, true).unwrap();
        collect(
            directory.parent().unwrap(),
            &directory,
            Limits {
                generations: 1,
                bytes: u64::MAX,
            },
        )
        .unwrap();
    } else {
        for _ in 0..16 {
            store(&directory, &key, &files(mode.as_bytes()[0]), None).unwrap();
        }
    }
}

#[test]
fn readers_hold_off_other_process_reclamation_and_run_copies_survive_it() {
    let root = crate::verify_capture::TemporaryDirectory::new("cache-reader-lease").unwrap();
    let (directory, key) = published(&root.0, b"old", 1);
    let names = files(b'o').keys().cloned().collect();
    let mut child = None;
    // Start GC in another process after the receipt was opened but before the
    // payload is read. The completed reader must keep this generation alive.
    let copied =
        crate::source_package_cache::completed_with_inventory(&directory, &key, &[], |_| {
            let mut process = worker(&root.0, "collect");
            assert!(process.0.try_wait().unwrap().is_none());
            assert!(directory.join("receipt.json").is_file());
            child = Some(process);
            Some((names, ()))
        })
        .unwrap()
        .0;
    assert!(child.as_mut().unwrap().0.wait().unwrap().success());
    assert!(!directory.exists());
    assert_eq!(copied, files(b'o'));
    // Executable/native consumers use their own copy, never an evicted path.
    let run_copy = root.0.join("running-code.bin");
    std::fs::write(&run_copy, &copied["code.bin"]).unwrap();
    assert_eq!(std::fs::read(run_copy).unwrap(), vec![b'o'; 4096]);
}

#[test]
fn simultaneous_process_writers_and_readers_never_observe_mixed_generations() {
    let root = crate::verify_capture::TemporaryDirectory::new("cache-writer-lease").unwrap();
    let (directory, key) = entry(&root.0, b"shared");
    store(&directory, &key, &files(b'a'), None).unwrap();
    let mut a = worker(&root.0, "a");
    let mut b = worker(&root.0, "b");
    let names: BTreeSet<_> = files(b'a').keys().cloned().collect();
    for _ in 0..32 {
        let observed =
            completed(&directory, &key, &names).expect("a sealed generation is always available");
        assert!(observed == files(b'a') || observed == files(b'b'));
    }
    assert!(a.0.wait().unwrap().success());
    assert!(b.0.wait().unwrap().success());
    assert!(std::fs::read_dir(directory).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .as_encoded_bytes()
            .starts_with(b".pending-")
    }));
}
