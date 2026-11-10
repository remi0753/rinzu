//! Language and release requirements for a resolved source graph.
use crate::package::package_diagnostic;
use crate::{Diagnostic, Manifest, PackageId, PackageSource, ResolvedGraph};

pub const LANGUAGE: &str = "0.5";
pub const LANGUAGES: &[&str] = &["0.1", "0.2", "0.3", "0.4", LANGUAGE];
pub const RELEASE: &str = env!("CARGO_PKG_VERSION");

pub(crate) fn version_parts<const N: usize>(value: &str) -> Option<[u32; N]> {
    let mut components = value.split('.');
    let mut result = [0; N];
    for part in &mut result {
        let component = components.next()?;
        if !component.bytes().all(|byte| byte.is_ascii_digit())
            || component.len() > 1 && component.starts_with('0')
        {
            return None;
        }
        *part = component.parse().ok()?;
    }
    components.next().is_none().then_some(result)
}

/// Check verified manifest requirements before inspecting SDK sources.
pub fn validate_graph(graph: &ResolvedGraph) -> Result<(), Vec<Diagnostic>> {
    let root = &graph.package(ResolvedGraph::ROOT).manifest;
    if let Some(required) = &root.toolchain_version
        && required != RELEASE
    {
        return Err(diagnostic(
            root,
            format!("project selects Rinzu {required}; running Rinzu {RELEASE}"),
            "invoke the selected local Rinzu toolchain",
        ));
    }
    let root_language = supported_language(graph, ResolvedGraph::ROOT)?;
    let mut minimum_language = None;
    let mut minimum_release = None;
    for (id, package) in graph.packages() {
        let language = supported_language(graph, id)?;
        if matches!(package.location.source, PackageSource::Archive(_))
            && minimum_language.is_none_or(|(previous, _)| language > previous)
        {
            minimum_language = Some((language, id));
        }
        if let Some(required) = &package.manifest.minimum_toolchain {
            let version = version_parts::<3>(required).ok_or_else(|| {
                diagnostic(
                    &package.manifest,
                    format!("invalid minimum Rinzu release `{required}`"),
                    "use a major.minor.patch release number",
                )
            })?;
            if minimum_release.is_none_or(|(previous, _)| version > previous) {
                minimum_release = Some((version, id));
            }
        }
    }
    if let Some((required, id)) = minimum_language
        && root_language < required
    {
        let dependency = &graph.package(id).manifest;
        return Err(diagnostic(
            root,
            format!(
                "project language {} is below required language {} ({}, declared in {})",
                root.language,
                dependency.language,
                graph.dependency_path(id),
                dependency.root.join("rinzu.toml").display()
            ),
            "set the project's language to a supported level that meets the SDK requirements",
        ));
    }
    if let Some((required, id)) = minimum_release
        && !release_at_least(required)
    {
        let dependency = &graph.package(id).manifest;
        return Err(diagnostic(
            dependency,
            format!(
                "Rinzu {} or later is required ({}); running Rinzu {RELEASE}",
                dependency.minimum_toolchain.as_deref().unwrap(),
                graph.dependency_path(id)
            ),
            "invoke a local Rinzu toolchain that meets the SDK requirements",
        ));
    }
    Ok(())
}

fn supported_language(graph: &ResolvedGraph, id: PackageId) -> Result<[u32; 2], Vec<Diagnostic>> {
    let manifest = &graph.package(id).manifest;
    if let Some(version) = version_parts(&manifest.language)
        && LANGUAGES.contains(&manifest.language.as_str())
    {
        return Ok(version);
    }
    Err(diagnostic(
        manifest,
        format!(
            "unsupported language version `{}` ({}; running Rinzu {RELEASE})",
            manifest.language,
            graph.dependency_path(id)
        ),
        format!(
            "use a Rinzu toolchain that supports the language; this release supports {}",
            LANGUAGES.join(", ")
        ),
    ))
}

fn release_at_least(minimum: [u32; 3]) -> bool {
    let release = RELEASE
        .split('+')
        .next()
        .expect("Cargo release is nonempty");
    let (number, preview) = release
        .split_once('-')
        .map_or((release, false), |(number, _)| (number, true));
    let current = version_parts(number).expect("Cargo release has three components");
    current > minimum || current == minimum && !preview
}

fn diagnostic(
    manifest: &Manifest,
    message: impl Into<String>,
    suggestion: impl Into<String>,
) -> Vec<Diagnostic> {
    vec![package_diagnostic(message, suggestion).with_path(manifest.root.join("rinzu.toml"))]
}
