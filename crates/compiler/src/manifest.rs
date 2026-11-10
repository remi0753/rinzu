use std::collections::BTreeMap;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Diagnostic, DiagnosticCode, FileId, SourceFile, SourceRange};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackageKind {
    Application,
    Sdk,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Dependency {
    pub name: String,
    pub source: DependencySource,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DependencySource {
    Path(PathBuf),
    Archive(ArchiveDependency),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveDependency {
    pub archive: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
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
    pub toolchain_version: Option<String>,
    pub minimum_toolchain: Option<String>,
    pub kind: PackageKind,
    pub entry: PathBuf,
    pub dependencies: Vec<Dependency>,
    pub exports: PackageExports,
    pub native: NativeManifest,
}

#[derive(Debug)]
pub struct ManifestError {
    code: DiagnosticCode,
    path: PathBuf,
    message: String,
    suggestion: String,
    span: Option<Range<usize>>,
}

impl ManifestError {
    fn new(
        path: impl Into<PathBuf>,
        message: impl Into<String>,
        suggestion: impl Into<String>,
    ) -> Self {
        Self {
            code: DiagnosticCode::ManifestInvalid,
            path: path.into(),
            message: message.into(),
            suggestion: suggestion.into(),
            span: None,
        }
    }

    fn at(mut self, span: Range<usize>) -> Self {
        self.span = Some(span);
        self
    }

    fn at_value(mut self, values: &Entries, key: &str) -> Self {
        self.span = values.get(key).map(|entry| entry.value.clone());
        self
    }

    pub(crate) fn has_span(&self) -> bool {
        self.span.is_some()
    }

    #[must_use]
    pub fn into_diagnostic(self) -> Diagnostic {
        self.diagnostic(None)
    }

    pub(crate) fn diagnostic(self, source: Option<&SourceFile>) -> Diagnostic {
        let range = self.span.zip(source).map(|(span, file)| {
            SourceRange::new(
                file.id(),
                file.original_offset(span.start),
                file.original_offset(span.end),
            )
        });
        let diagnostic = Diagnostic::error(
            self.code,
            range.unwrap_or_else(|| SourceRange::empty(FileId::MANIFEST, 0)),
            self.message,
            self.suggestion,
        );
        if range.is_some() {
            diagnostic
        } else {
            diagnostic.with_path(self.path)
        }
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
            return Err(ManifestError {
                code: DiagnosticCode::ManifestNotFound,
                ..ManifestError::new(
                    start,
                    "`rinzu.toml` was not found",
                    "run `rinzu init` or invoke the command inside a Rinzu package",
                )
            });
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

pub fn load_manifest_text(path: &Path, text: &str) -> Result<Manifest, ManifestError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let sections = parse_sections(path, text)?;
    let package = &sections
        .get("package")
        .ok_or_else(|| {
            ManifestError::new(
                path,
                "missing `[package]` section",
                "add package name and language under `[package]`",
            )
        })?
        .entries;
    let name = required_string(package, "name", path)?;
    if !valid_package_name(&name) {
        return Err(ManifestError::new(
            path,
            format!("invalid package name `{name}`"),
            "use a dotted package name whose segments match `[a-z][a-z0-9_]*`",
        )
        .at_value(package, "name"));
    }
    let language = required_string(package, "language", path)?;
    if crate::compatibility::version_parts::<2>(&language).is_none() {
        return Err(ManifestError::new(
            path,
            format!("invalid language version `{language}`"),
            "use a major.minor language version such as 0.5",
        )
        .at_value(package, "language"));
    }
    let kind = match optional_string(package, "kind", path)?.as_deref() {
        None | Some("application") => PackageKind::Application,
        Some("sdk") => PackageKind::Sdk,
        Some(value) => {
            return Err(ManifestError::new(
                path,
                format!("unknown package kind `{value}`"),
                "use `application` or `sdk`",
            )
            .at_value(package, "kind"));
        }
    };
    let default_entry = if kind == PackageKind::Sdk {
        format!("src/{}.rinzu", name.rsplit('.').next().unwrap_or("sdk"))
    } else {
        "src/main.rinzu".to_owned()
    };
    let entry = PathBuf::from(optional_string(package, "entry", path)?.unwrap_or(default_entry));
    validate_entry(path, &entry).map_err(|error| error.at_value(package, "entry"))?;

    let allowed_sections: &[&str] = if language == "0.1" {
        &["package"]
    } else {
        &[
            "package",
            "dependencies",
            "exports",
            "native",
            "toolchain",
            "requires",
        ]
    };
    if let Some((section, location)) = sections
        .iter()
        .find(|(section, _)| !allowed_sections.contains(&section.as_str()))
    {
        return Err(ManifestError::new(
            path,
            format!("unknown section `[{section}]`"),
            if language == "0.1" {
                "language 0.1 accepts only `[package]`"
            } else {
                "use a supported package, dependency, native, or toolchain section"
            },
        )
        .at(location.span.clone()));
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
        )
        .at_value(package, "kind"));
    }

    let toolchain_version = sections
        .get("toolchain")
        .map(|section| release_version(path, section, "toolchain", "version"))
        .transpose()?;
    let minimum_toolchain = sections
        .get("requires")
        .map(|section| {
            if kind != PackageKind::Sdk {
                return Err(ManifestError::new(
                    path,
                    "`[requires]` declares an SDK's minimum toolchain version",
                    "use `[toolchain]` to select an exact release for an application",
                )
                .at(section.span.clone()));
            }
            release_version(path, section, "requires", "toolchain")
        })
        .transpose()?;

    let mut dependencies = Vec::new();
    if let Some(section) = sections.get("dependencies") {
        for (dependency, entry) in &section.entries {
            if crate::standard_library::registered(dependency) {
                return Err(ManifestError::new(
                    path,
                    format!("{dependency} is a toolchain module and cannot be replaced"),
                    format!("remove the {dependency} dependency"),
                )
                .at(entry.key.clone()));
            }
            if !valid_package_name(dependency) {
                return Err(ManifestError::new(
                    path,
                    format!("invalid dependency name `{dependency}`"),
                    "use the dependency package's dotted lowercase name",
                )
                .at(entry.key.clone()));
            }
            let source = dependency_source(&entry.text).map_err(|message| {
                ManifestError::new(
                    path,
                    format!("dependency `{dependency}` {message}"),
                    "use `{ path = \"../sdk\" }` or `{ archive = \"https://.../sdk.tar.gz\" }`",
                )
                .at(entry.value.clone())
            })?;
            if matches!(&source, DependencySource::Path(path) if path.is_absolute()) {
                return Err(ManifestError::new(
                    path,
                    format!("dependency `{dependency}` path must be relative"),
                    "use a path relative to this package",
                )
                .at(entry.value.clone()));
            }
            if matches!(&source, DependencySource::Archive(_))
                && crate::official_sdk::registered(dependency)
            {
                return Err(ManifestError::new(
                    path,
                    format!("{dependency} is an official SDK and cannot be supplied by an archive"),
                    "import the installed SDK or use a local path for SDK development",
                )
                .at(entry.key.clone()));
            }
            dependencies.push(Dependency {
                name: dependency.clone(),
                source,
            });
        }
    }

    let exports = if let Some(section) = sections.get("exports") {
        let values = &section.entries;
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
            )
            .at_value(&sections["exports"].entries, "modules"));
        }
    }

    let native = if let Some(section) = sections.get("native") {
        let values = &section.entries;
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
    for (key, paths) in [
        ("sources", &native.sources),
        ("headers", &native.headers),
        ("objects", &native.objects),
        ("include-dirs", &native.include_dirs),
        ("search-paths", &native.search_paths),
    ] {
        for native_path in paths {
            validate_package_relative(path, native_path, "native input")
                .map_err(|error| error.at_value(&sections["native"].entries, key))?;
        }
    }
    for (key, names) in [
        ("frameworks", &native.frameworks),
        ("libraries", &native.libraries),
    ] {
        for name in names {
            if name.is_empty()
                || !name.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '_' | '+')
                })
            {
                return Err(ManifestError::new(
                    path,
                    format!("invalid native library/framework name `{name}`"),
                    "use an explicit ASCII library or framework name",
                )
                .at_value(&sections["native"].entries, key));
            }
        }
    }
    for tool in &native.config_tools {
        if tool != "sdl2-config" {
            return Err(ManifestError::new(
                path,
                format!("invalid native config tool `{tool}`"),
                "use only sdl2-config",
            )
            .at_value(&sections["native"].entries, "config-tools"));
        }
    }
    if kind != PackageKind::Sdk
        && (!exports.modules.is_empty() || native != NativeManifest::default())
    {
        let section = if exports.modules.is_empty() {
            "native"
        } else {
            "exports"
        };
        return Err(ManifestError::new(
            path,
            "application package cannot declare SDK exports or native inputs",
            "move native declarations and inputs into a `kind = \"sdk\"` dependency",
        )
        .at(sections[section].span.clone()));
    }

    Ok(Manifest {
        root: path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf(),
        name,
        language,
        toolchain_version,
        minimum_toolchain,
        kind,
        entry,
        dependencies,
        exports,
        native,
    })
}

fn release_version(
    path: &Path,
    section: &Section,
    section_name: &str,
    key: &str,
) -> Result<String, ManifestError> {
    let entries = &section.entries;
    reject_unknown_keys(path, section_name, entries, &[key])?;
    let value = required_string(entries, key, path)?;
    if crate::compatibility::version_parts::<3>(&value).is_none() {
        return Err(ManifestError::new(
            path,
            format!("invalid toolchain release `{value}`"),
            "use a major.minor.patch release number",
        )
        .at_value(entries, key));
    }
    Ok(value)
}

/// Resolve declared inputs within their owning package; path dependencies have separate roots.
pub fn canonicalize_native_inputs(manifest: &mut Manifest) -> Result<(), ManifestError> {
    canonicalize_native_inputs_with(manifest, Path::canonicalize)
}

pub(crate) fn canonicalize_native_inputs_with(
    manifest: &mut Manifest,
    resolve: impl Fn(&Path) -> std::io::Result<PathBuf>,
) -> Result<(), ManifestError> {
    let manifest_path = manifest.root.join("rinzu.toml");
    let root = resolve(&manifest.root).map_err(|error| {
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
            let canonical = resolve(&manifest.root.join(&*path)).map_err(|error| {
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

struct Entry {
    text: String,
    key: Range<usize>,
    value: Range<usize>,
}

type Entries = BTreeMap<String, Entry>;
type Sections = BTreeMap<String, Section>;

struct Section {
    span: Range<usize>,
    entries: Entries,
}

fn parse_sections(path: &Path, text: &str) -> Result<Sections, ManifestError> {
    let mut sections = Sections::new();
    let mut section: Option<String> = None;
    let mut offset = 0;
    for raw_line in text.split_inclusive('\n') {
        let content = strip_comment(raw_line);
        let left = content.trim_start();
        let line = left.trim_end();
        let start = offset + content.len() - left.len();
        offset += raw_line.len();
        if line.is_empty() {
            continue;
        }
        let span = start..start + line.len();
        if line.starts_with('[') && line.ends_with(']') {
            let name = line[1..line.len() - 1].trim();
            if name.is_empty() || sections.contains_key(name) {
                return Err(ManifestError::new(
                    path,
                    "invalid or duplicate section",
                    "declare each manifest section once",
                )
                .at(span));
            }
            sections.insert(
                name.to_owned(),
                Section {
                    span,
                    entries: Entries::new(),
                },
            );
            section = Some(name.to_owned());
            continue;
        }
        let section_name = section.as_ref().ok_or_else(|| {
            ManifestError::new(
                path,
                "key outside a section",
                "add a `[package]` section before package keys",
            )
            .at(span.clone())
        })?;
        let (raw_key, raw_value) = line.split_once('=').ok_or_else(|| {
            ManifestError::new(path, "invalid key/value", "write `key = value`").at(span)
        })?;
        let key = unquote(raw_key.trim()).unwrap_or_else(|| raw_key.trim().to_owned());
        if key.is_empty() {
            return Err(
                ManifestError::new(path, "empty key", "name the manifest key").at(start..start),
            );
        }
        let values = &mut sections
            .get_mut(section_name)
            .expect("section was inserted")
            .entries;
        let key_span = start..start + raw_key.trim_end().len();
        let value = raw_value.trim();
        let value_start = start + line.len() - value.len();
        if values
            .insert(
                key.clone(),
                Entry {
                    text: value.to_owned(),
                    key: key_span.clone(),
                    value: value_start..value_start + value.len(),
                },
            )
            .is_some()
        {
            return Err(ManifestError::new(
                path,
                format!("duplicate key `{key}` in `[{section_name}]`"),
                "keep exactly one value for each manifest key",
            )
            .at(key_span));
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

fn required_string(values: &Entries, key: &str, path: &Path) -> Result<String, ManifestError> {
    optional_string(values, key, path)?.ok_or_else(|| {
        ManifestError::new(
            path,
            format!("missing package key `{key}`"),
            format!("add `{key} = \"...\"` under `[package]`"),
        )
    })
}

fn optional_string(
    values: &Entries,
    key: &str,
    path: &Path,
) -> Result<Option<String>, ManifestError> {
    values
        .get(key)
        .map(|value| {
            unquote(&value.text).ok_or_else(|| {
                ManifestError::new(
                    path,
                    format!("value for `{key}` must be a quoted string"),
                    "surround the value with double quotes",
                )
                .at(value.value.clone())
            })
        })
        .transpose()
}

fn optional_array(
    values: &Entries,
    key: &str,
    path: &Path,
) -> Result<Option<Vec<String>>, ManifestError> {
    values
        .get(key)
        .map(|value| {
            parse_string_array(&value.text).ok_or_else(|| {
                ManifestError::new(
                    path,
                    format!("value for `{key}` must be an array of strings"),
                    "write `[\"value\", \"value\"]`",
                )
                .at(value.value.clone())
            })
        })
        .transpose()
}

fn dependency_source(value: &str) -> Result<DependencySource, &'static str> {
    let mut remaining = value
        .trim()
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
        .ok_or("must be an inline table")?
        .trim();
    let mut path = None;
    let mut archive = None;
    let mut integrity = None;
    loop {
        let (key, value) = remaining
            .split_once('=')
            .ok_or("must contain key/value pairs")?;
        let value = value.trim_start();
        let end = value
            .strip_prefix('"')
            .and_then(|text| text.find('"'))
            .map(|index| index + 2)
            .ok_or("values must be quoted strings")?;
        let field = match key.trim() {
            "path" => &mut path,
            "archive" => &mut archive,
            "integrity" => &mut integrity,
            _ => return Err("accepts only `path`, `archive`, and `integrity` keys"),
        };
        let value_text = unquote(&value[..end]).ok_or("values must be quoted strings")?;
        if field.replace(value_text).is_some() {
            return Err("contains a duplicate key");
        }
        let tail = value[end..].trim_start();
        if tail.is_empty() {
            break;
        }
        remaining = tail
            .strip_prefix(',')
            .ok_or("must separate fields with a comma")?
            .trim_start();
    }
    match (path, archive, integrity) {
        (Some(path), None, None) => Ok(DependencySource::Path(PathBuf::from(path))),
        (None, Some(archive), integrity) if valid_archive_locator(&archive) => {
            let sha256 = integrity
                .map(|value| {
                    value
                        .strip_prefix("sha256:")
                        .filter(|digest| {
                            digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit())
                        })
                        .map(str::to_ascii_lowercase)
                        .ok_or("integrity must be `sha256:` followed by 64 hexadecimal digits")
                })
                .transpose()?;
            Ok(DependencySource::Archive(ArchiveDependency {
                archive,
                sha256,
            }))
        }
        (None, Some(_), _) => Err("archive must be an HTTPS URL or a relative archive path"),
        _ => Err("requires either `path` or `archive`; `integrity` applies to archives"),
    }
}

pub fn valid_archive_locator(locator: &str) -> bool {
    if locator.is_empty()
        || locator
            .chars()
            .any(|c| c.is_control() || matches!(c, '"' | '\\'))
    {
        return false;
    }
    if let Some(url) = locator.strip_prefix("https://") {
        let authority = url.split(['/', '?', '#']).next().unwrap_or_default();
        return !authority.is_empty()
            && !authority.contains('@')
            && !url.chars().any(char::is_whitespace);
    }
    !locator.contains("://") && Path::new(locator).is_relative()
}

/// Edit the dependency entry using the same grammar as the compiler.
pub fn set_archive_dependency(
    path: &Path,
    original: &str,
    name: &str,
    dependency: &ArchiveDependency,
) -> Result<String, ManifestError> {
    if !valid_package_name(name) || !valid_archive_locator(&dependency.archive) {
        return Err(ManifestError::new(
            path,
            "invalid SDK name or archive location",
            "use a package name and an HTTPS URL or relative archive path",
        ));
    }
    let bom = "\u{feff}";
    let text = original.strip_prefix(bom).unwrap_or(original);
    let sections = parse_sections(path, text)?;
    let integrity = dependency
        .sha256
        .as_ref()
        .map_or_else(String::new, |digest| {
            format!(", integrity = \"sha256:{digest}\"")
        });
    let value = format!("{{ archive = \"{}\"{integrity} }}", dependency.archive);
    let mut updated = text.to_owned();
    if let Some(entry) = sections
        .get("dependencies")
        .and_then(|section| section.entries.get(name))
    {
        updated.replace_range(entry.value.clone(), &value);
    } else {
        let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let line = format!("\"{name}\" = {value}{newline}");
        if let Some(section) = sections.get("dependencies") {
            let end = section
                .entries
                .values()
                .map(|entry| entry.value.end)
                .max()
                .unwrap_or(section.span.end);
            if let Some(next) = text[end..].find('\n') {
                updated.insert_str(end + next + 1, &line);
            } else {
                updated.push_str(newline);
                updated.push_str(&line);
            }
        } else {
            if !updated.ends_with('\n') {
                updated.push_str(newline);
            }
            updated.push_str(newline);
            updated.push_str("[dependencies]");
            updated.push_str(newline);
            updated.push_str(&line);
        }
    }
    load_manifest_text(path, &updated)?;
    if original.starts_with(bom) {
        updated.insert_str(0, bom);
    }
    Ok(updated)
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
    values: &Entries,
    allowed: &[&str],
) -> Result<(), ManifestError> {
    if let Some((key, entry)) = values
        .iter()
        .find(|(key, _)| !allowed.contains(&key.as_str()))
    {
        return Err(ManifestError::new(
            path,
            format!("unknown {section} key `{key}`"),
            format!("remove `{key}` or use a supported {section} key"),
        )
        .at(entry.key.clone()));
    }
    Ok(())
}

pub fn valid_package_name(name: &str) -> bool {
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
