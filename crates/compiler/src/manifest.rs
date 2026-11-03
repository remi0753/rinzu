use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use crate::{Diagnostic, DiagnosticCode, FileId, SourceRange};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackageKind {
    Application,
    Sdk,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PathDependency {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PackageExports {
    pub modules: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NativeManifest {
    pub sources: Vec<PathBuf>,
    pub headers: Vec<PathBuf>,
    pub objects: Vec<PathBuf>,
    pub include_dirs: Vec<PathBuf>,
    pub search_paths: Vec<PathBuf>,
    pub frameworks: Vec<String>,
    pub libraries: Vec<String>,
    pub config_tools: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Manifest {
    pub root: PathBuf,
    pub name: String,
    pub language: String,
    pub kind: PackageKind,
    pub entry: PathBuf,
    pub dependencies: Vec<PathDependency>,
    pub exports: PackageExports,
    pub native: NativeManifest,
}

#[derive(Debug)]
pub struct ManifestError {
    path: PathBuf,
    message: String,
    suggestion: String,
}

impl ManifestError {
    fn new(
        path: impl Into<PathBuf>,
        message: impl Into<String>,
        suggestion: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
            suggestion: suggestion.into(),
        }
    }

    #[must_use]
    pub fn into_diagnostic(self) -> Diagnostic {
        Diagnostic::error(
            if self.message.contains("not found") {
                DiagnosticCode::ManifestNotFound
            } else {
                DiagnosticCode::ManifestInvalid
            },
            SourceRange::empty(FileId::MANIFEST, 0),
            format!("{}: {}", self.path.display(), self.message),
            self.suggestion,
        )
    }
}

pub fn find_manifest(start: &Path) -> Result<PathBuf, ManifestError> {
    let mut current = if start.is_file() {
        start.parent().unwrap_or(start).to_path_buf()
    } else {
        start.to_path_buf()
    };
    loop {
        let candidate = current.join("rinzu.toml");
        if candidate.is_file() {
            return Ok(candidate);
        }
        if !current.pop() {
            return Err(ManifestError::new(
                start,
                "`rinzu.toml` was not found",
                "run `rinzu init` or invoke the command inside a Rinzu package",
            ));
        }
    }
}

pub fn load_manifest(path: &Path) -> Result<Manifest, ManifestError> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        ManifestError::new(
            path,
            format!("cannot read manifest: {error}"),
            "make the manifest readable",
        )
    })?;
    load_manifest_text(path, &text)
}

pub(crate) fn load_manifest_text(path: &Path, text: &str) -> Result<Manifest, ManifestError> {
    let sections = parse_sections(path, text)?;
    let package = sections.get("package").ok_or_else(|| {
        ManifestError::new(
            path,
            "missing `[package]` section",
            "add package name and language under `[package]`",
        )
    })?;
    let name = required_string(package, "name", path)?;
    if !valid_package_name(&name) {
        return Err(ManifestError::new(
            path,
            format!("invalid package name `{name}`"),
            "use a dotted package name whose segments match `[a-z][a-z0-9_]*`",
        ));
    }
    let language = required_string(package, "language", path)?;
    if !matches!(language.as_str(), "0.1" | "0.2" | "0.3" | "0.4" | "0.5") {
        return Err(ManifestError::new(
            path,
            format!("unsupported language version `{language}`"),
            "set `language` to a supported version: 0.1, 0.2, 0.3, 0.4, or 0.5",
        ));
    }
    let kind = match optional_string(package, "kind", path)?.as_deref() {
        None | Some("application") => PackageKind::Application,
        Some("sdk") => PackageKind::Sdk,
        Some(value) => {
            return Err(ManifestError::new(
                path,
                format!("unknown package kind `{value}`"),
                "use `application` or `sdk`",
            ));
        }
    };
    let default_entry = if kind == PackageKind::Sdk {
        format!("src/{}.rinzu", name.rsplit('.').next().unwrap_or("sdk"))
    } else {
        "src/main.rinzu".to_owned()
    };
    let entry = PathBuf::from(optional_string(package, "entry", path)?.unwrap_or(default_entry));
    validate_entry(path, &entry)?;

    let allowed_sections: &[&str] = if language == "0.1" {
        &["package"]
    } else {
        &["package", "dependencies", "exports", "native"]
    };
    if let Some(section) = sections
        .keys()
        .find(|section| !allowed_sections.contains(&section.as_str()))
    {
        return Err(ManifestError::new(
            path,
            format!("unknown section `[{section}]`"),
            if language == "0.1" {
                "language 0.1 accepts only `[package]`"
            } else {
                "use `[package]`, `[dependencies]`, `[exports]`, or `[native]`"
            },
        ));
    }
    let allowed_package: &[&str] = if language == "0.1" {
        &["name", "language", "entry"]
    } else {
        &["name", "language", "entry", "kind"]
    };
    reject_unknown_keys(path, "package", package, allowed_package)?;
    if language == "0.1" && kind != PackageKind::Application {
        return Err(ManifestError::new(
            path,
            "language 0.1 does not support SDK packages",
            "set `language = \"0.5\"` or remove `package.kind`",
        ));
    }

    let mut dependencies = Vec::new();
    if let Some(values) = sections.get("dependencies") {
        for (dependency, raw) in values {
            if crate::standard_library::registered(dependency) {
                return Err(ManifestError::new(
                    path,
                    format!("{dependency} is a toolchain module and cannot be replaced"),
                    format!("remove the {dependency} dependency"),
                ));
            }
            if !valid_package_name(dependency) {
                return Err(ManifestError::new(
                    path,
                    format!("invalid dependency name `{dependency}`"),
                    "use the dependency package's dotted lowercase name",
                ));
            }
            let dependency_path = inline_path(raw).map_err(|message| {
                ManifestError::new(
                    path,
                    format!("dependency `{dependency}` {message}"),
                    "write `{ path = \"../sdk/package\" }`",
                )
            })?;
            if dependency_path.is_absolute() {
                return Err(ManifestError::new(
                    path,
                    format!("dependency `{dependency}` path must be relative"),
                    "use a path relative to this package",
                ));
            }
            dependencies.push(PathDependency {
                name: dependency.clone(),
                path: dependency_path,
            });
        }
    }

    let exports = if let Some(values) = sections.get("exports") {
        reject_unknown_keys(path, "exports", values, &["modules"])?;
        PackageExports {
            modules: optional_array(values, "modules", path)?.unwrap_or_default(),
        }
    } else {
        PackageExports::default()
    };
    for module in &exports.modules {
        if !valid_module_name(module) {
            return Err(ManifestError::new(
                path,
                format!("invalid exported module `{module}`"),
                "use a dotted lowercase module name",
            ));
        }
    }

    let native = if let Some(values) = sections.get("native") {
        reject_unknown_keys(
            path,
            "native",
            values,
            &[
                "sources",
                "headers",
                "objects",
                "include-dirs",
                "search-paths",
                "frameworks",
                "libraries",
                "config-tools",
            ],
        )?;
        NativeManifest {
            sources: optional_array(values, "sources", path)?
                .unwrap_or_default()
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            headers: optional_array(values, "headers", path)?
                .unwrap_or_default()
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            objects: optional_array(values, "objects", path)?
                .unwrap_or_default()
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            include_dirs: optional_array(values, "include-dirs", path)?
                .unwrap_or_default()
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            search_paths: optional_array(values, "search-paths", path)?
                .unwrap_or_default()
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            frameworks: optional_array(values, "frameworks", path)?.unwrap_or_default(),
            libraries: optional_array(values, "libraries", path)?.unwrap_or_default(),
            config_tools: optional_array(values, "config-tools", path)?.unwrap_or_default(),
        }
    } else {
        NativeManifest::default()
    };
    for native_path in native
        .sources
        .iter()
        .chain(&native.headers)
        .chain(&native.objects)
        .chain(&native.include_dirs)
        .chain(&native.search_paths)
    {
        validate_package_relative(path, native_path, "native input")?;
    }
    for name in native.frameworks.iter().chain(&native.libraries) {
        if name.is_empty()
            || !name.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '+')
            })
        {
            return Err(ManifestError::new(
                path,
                format!("invalid native library/framework name `{name}`"),
                "use an explicit ASCII library or framework name",
            ));
        }
    }
    for tool in &native.config_tools {
        if tool != "sdl2-config" {
            return Err(ManifestError::new(
                path,
                format!("invalid native config tool `{tool}`"),
                "use only sdl2-config",
            ));
        }
    }
    if kind != PackageKind::Sdk
        && (!exports.modules.is_empty() || native != NativeManifest::default())
    {
        return Err(ManifestError::new(
            path,
            "application package cannot declare SDK exports or native inputs",
            "move native declarations and inputs into a `kind = \"sdk\"` dependency",
        ));
    }

    Ok(Manifest {
        root: path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf(),
        name,
        language,
        kind,
        entry,
        dependencies,
        exports,
        native,
    })
}

/// Resolve declared inputs within their owning package; path dependencies have separate roots.
pub fn canonicalize_native_inputs(manifest: &mut Manifest) -> Result<(), ManifestError> {
    let manifest_path = manifest.root.join("rinzu.toml");
    let root = manifest.root.canonicalize().map_err(|error| {
        ManifestError::new(
            &manifest_path,
            error.to_string(),
            "make the package root readable",
        )
    })?;
    for (paths, directories) in [
        (&mut manifest.native.sources, false),
        (&mut manifest.native.headers, false),
        (&mut manifest.native.objects, false),
        (&mut manifest.native.include_dirs, true),
        (&mut manifest.native.search_paths, true),
    ] {
        for path in paths {
            let canonical = manifest.root.join(&*path).canonicalize().map_err(|error| {
                ManifestError::new(
                    &manifest_path,
                    format!("cannot resolve native input `{}`: {error}", path.display()),
                    "declare an existing input",
                )
            })?;
            if !canonical.starts_with(&root) {
                return Err(ManifestError::new(
                    &manifest_path,
                    format!("native input `{}` escapes the package root", path.display()),
                    "keep native inputs inside their owning package",
                ));
            }
            if (directories && !canonical.is_dir()) || (!directories && !canonical.is_file()) {
                return Err(ManifestError::new(
                    &manifest_path,
                    format!("native input `{}` has the wrong file kind", path.display()),
                    "use a file or directory appropriate to the input field",
                ));
            }
            *path = canonical.strip_prefix(&root).unwrap().to_path_buf();
        }
    }
    for source in &manifest.native.sources {
        if !matches!(
            source.extension().and_then(|extension| extension.to_str()),
            Some("c" | "m" | "cpp" | "mm")
        ) {
            return Err(ManifestError::new(
                &manifest_path,
                "native sources must be .c, .m, .cpp or .mm files",
                "use a typed C-compatible shim; C++/ObjC++ must contain exceptions and declare their runtime library",
            ));
        }
    }
    for object in &manifest.native.objects {
        if !matches!(
            object.extension().and_then(|extension| extension.to_str()),
            Some("o" | "a" | "dylib" | "so" | "dll" | "lib")
        ) {
            return Err(ManifestError::new(
                &manifest_path,
                "unsupported native object input",
                "use an explicit object, archive or shared library artifact",
            ));
        }
    }
    // Generated Clang queries live outside the package. Resolve their include
    // paths from the physical package root even when the CLI path was relative.
    manifest.root = root;
    Ok(())
}

type Sections = BTreeMap<String, BTreeMap<String, String>>;

fn parse_sections(path: &Path, text: &str) -> Result<Sections, ManifestError> {
    let mut sections = Sections::new();
    let mut section: Option<String> = None;
    for (line_index, raw_line) in text.lines().enumerate() {
        let line = strip_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let name = line[1..line.len() - 1].trim();
            if name.is_empty() || sections.contains_key(name) {
                return Err(ManifestError::new(
                    path,
                    format!("invalid or duplicate section on line {}", line_index + 1),
                    "declare each manifest section once",
                ));
            }
            sections.insert(name.to_owned(), BTreeMap::new());
            section = Some(name.to_owned());
            continue;
        }
        let section_name = section.as_ref().ok_or_else(|| {
            ManifestError::new(
                path,
                format!("key outside a section on line {}", line_index + 1),
                "add a `[package]` section before package keys",
            )
        })?;
        let (raw_key, raw_value) = line.split_once('=').ok_or_else(|| {
            ManifestError::new(
                path,
                format!("invalid key/value on line {}", line_index + 1),
                "write `key = value`",
            )
        })?;
        let key = unquote(raw_key.trim()).unwrap_or_else(|| raw_key.trim().to_owned());
        if key.is_empty() {
            return Err(ManifestError::new(
                path,
                format!("empty key on line {}", line_index + 1),
                "name the manifest key",
            ));
        }
        let values = sections
            .get_mut(section_name)
            .expect("section was inserted");
        if values
            .insert(key.clone(), raw_value.trim().to_owned())
            .is_some()
        {
            return Err(ManifestError::new(
                path,
                format!("duplicate key `{key}` in `[{section_name}]`"),
                "keep exactly one value for each manifest key",
            ));
        }
    }
    Ok(sections)
}

fn strip_comment(line: &str) -> &str {
    let mut quoted = false;
    let mut escaped = false;
    for (index, character) in line.char_indices() {
        match character {
            '\\' if quoted => escaped = !escaped,
            '"' if !escaped => quoted = !quoted,
            '#' if !quoted => return &line[..index],
            _ => escaped = false,
        }
    }
    line
}

fn required_string(
    values: &BTreeMap<String, String>,
    key: &str,
    path: &Path,
) -> Result<String, ManifestError> {
    optional_string(values, key, path)?.ok_or_else(|| {
        ManifestError::new(
            path,
            format!("missing package key `{key}`"),
            format!("add `{key} = \"...\"` under `[package]`"),
        )
    })
}

fn optional_string(
    values: &BTreeMap<String, String>,
    key: &str,
    path: &Path,
) -> Result<Option<String>, ManifestError> {
    values
        .get(key)
        .map(|value| {
            unquote(value).ok_or_else(|| {
                ManifestError::new(
                    path,
                    format!("value for `{key}` must be a quoted string"),
                    "surround the value with double quotes",
                )
            })
        })
        .transpose()
}

fn optional_array(
    values: &BTreeMap<String, String>,
    key: &str,
    path: &Path,
) -> Result<Option<Vec<String>>, ManifestError> {
    values
        .get(key)
        .map(|value| {
            parse_string_array(value).ok_or_else(|| {
                ManifestError::new(
                    path,
                    format!("value for `{key}` must be an array of strings"),
                    "write `[\"value\", \"value\"]`",
                )
            })
        })
        .transpose()
}

fn inline_path(value: &str) -> Result<PathBuf, &'static str> {
    let value = value.trim();
    let inner = value
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
        .ok_or("must be an inline table")?
        .trim();
    let (key, value) = inner.split_once('=').ok_or("must contain a `path` key")?;
    if key.trim() != "path" {
        return Err("accepts only a `path` key");
    }
    unquote(value.trim())
        .map(PathBuf::from)
        .ok_or("path must be a quoted string")
}

fn unquote(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() < 2 || !value.starts_with('"') || !value.ends_with('"') {
        return None;
    }
    let inner = &value[1..value.len() - 1];
    if inner.contains('"') || inner.contains('\\') {
        return None;
    }
    Some(inner.to_owned())
}

fn parse_string_array(value: &str) -> Option<Vec<String>> {
    let inner = value.trim().strip_prefix('[')?.strip_suffix(']')?.trim();
    if inner.is_empty() {
        return Some(Vec::new());
    }
    inner.split(',').map(|part| unquote(part.trim())).collect()
}

fn reject_unknown_keys(
    path: &Path,
    section: &str,
    values: &BTreeMap<String, String>,
    allowed: &[&str],
) -> Result<(), ManifestError> {
    if let Some(key) = values.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(ManifestError::new(
            path,
            format!("unknown {section} key `{key}`"),
            format!("remove `{key}` or use a supported {section} key"),
        ));
    }
    Ok(())
}

fn valid_package_name(name: &str) -> bool {
    !name.is_empty() && name.split('.').all(valid_name_segment)
}

fn valid_module_name(name: &str) -> bool {
    !name.is_empty() && name.split('.').all(valid_name_segment)
}

fn valid_name_segment(segment: &str) -> bool {
    let mut chars = segment.chars();
    matches!(chars.next(), Some('a'..='z'))
        && chars.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
}

fn validate_entry(path: &Path, entry: &Path) -> Result<(), ManifestError> {
    let mut components = entry.components();
    if !matches!(components.next(), Some(Component::Normal(first)) if first == "src")
        || components.any(|component| !matches!(component, Component::Normal(_)))
        || entry.extension().and_then(|extension| extension.to_str()) != Some("rinzu")
    {
        return Err(ManifestError::new(
            path,
            format!("invalid entry path `{}`", entry.display()),
            "use a relative `.rinzu` path inside `src/` without `.` or `..` segments",
        ));
    }
    Ok(())
}

fn validate_package_relative(
    manifest: &Path,
    value: &Path,
    label: &str,
) -> Result<(), ManifestError> {
    if value.is_absolute() || value.as_os_str().is_empty() {
        return Err(ManifestError::new(
            manifest,
            format!("{label} `{}` escapes the package root", value.display()),
            "use a relative input inside the package root",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_v02_sdk_manifest() {
        let root = std::env::temp_dir().join(format!("rinzu-manifest-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("rinzu.toml");
        std::fs::write(
            &path,
            r#"
[package]
name = "rinzu.example"
language = "0.2"
kind = "sdk"

[exports]
modules = ["example"]

[native]
headers = ["native/example.h"]
sources = ["native/example.c"]
frameworks = ["Foundation"]
"#,
        )
        .unwrap();
        let manifest = load_manifest(&path).unwrap();
        assert_eq!(manifest.kind, PackageKind::Sdk);
        assert_eq!(manifest.entry, Path::new("src/example.rinzu"));
        assert_eq!(manifest.exports.modules, ["example"]);
        assert_eq!(manifest.native.frameworks, ["Foundation"]);
        std::fs::remove_dir_all(root).unwrap();
    }
}
