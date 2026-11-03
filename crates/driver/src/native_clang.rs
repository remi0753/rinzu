use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

pub const TARGET: &str = "arm64-apple-macos11";

#[derive(Clone, Debug)]
pub struct TargetClang {
    pub executable: PathBuf,
    pub sysroot: PathBuf,
    pub fingerprint: BTreeMap<String, String>,
}

impl TargetClang {
    pub fn resolve() -> Result<Self, String> {
        let _timing = rinzu_compiler::timing::span("native.toolchain");
        if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            return Err("source native ABI requires the initial macOS arm64 target".to_owned());
        }
        fn query(arguments: &[&str]) -> Result<String, String> {
            let output = Command::new("xcrun")
                .args(arguments)
                .output()
                .map_err(|error| format!("resolve target Clang: {error}"))?;
            if !output.status.success() {
                return Err(format!(
                    "resolve target Clang failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            String::from_utf8(output.stdout)
                .map(|text| text.trim().to_owned())
                .map_err(|error| error.to_string())
        }
        let executable = PathBuf::from(query(&["--sdk", "macosx", "--find", "clang"])?)
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let sysroot = PathBuf::from(query(&["--sdk", "macosx", "--show-sdk-path"])?)
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let version = Command::new(&executable)
            .arg("--version")
            .output()
            .map_err(|error| error.to_string())?;
        if !version.status.success() {
            return Err("cannot read target Clang version".to_owned());
        }
        let sdk_settings = sysroot.join("SDKSettings.json");
        let os = Command::new("sw_vers")
            .arg("-buildVersion")
            .output()
            .map_err(|error| format!("read OS build: {error}"))?;
        let os_build = String::from_utf8(os.stdout).map_err(|error| error.to_string())?;
        if !os.status.success() || os_build.trim().is_empty() {
            return Err("cannot read OS build identity".into());
        }
        let mut fingerprint = BTreeMap::from([
            ("target.os-build".to_owned(), os_build.trim().to_owned()),
            (
                "clang.identity".to_owned(),
                executable.to_string_lossy().into_owned(),
            ),
            (
                "clang.version".to_owned(),
                String::from_utf8(version.stdout).map_err(|error| error.to_string())?,
            ),
            (
                "clang.binary".to_owned(),
                rinzu_bindgen::source_package::sha256(
                    &std::fs::read(&executable).map_err(|error| error.to_string())?,
                ),
            ),
            ("clang.target".to_owned(), TARGET.to_owned()),
            (
                "clang.sysroot".to_owned(),
                sysroot.to_string_lossy().into_owned(),
            ),
            (
                "clang.sdk".to_owned(),
                rinzu_bindgen::source_package::sha256(
                    &std::fs::read(sdk_settings).map_err(|error| error.to_string())?,
                ),
            ),
        ]);
        for name in ["ld", "dyld_info"] {
            let path = PathBuf::from(query(&["--sdk", "macosx", "--find", name])?)
                .canonicalize()
                .map_err(|error| error.to_string())?;
            fingerprint.insert(
                format!("tool.identity:{name}"),
                path.to_string_lossy().into_owned(),
            );
            fingerprint.insert(
                format!("tool.binary:{name}"),
                rinzu_bindgen::source_package::sha256(
                    &std::fs::read(path).map_err(|error| error.to_string())?,
                ),
            );
        }
        Ok(Self {
            executable,
            sysroot,
            fingerprint,
        })
    }

    pub fn verify(&self) -> Result<(), String> {
        if Self::resolve()?.fingerprint != self.fingerprint {
            return Err(
                "native toolchain/target/SDK changed before publication; check again".into(),
            );
        }
        Ok(())
    }

    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        command
            .arg("-target")
            .arg(TARGET)
            .arg("-isysroot")
            .arg(&self.sysroot);
        command
    }
}
