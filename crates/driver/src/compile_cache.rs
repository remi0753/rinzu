//! Cache native compiler output by the freshly preprocessed translation unit.
//! Preprocessing still runs on every lookup, including include search and macros.
use crate::native_clang::TargetClang;
use rinzu_bindgen::source_package::sha256;
use serde::{Deserialize, Serialize};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema: String,
    key: String,
    digest: String,
}

fn directory(key: &str) -> Result<PathBuf, String> {
    let root = rinzu_compiler::standard_library::cache_root()?;
    let mut path = root.clone();
    for part in ["native-compile-v1", key] {
        path.push(part);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_dir() => (),
            Ok(_) => return Err("native compilation cache path is not a directory".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match std::fs::create_dir(&path) {
                    Ok(()) => (),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
                    Err(error) => return Err(error.to_string()),
                }
            }
            Err(error) => return Err(error.to_string()),
        }
        if !std::fs::symlink_metadata(&path)
            .map_err(|error| error.to_string())?
            .file_type()
            .is_dir()
            || !path
                .canonicalize()
                .map_err(|error| error.to_string())?
                .starts_with(&root)
        {
            return Err("native compilation cache escaped its root".into());
        }
    }
    Ok(path)
}

fn read_file(path: &Path) -> Option<Vec<u8>> {
    std::fs::symlink_metadata(path)
        .ok()
        .filter(|metadata| metadata.file_type().is_file())?;
    std::fs::read(path).ok()
}

fn restore(directory: &Path, key: &str, output: &Path) -> Option<()> {
    let receipt: Receipt =
        serde_json::from_slice(&read_file(&directory.join("receipt.json"))?).ok()?;
    if receipt.schema != "rinzu.native-compile-v1" || receipt.key != key {
        return None;
    }
    let bytes = read_file(&directory.join("artifact"))?;
    if sha256(&bytes) != receipt.digest {
        return None;
    }
    std::fs::write(output, bytes).ok()
}

fn replace(directory: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let temporary = directory.join(format!(
        ".pending-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        file.write_all(bytes).map_err(|error| error.to_string())?;
        std::fs::rename(&temporary, directory.join(name)).map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn store(directory: &Path, key: &str, output: &Path) -> Result<(), String> {
    let bytes = std::fs::read(output).map_err(|error| error.to_string())?;
    let receipt = Receipt {
        schema: "rinzu.native-compile-v1".into(),
        key: key.into(),
        digest: sha256(&bytes),
    };
    replace(directory, "artifact", &bytes)?;
    replace(
        directory,
        "receipt.json",
        &serde_json::to_vec(&receipt).map_err(|error| error.to_string())?,
    )
}

fn preprocessor(command: &Command, dependencies: &Path) -> Command {
    let mut query = Command::new(command.get_program());
    let mut arguments = command.get_args();
    while let Some(argument) = arguments.next() {
        if argument == "-o" {
            arguments.next();
        } else if !matches!(argument.to_str(), Some("-c" | "-S" | "-emit-llvm")) {
            query.arg(argument);
        }
    }
    query.envs(
        command
            .get_envs()
            .filter_map(|(key, value)| value.map(|value| (key, value))),
    );
    for (key, value) in command.get_envs() {
        if value.is_none() {
            query.env_remove(key);
        }
    }
    if let Some(path) = command.get_current_dir() {
        query.current_dir(path);
    }
    // Date/time expansions are valid source but cannot be reused. The original
    // compile below retains its normal diagnostics and behavior in that case.
    query
        .args([
            "-E",
            "-P",
            "-Werror=date-time",
            "-MD",
            "-MT",
            "rinzu",
            "-MF",
        ])
        .arg(dependencies)
        .args(["-o", "-"]);
    query
}

pub fn compile(
    toolchain: &TargetClang,
    mut command: Command,
    generated: &Path,
    output: &Path,
    source: &Path,
    action: &str,
) -> Result<(), String> {
    let _timing = rinzu_compiler::timing::span("native.compile").detail(action);
    // Generated-directory names must not become embedded __FILE__ or debug
    // paths in a reusable object. User source paths retain their identities.
    command.arg(format!(
        "-ffile-prefix-map={}=/rinzu-generated",
        generated.display()
    ));
    let cache = (|| {
        let preprocess_timing = rinzu_compiler::timing::span("native.compile.preprocess");
        let mut original_arguments = command.get_args();
        let dependencies = original_arguments
            .find(|argument| *argument == "-MF")
            .and_then(|_| original_arguments.next())
            .map(PathBuf::from)
            .unwrap_or_else(|| output.with_extension("cache.d"));
        let preprocessed = preprocessor(&command, &dependencies).output().ok()?;
        drop(preprocess_timing);
        if !preprocessed.status.success() {
            return None;
        }
        // -P removes line directives so generated temporary paths do not
        // invalidate the cache. Also hash the original inputs: compiler
        // builtins such as __builtin_LINE and C++ source_location observe
        // whitespace/line directives that token-only preprocessing would lose.
        let mut input_files = std::collections::BTreeMap::new();
        for path in crate::native_fingerprint::dependency_paths(
            &std::fs::read_to_string(&dependencies).ok()?,
        )
        .ok()?
        {
            let identity = path.strip_prefix(generated).map_or_else(
                |_| path.display().to_string(),
                |relative| format!("@generated/{}", relative.display()),
            );
            input_files.insert(identity, sha256(&std::fs::read(path).ok()?));
        }
        if input_files.is_empty() {
            return None;
        }
        let arguments: Vec<_> = command
            .get_args()
            .map(|argument| {
                argument
                    .to_string_lossy()
                    .replace(generated.to_string_lossy().as_ref(), "@generated")
            })
            .collect();
        let mut environment: std::collections::BTreeMap<_, _> = std::env::vars_os()
            .filter(|(key, _)| !key.as_bytes().starts_with(b"RINZU_"))
            .map(|(key, value)| (key.as_bytes().to_vec(), Some(value.as_bytes().to_vec())))
            .collect();
        for (key, value) in command.get_envs() {
            environment.insert(
                key.as_bytes().to_vec(),
                value.map(|value| value.as_bytes().to_vec()),
            );
        }
        let cwd = command
            .get_current_dir()
            .map(Path::to_owned)
            .or_else(|| std::env::current_dir().ok())?;
        let environment: Vec<_> = environment.into_iter().collect();
        let identity = serde_json::to_vec(&(
            "rinzu.native-compile-v1",
            &toolchain.fingerprint,
            arguments,
            command.get_program().as_bytes(),
            cwd,
            environment,
            sha256(&preprocessed.stdout),
            input_files,
        ))
        .ok()?;
        let key = sha256(&identity);
        Some((directory(&key).ok()?, key))
    })();
    if let Some((directory, key)) = &cache
        && restore(directory, key, output).is_some()
    {
        let _hit = rinzu_compiler::timing::span("native.compile.hit").detail(action);
        return Ok(());
    }
    let compile_timing = rinzu_compiler::timing::span("native.compile.clang").detail(action);
    crate::cli_diagnostics::tool_output(
        command.output(),
        Some(source),
        "native compilation",
        "clang",
    )?;
    drop(compile_timing);
    if let Some((directory, key)) = cache {
        // Cache failures never prevent a successful compilation.
        let _ = store(&directory, &key, output);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrupt_mixed_and_symlink_artifacts_do_not_restore() {
        let root =
            std::env::temp_dir().join(format!("rinzu-compile-cache-unit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let output = root.join("output.o");
        std::fs::write(&output, b"first").unwrap();
        store(&root, "key", &output).unwrap();
        assert!(restore(&root, "key", &output).is_some());
        assert!(restore(&root, "another-key", &output).is_none());
        std::fs::write(root.join("artifact"), b"mixed generation").unwrap();
        assert!(restore(&root, "key", &output).is_none());
        store(&root, "key", &output).unwrap();
        std::fs::remove_file(root.join("artifact")).unwrap();
        std::os::unix::fs::symlink(&output, root.join("artifact")).unwrap();
        assert!(restore(&root, "key", &output).is_none());
        store(&root, "key", &output).unwrap();
        std::fs::write(root.join("receipt.json"), b"{broken").unwrap();
        assert!(restore(&root, "key", &output).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preprocessing_keeps_flags_and_dependencies_without_compiling() {
        let mut command = Command::new("clang");
        command.args([
            "-c",
            "-O2",
            "-MD",
            "-MF",
            "input.d",
            "-S",
            "-emit-llvm",
            "input.c",
            "-o",
            "output.ll",
        ]);
        let query = preprocessor(&command, Path::new("cache.d"));
        let args: Vec<_> = query
            .get_args()
            .map(|argument| argument.to_owned())
            .collect();
        assert!(args.contains(&"-O2".into()) && args.contains(&"input.d".into()));
        assert!(!args.contains(&"-c".into()) && !args.contains(&"output.ll".into()));
        assert!(args.contains(&"-E".into()) && args.contains(&"-Werror=date-time".into()));
    }
}
