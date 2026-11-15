use crate::native_clang::TargetClang;
use rinzu_abi::target::Target;
use rinzu_bindgen::source_package::sha256;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn find_on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

fn query(executable: &Path, arguments: &[&str]) -> Result<String, String> {
    let output = Command::new(executable)
        .args(arguments)
        .output()
        .map_err(|error| format!("start {}: {error}", executable.display()))?;
    if !output.status.success() {
        return Err(format!(
            "{}: {}",
            executable.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout)
        .map(|text| text.trim().to_owned())
        .map_err(|error| error.to_string())
}

fn file_digest(path: &Path) -> Result<String, String> {
    std::fs::read(path)
        .map(|bytes| sha256(&bytes))
        .map_err(|error| format!("read Linux toolchain input {}: {error}", path.display()))
}

pub(crate) fn resolve_clang() -> Result<TargetClang, String> {
    let target = Target::X86_64UnknownLinuxGnu;
    let executable = std::env::var_os("RINZU_CLANG")
        .map(PathBuf::from)
        .or_else(|| find_on_path("clang"))
        .ok_or("install Clang or set RINZU_CLANG to build Linux executables")?
        .canonicalize()
        .map_err(|error| format!("resolve Linux Clang: {error}"))?;
    let sysroot = std::env::var_os("RINZU_SYSROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
        .canonicalize()
        .map_err(|error| format!("resolve Linux sysroot: {error}"))?;
    if !sysroot.is_dir() {
        return Err(format!(
            "Linux sysroot must be a directory: {}",
            sysroot.display()
        ));
    }
    let linker = query(
        &executable,
        &["-target", target.triple(), "-print-prog-name=ld"],
    )?;
    let linker = if Path::new(&linker).is_absolute() {
        PathBuf::from(linker)
    } else {
        find_on_path(&linker).ok_or("install an ELF linker to build Linux executables")?
    };
    let linker_path = std::path::absolute(&linker).map_err(|error| error.to_string())?;
    let linker_binary = linker
        .canonicalize()
        .map_err(|error| format!("resolve ELF linker: {error}"))?;
    let mut fingerprint = BTreeMap::from([
        ("clang.identity".into(), executable.display().to_string()),
        ("clang.binary".into(), file_digest(&executable)?),
        ("clang.version".into(), query(&executable, &["--version"])?),
        ("clang.target".into(), target.triple().into()),
        ("clang.sysroot".into(), sysroot.display().to_string()),
        ("clang.cflags".into(), "-D_GNU_SOURCE -march=x86-64".into()),
        (
            "clang.aot-flags".into(),
            "-fPIE -pie -pthread -Wl,-z,noexecstack -lm".into(),
        ),
        (
            "tool.identity:ld".into(),
            linker_binary.display().to_string(),
        ),
        ("tool.binary:ld".into(), file_digest(&linker_binary)?),
        ("clang.linker".into(), linker_path.display().to_string()),
    ]);
    let sysroot_flag = format!("--sysroot={}", sysroot.display());
    for file in [
        "Scrt1.o",
        "crti.o",
        "crtbeginS.o",
        "crtendS.o",
        "crtn.o",
        "libc.so",
        "libc.so.6",
        "libc_nonshared.a",
        "libgcc.a",
        "libgcc_s.so",
        "libgcc_s.so.1",
        "libm.so",
        "libm.so.6",
    ] {
        let flag = format!("-print-file-name={file}");
        let path = query(
            &executable,
            &["-target", target.triple(), &sysroot_flag, &flag],
        )?;
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err(format!(
                "Linux C development toolchain could not resolve `{file}`"
            ));
        }
        let path = path.canonicalize().map_err(|error| {
            format!("resolve Linux startup or library input `{file}`: {error}; install the target C development toolchain")
        })?;
        fingerprint.insert(
            format!("link:external:{}", path.display()),
            file_digest(&path)?,
        );
    }
    Ok(TargetClang {
        executable,
        sysroot,
        fingerprint,
        target,
    })
}

pub(crate) fn configure_clang(toolchain: &TargetClang, command: &mut Command) {
    command
        .arg("--sysroot")
        .arg(&toolchain.sysroot)
        .args(["-D_GNU_SOURCE", "-march=x86-64"]);
}

pub(crate) fn configure_executable(toolchain: &TargetClang, command: &mut Command) {
    command
        .arg(format!(
            "--ld-path={}",
            toolchain.fingerprint["clang.linker"]
        ))
        .args(["-fPIE", "-pie", "-pthread", "-Wl,-z,noexecstack"]);
}

pub(crate) fn link_runtime_libraries(command: &mut Command) {
    command.arg("-lm");
}
