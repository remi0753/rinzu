use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

pub fn bounded(command: &mut Command) -> Output {
    bounded_for(command, 60)
}
pub fn bounded_tool(command: &mut Command) -> Output {
    bounded_for(command, 300)
}
fn bounded_for(command: &mut Command, seconds: u64) -> Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let read = |mut pipe: Box<dyn std::io::Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes).unwrap();
            bytes
        })
    };
    let out = read(Box::new(stdout));
    let err = read(Box::new(stderr));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            break child.wait().unwrap();
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    Output {
        status,
        stdout: out.join().unwrap(),
        stderr: err.join().unwrap(),
    }
}
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
pub fn ordinary_import(root: &Path) -> PathBuf {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            if ["build", ".rinzu"].contains(&entry.file_name().to_str().unwrap_or("")) {
                continue;
            }
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &to.join(entry.file_name()));
            } else {
                std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
            }
        }
    }
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/programs/source-callback");
    let app = root.join("app");
    copy(&source, &app);
    app
}
