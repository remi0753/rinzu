//! Compatibility wrapper for the I6 `verify-appkit` command.
//!
//! I7.5 routes execution through the ordinary SDK package and generic
//! JIT/AOT verifier. This module only adds macOS `.app` packaging.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::aot;

pub struct AppKitVerification {
    pub bundle: PathBuf,
    pub result: i64,
    pub callbacks: u64,
}

pub fn verify(package: &Path, bundle: PathBuf) -> Result<AppKitVerification, String> {
    crate::input::require_package("verify-appkit", package)?;
    rinzu_runtime::validate_v1_target().map_err(|error| error.to_string())?;
    let (manifest, _) =
        crate::input::check_package(package, &rinzu_compiler::NativeCliInputs::default())?;
    let equivalence = aot::verify_equivalence(package, None)?;
    if equivalence.outcome != "success" {
        return Err(format!(
            "AppKit SDK package did not complete successfully: {}",
            equivalence.outcome
        ));
    }
    build_bundle(package, &manifest.name, &bundle)?;
    Ok(AppKitVerification {
        bundle,
        result: equivalence.result,
        callbacks: 1,
    })
}

fn build_bundle(package: &Path, package_name: &str, bundle: &Path) -> Result<(), String> {
    let contents = bundle.join("Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");
    fs::create_dir_all(&macos)
        .map_err(|error| format!("create AppKit bundle directory: {error}"))?;
    fs::create_dir_all(&resources)
        .map_err(|error| format!("create AppKit resource directory: {error}"))?;
    let executable = macos.join(package_name);
    aot::build_package(package, Some(executable.clone()))?;
    let mut generated_resources = executable.as_os_str().to_owned();
    generated_resources.push(".resources");
    let generated_resources = PathBuf::from(generated_resources);
    let bundled_resources = resources.join(format!("{package_name}-runtime"));
    fs::rename(&generated_resources, &bundled_resources)
        .map_err(|error| format!("move AppKit runtime resources into bundle: {error}"))?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        Path::new("../Resources").join(format!("{package_name}-runtime")),
        &generated_resources,
    )
    .map_err(|error| format!("link AppKit runtime resources: {error}"))?;
    fs::write(
        contents.join("Info.plist"),
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
             \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\"><dict>\n\
             <key>CFBundleExecutable</key><string>{package_name}</string>\n\
             <key>CFBundleIdentifier</key><string>dev.rinzu.sample.appkit</string>\n\
             <key>CFBundleName</key><string>Rinzu AppKit Package Sample</string>\n\
             <key>CFBundlePackageType</key><string>APPL</string>\n\
             <key>CFBundleVersion</key><string>1</string>\n\
             <key>LSMinimumSystemVersion</key><string>13.0</string>\n\
             </dict></plist>\n"
        ),
    )
    .map_err(|error| format!("write AppKit Info.plist: {error}"))?;
    sign_bundle(bundle, &executable)
}

fn sign_bundle(bundle: &Path, executable: &Path) -> Result<(), String> {
    let signing = Command::new("codesign")
        // AOT has already signed and sealed each source/native image. Signing
        // those again with --deep would invalidate the embedded byte receipts.
        .args(["--force", "--sign", "-", "--timestamp=none"])
        .arg(bundle)
        .output()
        .map_err(|error| format!("ad-hoc sign AppKit bundle: {error}"))?;
    if !signing.status.success() {
        return Err(format!(
            "AppKit bundle ad-hoc signing failed:\n{}",
            String::from_utf8_lossy(&signing.stderr)
        ));
    }
    let source_bundle = PathBuf::from(format!("{}.source-packages", executable.display()));
    if source_bundle.exists() {
        crate::source_package_bundle::verify(&source_bundle.join("manifest.json"))?;
    }
    Ok(())
}

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
mod tests {
    use super::*;

    #[test]
    fn outer_app_signing_preserves_sealed_native_bytes() {
        let root = std::env::temp_dir().join(format!(
            "rinzu-app-signature-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _cleanup = crate::verify_capture::TemporaryDirectory(root.clone());
        let bundle = root.join("Probe.app");
        let macos = bundle.join("Contents/MacOS");
        fs::create_dir_all(&macos).unwrap();
        fs::write(root.join("native.c"), "int probe(void) { return 42; }\n").unwrap();
        fs::write(
            root.join("main.c"),
            "extern int probe(void); int main(void) { return probe() == 42 ? 0 : 1; }\n",
        )
        .unwrap();
        let library = macos.join("libprobe.dylib");
        let executable = macos.join("probe");
        let toolchain = crate::native_clang::TargetClang::resolve().unwrap();
        for command in [
            toolchain
                .command()
                .args([
                    "-dynamiclib",
                    "-install_name",
                    "@executable_path/libprobe.dylib",
                ])
                .arg(root.join("native.c"))
                .arg("-o")
                .arg(&library),
            toolchain
                .command()
                .arg(root.join("main.c"))
                .arg(&library)
                .arg("-o")
                .arg(&executable),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        fs::write(bundle.join("Contents/Info.plist"), "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>CFBundleExecutable</key><string>probe</string><key>CFBundleIdentifier</key><string>dev.rinzu.signature-test</string><key>CFBundlePackageType</key><string>APPL</string></dict></plist>").unwrap();
        let before = fs::read(&library).unwrap();
        sign_bundle(&bundle, &executable).unwrap();
        assert_eq!(fs::read(&library).unwrap(), before);
        let verify = Command::new("codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(&bundle)
            .output()
            .unwrap();
        assert!(
            verify.status.success(),
            "{}",
            String::from_utf8_lossy(&verify.stderr)
        );
        assert!(Command::new(&executable).status().unwrap().success());
    }
}
