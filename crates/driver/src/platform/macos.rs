use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use rinzu_abi::target::Target;

use crate::native_clang::TargetClang;

pub(crate) fn clang_target() -> Result<String, String> {
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return Ok("arm64-apple-macos11".into());
    }
    let version = match std::env::var("MACOSX_DEPLOYMENT_TARGET") {
        Ok(version) => version,
        Err(std::env::VarError::NotPresent) => {
            let output = Command::new("/usr/bin/sw_vers")
                .arg("-productVersion")
                .output()
                .map_err(|error| format!("read host macOS version: {error}"))?;
            if !output.status.success() {
                return Err("cannot read host macOS version".into());
            }
            String::from_utf8(output.stdout).map_err(|error| error.to_string())?
        }
        Err(error) => return Err(format!("read MACOSX_DEPLOYMENT_TARGET: {error}")),
    };
    macos_target(version.trim())
}

fn macos_target(version: &str) -> Result<String, String> {
    let components: Vec<_> = version.split('.').collect();
    if components.len() > 3
        || !components
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        || components[0]
            .parse::<u16>()
            .ok()
            .is_none_or(|major| major < 11)
        || components[1..]
            .iter()
            .any(|part| part.parse::<u8>().is_err())
    {
        return Err(format!(
            "invalid macOS deployment version `{version}`; expected macOS 11 or later"
        ));
    }
    Ok(format!("arm64-apple-macos{version}"))
}

fn query(arguments: &[&str]) -> Result<String, String> {
    let output = Command::new("xcrun")
        .args(arguments)
        .output()
        .map_err(|error| format!("resolve Xcode SDK input: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "resolve Xcode SDK input: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout)
        .map(|text| text.trim().to_owned())
        .map_err(|error| error.to_string())
}

pub(crate) fn sysroot() -> Result<PathBuf, String> {
    PathBuf::from(query(&["--sdk", "macosx", "--show-sdk-path"])?)
        .canonicalize()
        .map_err(|error| error.to_string())
}

pub(crate) fn resolve_clang() -> Result<TargetClang, String> {
    let executable = PathBuf::from(query(&["--sdk", "macosx", "--find", "clang"])?)
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let sysroot = sysroot()?;
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
        ("clang.target".to_owned(), clang_target()?),
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
    Ok(TargetClang {
        executable,
        sysroot,
        fingerprint,
        target: Target::Aarch64AppleDarwin,
    })
}

pub(crate) fn configure_clang(toolchain: &TargetClang, command: &mut Command) {
    command
        .arg("-isysroot")
        .arg(&toolchain.sysroot)
        .env_remove("MACOSX_DEPLOYMENT_TARGET");
}

#[cfg(test)]
mod tests {
    #[test]
    fn macos_deployment_version_is_a_single_supported_clang_target() {
        for version in ["11", "26.0", "26.6.2"] {
            assert_eq!(
                super::macos_target(version).unwrap(),
                format!("arm64-apple-macos{version}")
            );
        }
        for version in ["", "10.15", "26..0", "26.256", "26.0.1.2", "26 -flag"] {
            assert!(super::macos_target(version).is_err(), "{version}");
        }
    }
}
