//! A relocatable toolchain snapshot. Release signing/installation remains a separate gate.
use std::path::{Path, PathBuf};

pub fn build_inputs() -> Result<std::collections::BTreeMap<String, String>, String> {
    let fingerprint = rinzu_compiler::standard_library::fingerprint();
    if fingerprint["sourcePackageAbi"]
        != format!(
            "{}.{}",
            rinzu_abi::SOURCE_PACKAGE_MAJOR,
            rinzu_abi::SOURCE_PACKAGE_MINOR
        )
        || !rinzu_codegen::BASELINE_RUNTIME_ABI_REVISION
            .ends_with(&format!("-r{}", fingerprint["runtimeCodegenRevision"]))
    {
        return Err("standard resource target/ABI revision does not match this driver".into());
    }
    let root = rinzu_compiler::standard_library::resource_root()?;
    Ok(std::collections::BTreeMap::from([
        (
            "rinzu.toolchain-resource-root".into(),
            root.display().to_string(),
        ),
        (
            "rinzu.toolchain-resource-receipt".into(),
            fingerprint.to_string(),
        ),
    ]))
}

pub fn run(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    if arguments.next().as_deref() != Some("--output") {
        return Err("usage: toolchain-bundle --output directory".into());
    }
    let output = PathBuf::from(arguments.next().ok_or("--output requires a directory")?);
    if arguments.next().is_some() {
        return Err("toolchain-bundle takes only --output directory".into());
    }
    export(&output)?;
    println!("toolchain bundle: {}", output.display());
    Ok(())
}

fn export(output: &Path) -> Result<(), String> {
    if std::fs::symlink_metadata(output).is_ok() {
        return Err("toolchain bundle output already exists; choose a new directory".into());
    }
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if !parent.is_dir() {
        return Err("toolchain bundle parent directory must exist".into());
    }
    std::fs::create_dir(output).map_err(|e| e.to_string())?;
    let result = (|| {
        let bin = output.join("bin");
        std::fs::create_dir(&bin).map_err(|e| e.to_string())?;
        let source = std::env::current_exe().map_err(|e| e.to_string())?;
        std::fs::copy(&source, bin.join("rinzu")).map_err(|e| e.to_string())?;
        let bytes = std::fs::read(&source).map_err(|e| e.to_string())?;
        if std::fs::read(bin.join("rinzu")).map_err(|e| e.to_string())? != bytes {
            return Err("toolchain binary changed while copying".into());
        }
        let resources = output.join("share/rinzu");
        rinzu_compiler::standard_library::write_resources(&resources)?;
        let metadata = serde_json::json!({
            "schema": "rinzu.toolchain-bundle-v1", "toolchainVersion": env!("CARGO_PKG_VERSION"),
            "target": rinzu_compiler::standard_library::TARGET,
            "binary": "bin/rinzu", "binarySha256": rinzu_compiler::standard_library::sha256(&bytes),
            "resources": "share/rinzu", "registrySha256": rinzu_compiler::standard_library::sha256(&rinzu_compiler::standard_library::registry_bytes()),
            "licenseInventory": "share/rinzu/license-inventory.json",
            "signedInstaller": false,
        });
        std::fs::write(
            output.join("bundle.json"),
            serde_json::to_vec_pretty(&metadata).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(output);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn output_refuses_existing_directories_and_invalid_arguments() {
        assert!(
            export(&std::env::temp_dir())
                .unwrap_err()
                .contains("already exists")
        );
        assert!(
            run(["--wrong".to_owned()].into_iter())
                .unwrap_err()
                .contains("usage")
        );
        assert!(
            run(["--output".to_owned()].into_iter())
                .unwrap_err()
                .contains("requires")
        );
    }
}
