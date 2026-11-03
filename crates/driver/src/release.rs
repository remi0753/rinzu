use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rinzu_runtime::reactor::AsyncReactor;
use rinzu_runtime::scheduler::{Resolution, TaskOutcome};
use rinzu_runtime::standalone::{
    ManagedTask, RunStatus, StandaloneOptions, StandaloneRuntime, TaskAction, TaskEvent,
};
use rinzu_runtime::worker::CpuWorkerPool;
use serde_json::json;

use crate::aot;

pub struct ReleaseArtifact {
    pub directory: PathBuf,
    pub executable: PathBuf,
    pub manifest: PathBuf,
}

pub fn build_release(package: &Path, directory: PathBuf) -> Result<ReleaseArtifact, String> {
    crate::input::require_package("release", package)?;
    let equivalence = aot::verify_equivalence(package, None)?;
    if equivalence.outcome != "success" {
        return Err("v0.1 release sample must complete successfully".into());
    }
    let service_score = run_async_services(package)?;
    if service_score != 40 {
        return Err(format!(
            "async file/CPU service result mismatch: expected 40, found {service_score}"
        ));
    }
    fs::create_dir_all(&directory).map_err(|error| {
        format!(
            "create release artifact directory {}: {error}",
            directory.display()
        )
    })?;
    let executable = directory.join("v0_1_async_spawn");
    let build = aot::build_package(package, Some(executable.clone()))?;

    let verification_directory = std::env::temp_dir().join(format!(
        "rinzu-release-repro-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| format!("read system clock: {error}"))?
            .as_nanos()
    ));
    let verification = aot::build_package(
        package,
        Some(verification_directory.join("v0_1_async_spawn")),
    )?;
    for (left, right, label) in [
        (&build.object, &verification.object, "object"),
        (&build.metadata, &verification.metadata, "metadata"),
        (&build.executable, &verification.executable, "executable"),
    ] {
        let left_bytes = fs::read(left)
            .map_err(|error| format!("read release {label} {}: {error}", left.display()))?;
        let right_bytes = fs::read(right)
            .map_err(|error| format!("read verification {label} {}: {error}", right.display()))?;
        if left_bytes != right_bytes {
            return Err(format!("release {label} is not reproducible"));
        }
    }
    let _ = fs::remove_dir_all(verification_directory);

    let runtime_evidence = directory.join("runtime-evidence.json");
    fs::write(
        &runtime_evidence,
        serde_json::to_vec_pretty(&json!({
            "schema": "rinzu.v0.1-runtime-evidence",
            "version": 1,
            "asyncFileBytes": 20,
            "cpuSpawnScore": service_score,
            "resumeDomain": "main",
            "resources": "baseline",
            "shutdown": "complete",
            "jitAotOutcome": equivalence.outcome,
            "jitAotResult": equivalence.result,
        }))
        .map_err(|error| format!("serialize runtime evidence: {error}"))?,
    )
    .map_err(|error| format!("write runtime evidence: {error}"))?;
    let manifest = directory.join("release-manifest.json");
    let files = [
        ("executable", &build.executable),
        ("object", &build.object),
        ("metadata", &build.metadata),
        ("runtimeEvidence", &runtime_evidence),
    ];
    let checksums = files
        .iter()
        .map(|(name, path)| {
            let bytes = fs::read(path)
                .map_err(|error| format!("read artifact {}: {error}", path.display()))?;
            Ok((
                (*name).to_owned(),
                json!({
                    "file": path.file_name().unwrap().to_string_lossy(),
                    "bytes": bytes.len(),
                    "fnv1a64": format!("{:016x}", fnv1a64(&bytes)),
                }),
            ))
        })
        .collect::<Result<serde_json::Map<String, serde_json::Value>, String>>()?;
    fs::write(
        &manifest,
        serde_json::to_vec_pretty(&json!({
            "schema": "rinzu.v0.1-release",
            "version": 1,
            "target": rinzu_runtime::V1_TARGET,
            "installerIncluded": false,
            "reproducible": true,
            "files": checksums,
        }))
        .map_err(|error| format!("serialize release manifest: {error}"))?,
    )
    .map_err(|error| format!("write release manifest: {error}"))?;
    Ok(ReleaseArtifact {
        directory,
        executable,
        manifest,
    })
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3)
    })
}

struct FileChild;

impl ManagedTask for FileChild {
    fn resume(&mut self, event: TaskEvent) -> TaskAction {
        match event {
            TaskEvent::Start => TaskAction::Pending {
                label: "sample-file".into(),
                cleanup_safe: false,
            },
            TaskEvent::Operation(Resolution::Completed(Ok(()))) => TaskAction::Return,
            event => TaskAction::Fail(format!("unexpected sample file event: {event:?}")),
        }
    }
}

struct CpuParent {
    child: Option<Box<dyn ManagedTask>>,
}

impl ManagedTask for CpuParent {
    fn resume(&mut self, event: TaskEvent) -> TaskAction {
        match event {
            TaskEvent::Start => TaskAction::Spawn(self.child.take().unwrap()),
            TaskEvent::ChildSpawned(_) => TaskAction::Pending {
                label: "sample-cpu".into(),
                cleanup_safe: false,
            },
            TaskEvent::Operation(Resolution::Completed(Ok(()))) => TaskAction::Return,
            event => TaskAction::Fail(format!("unexpected sample CPU event: {event:?}")),
        }
    }
}

fn run_async_services(package: &Path) -> Result<u64, String> {
    let mut runtime = StandaloneRuntime::new(StandaloneOptions::default());
    runtime
        .start_main(Box::new(CpuParent {
            child: Some(Box::new(FileChild)),
        }))
        .map_err(|error| error.to_string())?;
    let status = runtime
        .run_until_stalled()
        .map_err(|error| error.to_string())?;
    if status
        != (RunStatus::Stalled {
            pending_operations: 2,
        })
    {
        return Err(format!(
            "sample did not expose two pending operations: {status:?}"
        ));
    }
    let tickets = runtime.take_operation_tickets();
    let file_ticket = tickets
        .iter()
        .find(|ticket| ticket.label == "sample-file")
        .cloned()
        .ok_or_else(|| "sample file ticket is missing".to_owned())?;
    let cpu_ticket = tickets
        .iter()
        .find(|ticket| ticket.label == "sample-cpu")
        .cloned()
        .ok_or_else(|| "sample CPU ticket is missing".to_owned())?;
    let reactor = AsyncReactor::new();
    let workers = CpuWorkerPool::new();
    let file = reactor
        .submit_file_read(file_ticket, package.join("data/input.txt"))
        .map_err(|error| error.to_string())?;
    let cpu = workers
        .spawn(cpu_ticket, |_domain| {
            Ok(20_u64.wrapping_mul(2).to_le_bytes().to_vec())
        })
        .map_err(|error| error.to_string())?;
    reactor
        .wait_completion(Duration::from_secs(2))
        .map_err(|error| error.to_string())?;
    workers
        .wait_completion(Duration::from_secs(2))
        .map_err(|error| error.to_string())?;
    if runtime
        .run_until_stalled()
        .map_err(|error| error.to_string())?
        != RunStatus::Complete(TaskOutcome::Returned)
    {
        return Err("sample standalone runtime did not complete".into());
    }
    let file_bytes = file
        .take()
        .ok_or_else(|| "sample file result is missing".to_owned())?
        .map_err(|error| format!("sample file read failed: {error}"))?;
    if file_bytes.len() != 20 {
        return Err(format!(
            "sample file must contain 20 bytes, found {}",
            file_bytes.len()
        ));
    }
    let cpu = cpu
        .take()
        .ok_or_else(|| "sample CPU result is missing".to_owned())?
        .map_err(|error| format!("sample CPU spawn failed: {error}"))?;
    if !runtime.resources().used.is_baseline() {
        return Err("sample runtime resources did not return to baseline".into());
    }
    let bytes: [u8; 8] = cpu
        .bytes
        .try_into()
        .map_err(|_| "sample CPU result has the wrong width".to_owned())?;
    Ok(u64::from_le_bytes(bytes))
}
