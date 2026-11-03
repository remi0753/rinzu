use rinzu_bindgen::source_package::sha256;
use rinzu_compiler::{Manifest, SourceSdkInterface};
use std::collections::BTreeMap;
use std::path::Path;

/// Paths inside an SDK identify inputs relative to that SDK, independent of checkout location.
pub fn normalize(
    root: &Path,
    fingerprint: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, String> {
    fn normalize_value(root: &Path, value: &mut serde_json::Value) {
        match value {
            serde_json::Value::String(text) => {
                if let Ok(relative) = Path::new(text).strip_prefix(root) {
                    *text = format!("sdk:{}", relative.display());
                }
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    normalize_value(root, value);
                }
            }
            serde_json::Value::Object(values) => {
                for value in values.values_mut() {
                    normalize_value(root, value);
                }
            }
            _ => {}
        }
    }
    let mut result = BTreeMap::new();
    for (key, value) in fingerprint {
        let key = if let Some(path) = key.strip_prefix("header:") {
            Path::new(path).strip_prefix(root).map_or_else(
                |_| key.clone(),
                |relative| format!("header:sdk:{}", relative.display()),
            )
        } else {
            key.clone()
        };
        let value = if key == "clang.options" {
            let mut options = serde_json::from_str(value).map_err(|error| error.to_string())?;
            normalize_value(root, &mut options);
            serde_json::to_string(&options).map_err(|error| error.to_string())?
        } else {
            value.clone()
        };
        result.insert(key, value);
    }
    Ok(result)
}

pub fn source_inputs(
    manifest: &Manifest,
    source: &SourceSdkInterface,
    inputs: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    for (path, checked_bytes) in &source.source_inputs {
        let current = std::fs::read(manifest.root.join(path)).map_err(|error| error.to_string())?;
        if current != *checked_bytes {
            return Err(format!(
                "SDK source `{path}` changed after checking; check the package again"
            ));
        }
        inputs.insert(format!("rinzu:{path}"), sha256(checked_bytes));
    }
    inputs.insert(
        "rinzu.language-semantics".to_owned(),
        rinzu_compiler::LANGUAGE_SEMANTICS_REVISION.to_owned(),
    );
    inputs.insert(
        "rinzu.private-runtime-abi".to_owned(),
        rinzu_codegen::BASELINE_RUNTIME_ABI_REVISION.to_owned(),
    );
    let native = &manifest.native;
    let manifest_plan = serde_json::json!({
        "name": manifest.name, "language": manifest.language, "entry": manifest.entry,
        "exports": manifest.exports.modules,
        "dependencies": manifest.dependencies.iter().map(|dependency| &dependency.name).collect::<Vec<_>>(),
        "nativeSources": native.sources, "nativeHeaders": native.headers, "nativeObjects": native.objects,
        "includeDirs": native.include_dirs, "searchPaths": native.search_paths,
        "libraries": native.libraries, "frameworks": native.frameworks, "configTools": native.config_tools,
    });
    inputs.insert(
        "manifest.plan".to_owned(),
        serde_json::to_string(&manifest_plan).map_err(|error| error.to_string())?,
    );
    inputs.insert(
        "native.compile-options".to_owned(),
        "-O2 -Wall -Wextra -Werror -fno-builtin; C: -std=c11; C++: -std=c++17; Objective-C/ObjC++: -fobjc-arc; forced C-compatible ABI contract; typed indirect facade -fno-builtin -fno-lto".to_owned(),
    );
    Ok(())
}

/// Parse Clang's Make dependency output as data, including escaped spaces and line continuations.
pub fn dependency_paths(text: &str) -> Result<Vec<std::path::PathBuf>, String> {
    let (_, dependencies) = text
        .split_once(':')
        .ok_or("Clang dependency output has no target")?;
    let mut paths = Vec::new();
    let mut token = String::new();
    let mut chars = dependencies.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\\' => match chars.next() {
                Some('\n') => {}
                Some('\r') if chars.peek() == Some(&'\n') => {
                    chars.next();
                }
                Some(character) => token.push(character),
                None => return Err("incomplete dependency path escape".to_owned()),
            },
            '$' if chars.peek() == Some(&'$') => {
                chars.next();
                token.push('$');
            }
            character if character.is_whitespace() => {
                if !token.is_empty() {
                    paths.push(std::path::PathBuf::from(std::mem::take(&mut token)));
                }
            }
            character => token.push(character),
        }
    }
    if !token.is_empty() {
        paths.push(token.into());
    }
    Ok(paths)
}

pub fn include_inputs(
    root: &Path,
    sysroot: &Path,
    generated: &Path,
    dependencies: &str,
    fingerprint: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let generated = generated
        .canonicalize()
        .map_err(|error| error.to_string())?;
    for path in dependency_paths(dependencies)? {
        let path = path
            .canonicalize()
            .map_err(|error| format!("read included input {}: {error}", path.display()))?;
        if path.starts_with(&generated) {
            continue;
        }
        let identity = if let Ok(relative) = path.strip_prefix(&root) {
            format!("include:sdk:{}", relative.display())
        } else if let Ok(relative) = path.strip_prefix(sysroot) {
            format!("include:sysroot:{}", relative.display())
        } else {
            format!("include:external:{}", path.display())
        };
        fingerprint.insert(
            identity,
            sha256(&std::fs::read(&path).map_err(|error| error.to_string())?),
        );
    }
    Ok(())
}

pub fn link_inputs(
    root: &Path,
    sysroot: &Path,
    trace: &str,
) -> Result<BTreeMap<String, String>, String> {
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let mut result = BTreeMap::new();
    for line in trace.lines().filter(|line| !line.trim().is_empty()) {
        let path = line.rfind(".a(").map_or(line, |index| &line[..index + 2]);
        let path = std::path::PathBuf::from(path);
        // Temporary C object names carry no input identity; generated/source objects are separately framed.
        if path.extension().is_some_and(|extension| extension == "o") {
            continue;
        }
        if !path.is_absolute() {
            return Err(format!("unrecognized linker input trace `{line}`"));
        }
        let path = path
            .canonicalize()
            .map_err(|error| format!("resolve linked input {}: {error}", path.display()))?;
        let identity = if let Ok(relative) = path.strip_prefix(&root) {
            format!("link:sdk:{}", relative.display())
        } else if let Ok(relative) = path.strip_prefix(sysroot) {
            format!("link:sysroot:{}", relative.display())
        } else {
            format!("link:external:{}", path.display())
        };
        if let std::collections::btree_map::Entry::Vacant(entry) = result.entry(identity) {
            entry.insert(sha256(
                &std::fs::read(path).map_err(|error| error.to_string())?,
            ));
        }
    }
    Ok(result)
}

pub fn verify_link_inputs(
    root: &Path,
    sysroot: &Path,
    inputs: &BTreeMap<String, String>,
) -> Result<(), String> {
    for (identity, digest) in inputs {
        let path = if let Some(relative) = identity.strip_prefix("link:sdk:") {
            root.join(relative)
        } else if let Some(relative) = identity.strip_prefix("link:sysroot:") {
            sysroot.join(relative)
        } else if let Some(path) = identity.strip_prefix("link:external:") {
            path.into()
        } else {
            return Err("invalid resolved native input identity".to_owned());
        };
        if sha256(&std::fs::read(path).map_err(|error| error.to_string())?) != *digest {
            return Err("resolved native input changed while preparing source facade".to_owned());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn make_paths_are_unescaped_without_shell_evaluation() {
        assert_eq!(
            dependency_paths("rinzu: /a/first\\ file.h \\\n /b/x\\#y.h /c/$$value.h\n").unwrap(),
            vec![
                std::path::PathBuf::from("/a/first file.h"),
                "/b/x#y.h".into(),
                "/c/$value.h".into()
            ]
        );
        assert!(dependency_paths("invalid").is_err());
        assert!(dependency_paths("rinzu: /x\\").is_err());
    }
}
