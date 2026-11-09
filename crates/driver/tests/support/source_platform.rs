use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[path = "../../../../tests/support/process.rs"]
mod process;
// Integration targets use different subsets of the shared runners.
#[allow(unused_imports)]
pub use process::{bounded, bounded_tool};

pub fn success(output: &Output) {
    assert!(output.status.success(), "{output:?}");
}
pub fn sanitize(binary: &Path) -> PathBuf {
    let sanitized = binary.with_file_name("source-sanitized");
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    success(&bounded_tool(
        Command::new("clang")
            .args([
                "-std=c11",
                "-O1",
                "-g",
                "-Wall",
                "-Wextra",
                "-Werror",
                // Generated runtime retains labels for unreachable IR blocks.
                "-Wno-unused-label",
                "-pthread",
                "-fsanitize=address,undefined",
                "-fno-omit-frame-pointer",
            ])
            .arg(binary.with_extension("o"))
            .arg(binary.with_extension("runtime.c"))
            .arg(manifest.join("../runtime/src/callback_entry.c"))
            .arg(manifest.join("../runtime/src/callback_template.S"))
            .arg(manifest.join("../runtime/src/owner_dispatcher.c"))
            .args(["-framework", "CoreFoundation"])
            .arg("-I")
            .arg(manifest.join("../../include"))
            .arg("-o")
            .arg(&sanitized),
    ));
    for extension in ["source-packages", "resources"] {
        if !binary.with_extension(extension).exists() {
            continue;
        }
        std::os::unix::fs::symlink(
            binary.with_extension(extension),
            sanitized.with_extension(extension),
        )
        .unwrap();
    }
    sanitized
}
pub fn sign(binary: &Path, root: &Path) {
    let entitlements = root.join("adhoc-sdk.plist");
    std::fs::write(&entitlements,"<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>com.apple.security.cs.disable-library-validation</key><true/></dict></plist>").unwrap();
    success(
        &Command::new("codesign")
            .args([
                "--force",
                "--sign",
                "-",
                "--options",
                "runtime",
                "--entitlements",
            ])
            .arg(entitlements)
            .arg(binary)
            .output()
            .unwrap(),
    );
    success(
        &Command::new("codesign")
            .args(["--verify", "--strict"])
            .arg(binary)
            .output()
            .unwrap(),
    );
    let output = Command::new("codesign")
        .args(["-d", "--entitlements", "-"])
        .arg(binary)
        .output()
        .unwrap();
    success(&output);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        !text.contains("allow-jit") && !text.contains("allow-unsigned-executable-memory"),
        "{text}"
    );
}

#[allow(dead_code)]
pub fn copy_fixture(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if ["build", ".rinzu"].contains(&entry.file_name().to_str().unwrap_or("")) {
            continue;
        }
        if entry.file_type().unwrap().is_dir() {
            copy_fixture(&entry.path(), &to.join(entry.file_name()));
        } else {
            std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
        }
    }
}

#[allow(dead_code)]
pub fn stage_fixture(scratch: &Path, relative: impl AsRef<Path>) -> PathBuf {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let relative = relative.as_ref();
    let target = scratch.join(relative);
    copy_fixture(&workspace.join(relative), &target);
    target
}

#[allow(dead_code)]
pub fn ordinary_import(root: &Path) -> PathBuf {
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/programs/source-callback");
    let app = root.join("app");
    copy_fixture(&source, &app);
    app
}
