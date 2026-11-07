//! Versioned standard-package resources, independent of the caller's checkout.
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use sha2::{Digest, Sha256};

pub const TARGET: &str = "aarch64-apple-darwin";
pub const REVISION: u32 = 7;
/// Shared by the embedded registry and its distribution fingerprint.
pub const RUNTIME_CODEGEN_REVISION: u32 = 183;
pub const SOURCE_PACKAGES: [&str; 10] = [
    "rinzu.option",
    "rinzu.io",
    "rinzu.process",
    "rinzu.json",
    "rinzu.stream",
    "rinzu.testing",
    "rinzu.shutdown",
    "rinzu.http",
    "rinzu.httpcodec",
    "rinzu.socket",
];

const BUILTIN_PACKAGES: [&str; 5] = [
    "rinzu.core",
    "rinzu.stdio",
    "rinzu.timer",
    "rinzu.list",
    "rinzu.map",
];

pub(crate) fn builtin(name: &str) -> bool {
    BUILTIN_PACKAGES.contains(&name)
}

pub(crate) fn intrinsic_source(name: &str) -> Option<&'static [u8]> {
    match name {
        "rinzu.process" => Some(include_bytes!("../../../sdk/process/src/process.rinzu")),
        "rinzu.json" => Some(include_bytes!("../../../sdk/json/src/json.rinzu")),
        _ => None,
    }
}

pub fn registered(name: &str) -> bool {
    builtin(name) || SOURCE_PACKAGES.contains(&name)
}

pub fn names() -> impl Iterator<Item = &'static str> {
    BUILTIN_PACKAGES.into_iter().chain(SOURCE_PACKAGES)
}

pub fn dependencies(name: &str) -> &'static [&'static str] {
    match name {
        "rinzu.process" => &["rinzu.option"],
        "rinzu.stream" => &["rinzu.io"],
        _ => &[],
    }
}

macro_rules! sdk_resource {
    ($package:literal, $file:literal) => {
        (
            concat!("packages/", $package, "/", $file),
            include_bytes!(concat!("../../../sdk/", $package, "/", $file)).as_slice(),
        )
    };
}

macro_rules! repository_resource {
    ($name:literal, $source:literal) => {
        (
            $name,
            include_bytes!(concat!("../../../", $source)).as_slice(),
        )
    };
}

pub fn resources() -> BTreeMap<&'static str, &'static [u8]> {
    BTreeMap::from([
        repository_resource!("include/rinzu/byte_scan.h", "include/rinzu/byte_scan.h"),
        repository_resource!("include/rinzu/callback_consumption.h", "include/rinzu/callback_consumption.h"),
        repository_resource!("include/rinzu/callback_entry.h", "include/rinzu/callback_entry.h"),
        repository_resource!("include/rinzu/callback_return.h", "include/rinzu/callback_return.h"),
        repository_resource!("include/rinzu/child_scope.h", "include/rinzu/child_scope.h"),
        repository_resource!("include/rinzu/identity_index.h", "include/rinzu/identity_index.h"),
        repository_resource!("include/rinzu/continuation.h", "include/rinzu/continuation.h"),
        repository_resource!("include/rinzu/display.h", "include/rinzu/display.h"),
        repository_resource!("include/rinzu/enum_key.h", "include/rinzu/enum_key.h"),
        repository_resource!("include/rinzu/error_value.h", "include/rinzu/error_value.h"),
        repository_resource!("include/rinzu/error_trace.h", "include/rinzu/error_trace.h"),
        repository_resource!("include/rinzu/failure.h", "include/rinzu/failure.h"),
        repository_resource!("include/rinzu/native_fatal.h", "include/rinzu/native_fatal.h"),
        repository_resource!("include/rinzu/failure_transport.h", "include/rinzu/failure_transport.h"),
        repository_resource!("include/rinzu/foreign_queue.h", "include/rinzu/foreign_queue.h"),
        repository_resource!("include/rinzu/json.h", "include/rinzu/json.h"),
        repository_resource!("include/rinzu/json.c", "include/rinzu/json.c"),
        repository_resource!("include/rinzu/vendor/ffc-LICENSE-MIT", "include/rinzu/vendor/ffc-LICENSE-MIT"),
        repository_resource!("include/rinzu/vendor/ffc.h", "include/rinzu/vendor/ffc.h"),
        repository_resource!("include/rinzu/vendor/ryu/LICENSE-Boost", "include/rinzu/vendor/ryu/LICENSE-Boost"),
        repository_resource!("include/rinzu/vendor/ryu/common.h", "include/rinzu/vendor/ryu/common.h"),
        repository_resource!("include/rinzu/vendor/ryu/d2s.c", "include/rinzu/vendor/ryu/d2s.c"),
        repository_resource!("include/rinzu/vendor/ryu/d2s.h", "include/rinzu/vendor/ryu/d2s.h"),
        repository_resource!("include/rinzu/vendor/ryu/d2s_full_table.h", "include/rinzu/vendor/ryu/d2s_full_table.h"),
        repository_resource!("include/rinzu/vendor/ryu/d2s_intrinsics.h", "include/rinzu/vendor/ryu/d2s_intrinsics.h"),
        repository_resource!("include/rinzu/vendor/ryu/digit_table.h", "include/rinzu/vendor/ryu/digit_table.h"),
        repository_resource!("include/rinzu/vendor/ryu/ryu.h", "include/rinzu/vendor/ryu/ryu.h"),
        repository_resource!("include/rinzu/key.h", "include/rinzu/key.h"),
        repository_resource!("include/rinzu/managed_composite.h", "include/rinzu/managed_composite.h"),
        repository_resource!("include/rinzu/managed_data.h", "include/rinzu/managed_data.h"),
        repository_resource!("include/rinzu/managed_errors.h", "include/rinzu/managed_errors.h"),
        repository_resource!("include/rinzu/error_groups.h", "include/rinzu/error_groups.h"),
        repository_resource!("include/rinzu/gc_policy.h", "include/rinzu/gc_policy.h"),
        repository_resource!("include/rinzu/managed_roots.h", "include/rinzu/managed_roots.h"),
        repository_resource!("include/rinzu/native_completion.h", "include/rinzu/native_completion.h"),
        repository_resource!("include/rinzu/native_diagnostics.h", "include/rinzu/native_diagnostics.h"),
        repository_resource!("include/rinzu/native_input.h", "include/rinzu/native_input.h"),
        repository_resource!("include/rinzu/native_bytes.h", "include/rinzu/native_bytes.h"),
        repository_resource!("include/rinzu/native_opaque.h", "include/rinzu/native_opaque.h"),
        repository_resource!("include/rinzu/native_record.h", "include/rinzu/native_record.h"),
        repository_resource!("include/rinzu/native_userdata.h", "include/rinzu/native_userdata.h"),
        repository_resource!("include/rinzu/owner_dispatcher.h", "include/rinzu/owner_dispatcher.h"),
        repository_resource!("include/rinzu/owner_transition.h", "include/rinzu/owner_transition.h"),
        repository_resource!("include/rinzu/posix_write.h", "include/rinzu/posix_write.h"),
        repository_resource!("include/rinzu/ready_work.h", "include/rinzu/ready_work.h"),
        repository_resource!("include/rinzu/resource_counters.h", "include/rinzu/resource_counters.h"),
        repository_resource!("include/rinzu/resource_report.h", "include/rinzu/resource_report.h"),
        repository_resource!("include/rinzu/runtime.h", "include/rinzu/runtime.h"),
        repository_resource!("include/rinzu/scalar.h", "include/rinzu/scalar.h"),
        repository_resource!("include/rinzu/scalar_transport.h", "include/rinzu/scalar_transport.h"),
        repository_resource!("include/rinzu/source_async.h", "include/rinzu/source_async.h"),
        repository_resource!("include/rinzu/standalone_interrupt.h", "include/rinzu/standalone_interrupt.h"),
        repository_resource!("include/rinzu/supplant.h", "include/rinzu/supplant.h"),
        repository_resource!("include/rinzu/thread_identity.h", "include/rinzu/thread_identity.h"),
        repository_resource!("include/rinzu/typed_transport.h", "include/rinzu/typed_transport.h"),
        repository_resource!("include/rinzu/value.h", "include/rinzu/value.h"),
        repository_resource!("runtime/callback_entry.c", "crates/runtime/src/callback_entry.c"),
        repository_resource!("runtime/callback_return.c", "crates/runtime/src/callback_return.c"),
        repository_resource!("runtime/callback_template.S", "crates/runtime/src/callback_template.S"),
        repository_resource!("runtime/display.c", "crates/runtime/src/display.c"),
        repository_resource!("runtime/foreign_queue.c", "crates/runtime/src/foreign_queue.c"),
        repository_resource!("runtime/native_diagnostics.c", "crates/runtime/src/native_diagnostics.c"),
        repository_resource!("runtime/owner_dispatcher.c", "crates/runtime/src/owner_dispatcher.c"),
        repository_resource!("runtime/ready_work.c", "crates/runtime/src/ready_work.c"),
        repository_resource!("runtime/standalone_interrupt.c", "crates/runtime/src/standalone_interrupt.c"),
        repository_resource!("runtime/stdio.c", "crates/runtime/src/stdio.c"),
        repository_resource!("packages/io/README.md", "sdk/io/README.md"),
        repository_resource!("packages/timer/README.md", "sdk/timer/README.md"),
        repository_resource!("packages/process/README.md", "sdk/process/README.md"),
        repository_resource!("packages/stream/README.md", "sdk/stream/README.md"),
        repository_resource!("packages/testing/README.md", "sdk/testing/README.md"),
        repository_resource!("packages/shutdown/README.md", "sdk/shutdown/README.md"),
        repository_resource!("packages/http/README.md", "sdk/http/README.md"),
        repository_resource!("packages/socket/README.md", "sdk/socket/README.md"),
        ("license-inventory.json", include_bytes!("../../../docs/reference/third-party-licenses.json").as_slice()),
        sdk_resource!("io", "rinzu.toml"),
        sdk_resource!("io", "src/io.rinzu"),
        sdk_resource!("io", "native/src/io.c"),
        sdk_resource!("io", "native/include/rinzu_io.h"),
        sdk_resource!("process", "rinzu.toml"),
        sdk_resource!("process", "src/process.rinzu"),
        sdk_resource!("process", "native/process.c"),
        sdk_resource!("process", "native/process.h"),
        sdk_resource!("json", "rinzu.toml"),
        sdk_resource!("json", "README.md"),
        sdk_resource!("json", "src/json.rinzu"),
        ("packages/stream/rinzu.toml", b"[package]\nname=\"rinzu.stream\"\nlanguage=\"0.5\"\nkind=\"sdk\"\nentry=\"src/stream.rinzu\"\n".as_slice()),
        sdk_resource!("stream", "src/stream.rinzu"),
        sdk_resource!("testing", "rinzu.toml"),
        sdk_resource!("testing", "src/testing.rinzu"),
        sdk_resource!("option", "rinzu.toml"),
        sdk_resource!("option", "src/option.rinzu"),
        sdk_resource!("shutdown", "rinzu.toml"),
        sdk_resource!("shutdown", "src/shutdown.rinzu"),
        sdk_resource!("shutdown", "native/control.c"),
        sdk_resource!("shutdown", "native/control.h"),
        sdk_resource!("httpcodec", "rinzu.toml"),
        sdk_resource!("httpcodec", "README.md"),
        sdk_resource!("httpcodec", "src/httpcodec.rinzu"),
        sdk_resource!("httpcodec", "native/request_line.c"),
        sdk_resource!("httpcodec", "native/request_line.h"),
        sdk_resource!("http", "rinzu.toml"),
        sdk_resource!("http", "src/http.rinzu"),
        sdk_resource!("http", "native/src/http.c"),
        sdk_resource!("http", "native/include/rinzu_http.h"),
        sdk_resource!("socket", "rinzu.toml"),
        sdk_resource!("socket", "src/socket.rinzu"),
        sdk_resource!("socket", "native/src/socket.c"),
        sdk_resource!("socket", "native/include/rinzu_socket.h"),
    ])
}

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn registry_bytes() -> Vec<u8> {
    // This inventory describes immutable embedded bytes, not disk state.
    // Installed resources are still freshly checked by verify on every use.
    static REGISTRY: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(build_registry_bytes).clone()
}

fn build_registry_bytes() -> Vec<u8> {
    let inventory: BTreeMap<_, _> = resources()
        .into_iter()
        .map(|(name, bytes)| (name, sha256(bytes)))
        .collect();
    let packages: BTreeMap<_, _> = names()
        .map(|name| {
            (
                name,
                serde_json::json!({
                    "version": "0.1.0", "builtin": !SOURCE_PACKAGES.contains(&name),
                    "dependencies": dependencies(name),
                }),
            )
        })
        .collect();
    serde_json::to_vec_pretty(&serde_json::json!({
        "schema": "rinzu.standard-library-v1", "revision": REVISION,
        "toolchainVersion": env!("CARGO_PKG_VERSION"), "language": "0.5",
        "semantics": crate::LANGUAGE_SEMANTICS_REVISION, "target": TARGET,
        "sourcePackageAbi": format!("{}.{}",rinzu_abi::SOURCE_PACKAGE_MAJOR,rinzu_abi::SOURCE_PACKAGE_MINOR), "runtimeCodegenRevision": RUNTIME_CODEGEN_REVISION,
        "packages": packages, "files": inventory,
    }))
    .expect("static standard-library registry is serializable")
}

pub fn cache_root() -> Result<PathBuf, String> {
    let path = if let Some(path) = std::env::var_os("RINZU_CACHE_DIR") {
        PathBuf::from(path)
    } else if let Some(path) = std::env::var_os("HOME") {
        PathBuf::from(path).join("Library/Caches/rinzu")
    } else {
        return Err("set RINZU_CACHE_DIR to a writable user cache directory".into());
    };
    if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("RINZU_CACHE_DIR cannot be a symlink".into());
    }
    std::fs::create_dir_all(&path)
        .map_err(|e| format!("user cache {}: {e}; set RINZU_CACHE_DIR", path.display()))?;
    path.canonicalize().map_err(|e| e.to_string())
}

pub fn verify(root: &Path) -> Result<(), String> {
    let mut expected = resources();
    let registry = registry_bytes();
    expected.insert("registry.json", &registry);
    for (relative, bytes) in expected {
        let path = root.join(relative);
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|e| format!("standard-library resource {}: {e}", path.display()))?;
        if !metadata.file_type().is_file()
            || !path
                .canonicalize()
                .map_err(|e| e.to_string())?
                .starts_with(root)
            || std::fs::read(&path).map_err(|e| e.to_string())? != bytes
        {
            return Err(format!(
                "standard-library resource changed or invalid: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

/// Write an embedded snapshot into an owned empty directory; never repair an installed root.
pub fn write_resources(root: &Path) -> Result<(), String> {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let registry = registry_bytes();
    let mut files = resources();
    files.insert("registry.json", &registry);
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    for (name, bytes) in files {
        let path = root.join(name);
        let mut directory = root.clone();
        for part in Path::new(name).parent().unwrap().components() {
            directory.push(part);
            match std::fs::symlink_metadata(&directory) {
                Ok(m) if !m.file_type().is_dir() => {
                    return Err(format!(
                        "invalid resource directory {}",
                        directory.display()
                    ));
                }
                Ok(_) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    match std::fs::create_dir(&directory) {
                        Ok(()) => (),
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                        Err(e) => return Err(e.to_string()),
                    }
                }
                Err(e) => return Err(e.to_string()),
            }
            if !directory
                .canonicalize()
                .map_err(|e| e.to_string())?
                .starts_with(&root)
            {
                return Err("resource directory escaped its root".into());
            }
        }
        if path.exists() {
            continue;
        }
        let temporary = path.with_extension(format!(
            "tmp-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        output.write_all(bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&temporary, &path).map_err(|e| e.to_string())?;
    }
    verify(&root)
}

pub fn resource_root() -> Result<PathBuf, String> {
    if let Some(root) = std::env::var_os("RINZU_TOOLCHAIN_ROOT") {
        let root = PathBuf::from(root)
            .canonicalize()
            .map_err(|e| format!("toolchain root: {e}"))?;
        verify(&root)?;
        return Ok(root);
    }
    let adjacent = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .unwrap()
        .join("../share/rinzu");
    if adjacent.exists() {
        let root = adjacent.canonicalize().map_err(|e| e.to_string())?;
        verify(&root)?;
        return Ok(root);
    }
    let cache = cache_root()?;
    let root = cache.join("resources").join(sha256(&registry_bytes()));
    for path in [cache.join("resources"), root.clone()] {
        if std::fs::symlink_metadata(&path).is_ok_and(|m| !m.file_type().is_dir()) {
            return Err(format!(
                "invalid resource cache directory {}",
                path.display()
            ));
        }
    }
    write_resources(&root)?;
    Ok(root)
}

pub fn package_root(name: &str) -> Result<PathBuf, String> {
    if !SOURCE_PACKAGES.contains(&name) {
        return Err(format!("unregistered standard package {name}"));
    }
    Ok(resource_root()?
        .join("packages")
        .join(name.strip_prefix("rinzu.").unwrap()))
}

/// Expected distribution identity, separate from generated artifact seals.
pub fn fingerprint() -> serde_json::Value {
    serde_json::json!({"schema":"rinzu.toolchain-resource-receipt-v1", "revision": REVISION,
        "toolchainVersion": env!("CARGO_PKG_VERSION"), "standardPackageVersion":"0.1.0",
        "target": TARGET, "language":"0.5", "semantics":crate::LANGUAGE_SEMANTICS_REVISION,
        "sourcePackageAbi":format!("{}.{}",rinzu_abi::SOURCE_PACKAGE_MAJOR,rinzu_abi::SOURCE_PACKAGE_MINOR), "runtimeCodegenRevision":RUNTIME_CODEGEN_REVISION,
        "registrySha256":sha256(&registry_bytes())})
}

pub fn is_standard_manifest(manifest: &crate::Manifest) -> Result<bool, String> {
    if !SOURCE_PACKAGES.contains(&manifest.name.as_str()) {
        return Ok(false);
    }
    Ok(manifest.root.canonicalize().map_err(|e| e.to_string())? == package_root(&manifest.name)?)
}

#[cfg(all(test, unix))]
mod gc_resource_tests {
    #[test]
    fn exported_toolchain_headers_compile_and_run_the_gc_pacing_fixture() {
        use std::process::Command;
        let root = std::env::temp_dir().join(format!("rinzu-gc-resources-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        super::write_resources(&root).unwrap();
        let root = root.canonicalize().unwrap();
        super::verify(&root).unwrap();
        let binary = root.join("gc-pacing");
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../runtime/tests/fixtures/gc_pacing.c");
        let build = Command::new("clang")
            .args([
                "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread", "-I",
            ])
            .arg(root.join("include"))
            .arg(fixture)
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(
            build.status.success(),
            "{}",
            String::from_utf8_lossy(&build.stderr)
        );
        let run = Command::new(&binary).output().unwrap();
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
