use super::{command, snapshot::write_new};
use object::{Object, ObjectSection};
use rinzu_bindgen::source_package::sha256;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const RECEIPT: &str = "Resources/BundleReceipt.json";

#[derive(Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Entry {
    File {
        bytes: u64,
        sha256: String,
        executable: bool,
    },
    Symlink {
        target: PathBuf,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt {
    schema: String,
    target: String,
    build_signing: String,
    config: super::config::Config,
    inputs: BTreeMap<String, String>,
    executable: String,
    executable_sections: BTreeMap<String, String>,
    executable_before_bundle_signing_sha256: String,
    files: BTreeMap<String, Entry>,
}

fn is_executable(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}

fn sections(bytes: &[u8]) -> Result<BTreeMap<String, String>, String> {
    let object = object::File::parse(bytes).map_err(|e| e.to_string())?;
    if object.format() != object::BinaryFormat::MachO
        || object.architecture() != object::Architecture::Aarch64
        || object.kind() != object::ObjectKind::Executable
    {
        return Err("app executable must be an arm64 Mach-O executable".into());
    }
    let mut result = BTreeMap::new();
    for section in object.sections() {
        result.insert(
            format!(
                "{}:{:?}:{}:{}",
                section.name().map_err(|e| e.to_string())?,
                section.kind(),
                section.address(),
                section.size()
            ),
            sha256(section.data().map_err(|e| e.to_string())?),
        );
    }
    Ok(result)
}

fn inventory(contents: &Path, executable: &str) -> Result<BTreeMap<String, Entry>, String> {
    fn walk(
        root: &Path,
        path: &Path,
        executable: &str,
        files: &mut BTreeMap<String, Entry>,
    ) -> Result<(), String> {
        let mut entries = fs::read_dir(path)
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .map_err(|e| e.to_string())?
                .to_str()
                .ok_or("bundle paths must be UTF-8")?
                .to_owned();
            if relative == "_CodeSignature" || relative == RECEIPT || relative == executable {
                continue;
            }
            let metadata = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            if metadata.is_symlink() {
                let target = fs::read_link(&path).map_err(|e| e.to_string())?;
                if target.is_absolute()
                    || !path
                        .canonicalize()
                        .map_err(|e| e.to_string())?
                        .starts_with(root)
                {
                    return Err("app symlink escapes Contents".into());
                }
                files.insert(relative, Entry::Symlink { target });
            } else if metadata.is_dir() {
                walk(root, &path, executable, files)?;
            } else if metadata.is_file() {
                files.insert(
                    relative,
                    Entry::File {
                        bytes: metadata.len(),
                        sha256: sha256(&fs::read(path).map_err(|e| e.to_string())?),
                        executable: is_executable(&metadata),
                    },
                );
            } else {
                return Err("unsupported bundle entry type".into());
            }
        }
        Ok(())
    }
    let root = contents.canonicalize().map_err(|e| e.to_string())?;
    let mut result = BTreeMap::new();
    walk(&root, &root, executable, &mut result)?;
    Ok(result)
}

pub fn save(
    contents: &Path,
    executable: &str,
    config: super::config::Config,
    inputs: BTreeMap<String, String>,
) -> Result<(), String> {
    let bytes = fs::read(contents.join(executable)).map_err(|e| e.to_string())?;
    let receipt = Receipt {
        schema: "rinzu.macos-app-receipt-v1".into(),
        target: rinzu_runtime::current_target().into(),
        build_signing: "ad-hoc".into(),
        config,
        inputs,
        executable: executable.into(),
        executable_sections: sections(&bytes)?,
        executable_before_bundle_signing_sha256: sha256(&bytes),
        files: inventory(contents, executable)?,
    };
    write_new(
        &contents.join(RECEIPT),
        &serde_json::to_vec_pretty(&receipt).map_err(|e| e.to_string())?,
    )
}

pub fn verify(bundle: &Path) -> Result<(), String> {
    let contents = bundle
        .join("Contents")
        .canonicalize()
        .map_err(|e| format!("read app Contents: {e}"))?;
    let receipt_bytes = super::snapshot::read(&contents, Path::new(RECEIPT), 8 * 1024 * 1024)?;
    let receipt: Receipt =
        serde_json::from_slice(&receipt_bytes).map_err(|e| format!("invalid app receipt: {e}"))?;
    if receipt.schema != "rinzu.macos-app-receipt-v1"
        || receipt.target != rinzu_runtime::current_target()
    {
        return Err("unsupported app receipt schema or target".into());
    }
    super::config::relative(Path::new(&receipt.executable))?;
    if !receipt.executable.starts_with("MacOS/")
        || Path::new(&receipt.executable).components().count() != 2
    {
        return Err("invalid app executable location".into());
    }
    let executable = super::snapshot::confined(&contents, Path::new(&receipt.executable))?;
    if !is_executable(&fs::metadata(&executable).map_err(|e| e.to_string())?)
        || sections(&fs::read(executable).map_err(|e| e.to_string())?)?
            != receipt.executable_sections
    {
        return Err("app executable sections or permissions changed".into());
    }
    if inventory(&contents, &receipt.executable)? != receipt.files {
        return Err("app file inventory changed (missing, extra, modified file or link)".into());
    }
    command(
        Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(bundle),
    )?;
    Ok(())
}
