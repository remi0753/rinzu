use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug)]
pub struct ConfigToolOutput {
    pub flags: Vec<String>,
    pub fingerprint: BTreeMap<String, String>,
}

pub fn resolve(tool: &str) -> Result<ConfigToolOutput, String> {
    if tool != "sdl2-config" {
        return Err("unsupported native config tool".to_owned());
    }
    let executable = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|directory| directory.join(tool))
        .find(|path| path.is_file())
        .ok_or_else(|| format!("native config tool `{tool}` was not found"))?
        .canonicalize()
        .map_err(|error| format!("resolve config tool: {error}"))?;
    let prefix_text = run(&executable, "--prefix")?;
    let prefix = PathBuf::from(prefix_text.trim());
    if !prefix.is_absolute() || prefix.parent().is_none() {
        return Err("config tool prefix must be an absolute installation directory".to_owned());
    }
    let prefix = prefix
        .canonicalize()
        .map_err(|error| format!("resolve config tool prefix: {error}"))?;
    if !prefix.is_dir() || !executable.starts_with(&prefix) {
        return Err("config tool is outside its reported installation prefix".to_owned());
    }
    let output = Command::new("xcrun")
        .args(["--sdk", "macosx", "--show-sdk-path"])
        .output()
        .map_err(|error| format!("read target sysroot: {error}"))?;
    if !output.status.success() {
        return Err("cannot resolve target toolchain sysroot".to_owned());
    }
    let sysroot_text = String::from_utf8(output.stdout).map_err(|error| error.to_string())?;
    let sysroot = Path::new(sysroot_text.trim())
        .canonicalize()
        .map_err(|error| format!("resolve target sysroot: {error}"))?;
    let cflags = run(&executable, "--cflags")?;
    let libraries = run(&executable, "--libs")?;
    let mut flags = parse_flags(&cflags, &[&prefix, &sysroot])?;
    flags.extend(parse_flags(&libraries, &[&prefix, &sysroot])?);
    let digest = rinzu_bindgen::source_package::sha256(
        &std::fs::read(&executable).map_err(|error| error.to_string())?,
    );
    let fingerprint = BTreeMap::from([
        (
            "config.tool".to_owned(),
            executable.to_string_lossy().into_owned(),
        ),
        ("config.identity-version-digest".to_owned(), digest),
        (
            "config.prefix".to_owned(),
            prefix.to_string_lossy().into_owned(),
        ),
        (
            "config.sysroot".to_owned(),
            sysroot.to_string_lossy().into_owned(),
        ),
        ("config.cflags".to_owned(), cflags),
        ("config.libs".to_owned(), libraries),
    ]);
    Ok(ConfigToolOutput { flags, fingerprint })
}

fn run(executable: &Path, mode: &str) -> Result<String, String> {
    let output = Command::new(executable)
        .arg(mode)
        .output()
        .map_err(|error| format!("start config tool: {error}"))?;
    if !output.status.success() {
        return Err(format!("native config tool failed for {mode}"));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("config tool output is not UTF-8: {error}"))
}

pub fn parse_flags(output: &str, trusted_roots: &[&Path]) -> Result<Vec<String>, String> {
    if output.chars().any(|ch| "$`\\\"';|&".contains(ch)) {
        return Err("config output contains shell syntax".to_owned());
    }
    let mut input = output.split_whitespace();
    let mut result = Vec::new();
    while let Some(flag) = input.next() {
        let (kind, value) = if flag == "-framework" {
            (
                "-framework",
                input.next().ok_or("framework name is missing")?,
            )
        } else if matches!(flag, "-I" | "-L" | "-F" | "-D" | "-l") {
            (flag, input.next().ok_or("config flag value is missing")?)
        } else if ["-I", "-L", "-F", "-D", "-l"]
            .iter()
            .any(|prefix| flag.starts_with(prefix))
            && flag.len() > 2
        {
            (&flag[..2], &flag[2..])
        } else {
            return Err(format!("unsupported config flag `{flag}`"));
        };
        if matches!(kind, "-I" | "-L" | "-F") {
            let path = Path::new(value);
            if !path.is_absolute() {
                return Err("config input paths must be absolute".to_owned());
            }
            let canonical = path
                .canonicalize()
                .map_err(|error| format!("resolve config path `{value}`: {error}"))?;
            if !canonical.is_dir() || !trusted_roots.iter().any(|root| canonical.starts_with(root))
            {
                return Err(format!(
                    "config path `{value}` is outside trusted tool inputs"
                ));
            }
            result.push(kind.to_owned());
            result.push(canonical.to_string_lossy().into_owned());
        } else {
            let valid = if kind == "-D" {
                let (name, value) = value
                    .split_once('=')
                    .map_or((value, None), |(name, value)| (name, Some(value)));
                let mut chars = name.chars();
                matches!(chars.next(), Some('a'..='z' | 'A'..='Z' | '_'))
                    && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
                    && value.is_none_or(|value| {
                        !value.is_empty()
                            && value
                                .chars()
                                .all(|ch| ch.is_ascii_alphanumeric() || "_+.-".contains(ch))
                    })
            } else {
                !value.is_empty()
                    && !value.starts_with('-')
                    && value
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || "_+.-".contains(ch))
            };
            if !valid {
                return Err(format!("invalid value for config flag `{kind}`"));
            }
            result.push(kind.to_owned());
            result.push(value.to_owned());
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_flags_are_closed_and_confined_to_trusted_roots() {
        let root = std::env::temp_dir().join(format!("rinzu-config-flags-{}", std::process::id()));
        std::fs::create_dir_all(root.join("include")).unwrap();
        let root = root.canonicalize().unwrap();
        let allowed = format!(
            "-I{} -L {} -F {} -D_THREAD_SAFE -DVERSION=2 -lSDL2 -framework Cocoa",
            root.join("include").display(),
            root.display(),
            root.display()
        );
        assert!(parse_flags(&allowed, &[&root]).is_ok());
        for output in [
            "-Wl,-rpath,/tmp",
            "-Xclang plugin",
            "@response",
            "-fplugin=x",
            "-framework",
            "-D@bad",
            "-l-evil",
            "-DNAME=$(command)",
            "-I/tmp",
            "-Irelative",
        ] {
            assert!(parse_flags(output, &[&root]).is_err(), "accepted {output}");
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(std::env::temp_dir(), root.join("outside")).unwrap();
            assert!(
                parse_flags(&format!("-I{}", root.join("outside").display()), &[&root]).is_err()
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
