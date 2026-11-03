//! Transactional local macOS app bundles. Developer ID/notarization is a separate release gate.
mod config;
mod icon;
mod receipt;
mod snapshot;
#[cfg(test)]
mod tests;

use crate::app_layout::Layout;
use rinzu_bindgen::source_package::sha256;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn command(command: &mut Command) -> Result<String, String> {
    let output = command
        .output()
        .map_err(|e| format!("start {command:?}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "{command:?} failed:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| e.to_string())
}

pub fn run(arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let parsed = crate::native_cli::parse("bundle", arguments)?;
    let path = parsed
        .path
        .unwrap_or(std::env::current_dir().map_err(|e| e.to_string())?);
    crate::input::require_package("bundle", &path)?;
    let context = crate::input::check(&path, &parsed.input, &parsed.native)?;
    let bundle = build(&context, parsed.output, parsed.app_config)?;
    println!("application bundle: {}", bundle.display());
    println!(
        "executable: {}",
        bundle
            .join("Contents/MacOS")
            .join(&context.metadata.name)
            .display()
    );
    println!(
        "receipt: {}",
        bundle.join("Contents").join(receipt::RECEIPT).display()
    );
    println!("bundle signing: ad-hoc");
    Ok(())
}

pub fn verify(arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let paths = arguments.collect::<Vec<_>>();
    if paths.len() != 1 {
        return Err("usage: bundle-verify path.app".into());
    }
    receipt::verify(Path::new(&paths[0]))?;
    println!("application bundle verified: {}", paths[0]);
    Ok(())
}

pub fn build(
    context: &rinzu_compiler::CompilationContext,
    output: Option<PathBuf>,
    config_path: Option<PathBuf>,
) -> Result<PathBuf, String> {
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return Err("app bundles require macOS arm64".into());
    }
    if context.artifact.is_file_input
        || context.metadata.kind != rinzu_compiler::PackageKind::Application
        || context.metadata.language != "0.5"
    {
        return Err("bundle requires a language 0.5 application package".into());
    }
    let root = context
        .metadata
        .root
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let mut source_inputs = crate::source_receipt::InputSnapshot::checked(
        &context.artifact.checked_inputs,
        &context.artifact.module_graph,
    )?;
    source_inputs.native(&context.metadata, &context.artifact.native_inputs)?;
    for package in &context.artifact.sdk_packages {
        source_inputs.native(
            &package.manifest,
            &rinzu_compiler::NativeCliInputs::default(),
        )?;
    }
    let config_path = config_path.unwrap_or_else(|| "macos-app.json".into());
    let config_bytes = snapshot::read(&root, &config_path, 65536)?;
    let config = config::Config::parse(&config_bytes)?;
    let inputs = snapshot::Snapshot::capture(&root, &config_path, &config_bytes, &config)?;
    let output = std::path::absolute(
        output.unwrap_or_else(|| root.join("build").join(format!("{}.app", config.name))),
    )
    .map_err(|e| e.to_string())?;
    if output.extension().is_none_or(|s| s != "app") {
        return Err("bundle output must end in .app".into());
    }
    if fs::symlink_metadata(&output).is_ok() {
        return Err("app bundle output already exists; choose a new path".into());
    }
    crate::input::protect_outputs(context, std::slice::from_ref(&output))?;
    // An output inside a declared resource root would include its own build and invalidate the snapshot.
    let identity = crate::package_commands::output_identity(&output)?;
    for path in config
        .resources
        .iter()
        .chain(config.icon.iter())
        .chain(std::iter::once(&config_path))
    {
        let input = snapshot::confined(&root, path)?;
        if identity.starts_with(&input) || input.starts_with(&identity) {
            return Err("app output collides with bundle inputs".into());
        }
    }
    let parent = output.parent().ok_or("app output has no parent")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let work = parent.join(format!(
        ".rinzu-app-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos()
    ));
    fs::create_dir(&work).map_err(|e| e.to_string())?;
    let _cleanup = crate::verify_capture::TemporaryDirectory(work.clone());
    let bundle = work.join("Application.app");
    let contents = bundle.join("Contents");
    let resources = contents.join("Resources");
    for path in [
        contents.join("MacOS"),
        contents.join("Frameworks"),
        resources.join("app"),
    ] {
        fs::create_dir_all(path).map_err(|e| e.to_string())?;
    }
    snapshot::write_new(
        &contents.join("Info.plist"),
        config.plist(&context.metadata.name).as_bytes(),
    )?;
    command(
        Command::new("/usr/bin/plutil")
            .arg("-lint")
            .arg(contents.join("Info.plist")),
    )?;
    if let (Some(path), Some(bytes)) = (&config.icon, &inputs.icon) {
        icon::generate(bytes, path, &work, &resources)?;
    }
    for path in &inputs.directories {
        fs::create_dir_all(resources.join("app").join(path)).map_err(|e| e.to_string())?;
    }
    for (path, bytes) in &inputs.files {
        snapshot::write_new(&resources.join("app").join(path), bytes)?;
    }
    let built =
        crate::aot::build_checked_app(context, work.join("aot").join(&context.metadata.name))?;
    let executable = contents.join("MacOS").join(&context.metadata.name);
    fs::copy(&built.executable, &executable).map_err(|e| e.to_string())?;
    snapshot::write_new(
        &resources.join("AOT.json"),
        &fs::read(&built.metadata).map_err(|e| e.to_string())?,
    )?;
    let sidecar = crate::aot::with_suffix(&built.executable, ".source-packages");
    if sidecar.exists() {
        crate::source_package_bundle::verify(&sidecar.join("manifest.json"))?;
        copy_source_packages(&sidecar, &sidecar, &contents)?;
    }
    let runtime = crate::aot::with_suffix(&built.executable, ".resources");
    if runtime.exists() {
        fs::rename(&runtime, resources.join("Runtime")).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            "../Resources/Runtime",
            crate::aot::with_suffix(&executable, ".resources"),
        )
        .map_err(|e| e.to_string())?;
    }
    // Retained versioned native packages link through @executable_path; keep their code in MacOS.
    for entry in fs::read_dir(built.executable.parent().unwrap()).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.path().extension().is_some_and(|s| s == "dylib") {
            fs::copy(entry.path(), contents.join("MacOS").join(entry.file_name()))
                .map_err(|e| e.to_string())?;
        }
    }
    let executable_relative = format!("MacOS/{}", context.metadata.name);
    receipt::save(
        &contents,
        &executable_relative,
        config.clone(),
        inputs.hashes(),
    )?;
    // Never use --deep to sign: provider/facade hashes are sealed into the AOT startup.
    command(
        Command::new("/usr/bin/codesign")
            .args(["--force", "--sign", "-", "--timestamp=none"])
            .arg(&bundle),
    )?;
    receipt::verify(&bundle)?;
    inputs.verify(&root, &config_path, &config_bytes, &config)?;
    source_inputs.verify()?;
    publish(&bundle, &output)?;
    Ok(output)
}

fn copy_source_packages(root: &Path, directory: &Path, contents: &Path) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_symlink() {
            return Err("generated source package contains an unexpected symlink".into());
        }
        if kind.is_dir() {
            copy_source_packages(root, &entry.path(), contents)?;
        } else if kind.is_file() {
            let relative = entry
                .path()
                .strip_prefix(root)
                .map_err(|e| e.to_string())?
                .to_str()
                .ok_or("source-package paths must be UTF-8")?
                .to_owned();
            let target = contents.join(Layout::MacosApp.file(&relative)?);
            let bytes = fs::read(entry.path()).map_err(|e| e.to_string())?;
            if target.exists() {
                if sha256(&fs::read(&target).map_err(|e| e.to_string())?) != sha256(&bytes) {
                    return Err("deployed native filename collision".into());
                }
            } else if target.starts_with(contents.join("Frameworks")) {
                fs::copy(entry.path(), &target).map_err(|e| e.to_string())?;
            } else {
                snapshot::write_new(&target, &bytes)?;
            }
        } else {
            return Err("unexpected generated source-package entry".into());
        }
    }
    Ok(())
}

fn publish(source: &Path, destination: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let source =
            std::ffi::CString::new(source.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
        let destination = std::ffi::CString::new(destination.as_os_str().as_bytes())
            .map_err(|e| e.to_string())?;
        // SAFETY: both paths are live NUL-terminated strings; RENAME_EXCL refuses all existing destinations.
        if unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) }
            != 0
        {
            return Err(format!(
                "publish app without replacing existing output: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (source, destination);
        Err("app bundle publication requires macOS".into())
    }
}
