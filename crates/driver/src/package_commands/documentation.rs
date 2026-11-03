//! Render checked public bindings using captured source inputs, never a second disk read.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rinzu_compiler::ast::NativeMember;
use rinzu_compiler::{
    FileId, SourceDeclarationDocumentation, SourceFile, SourceSdkInterface, source_documentation,
};

use super::{ApiEntry, FrontendArtifact, Manifest, PackageApi, output_identity, relative};

struct Origin {
    path: PathBuf,
    declaration: SourceDeclarationDocumentation,
}

fn collect(
    manifest: &Manifest,
    source: &SourceSdkInterface,
    docs: &mut BTreeMap<String, Origin>,
) -> Result<(), String> {
    let prefix = format!("{}/", manifest.name);
    for (id, logical) in &source.source_files {
        // The checked file map also includes transitive dependencies. Each is
        // collected from its own captured package, with its own input bytes.
        let Some(path) = logical.strip_prefix(&prefix) else {
            continue;
        };
        let bytes = source
            .source_inputs
            .get(path)
            .ok_or_else(|| format!("documentation snapshot is missing `{logical}`"))?;
        let file = SourceFile::from_utf8(FileId(*id), logical, bytes.clone())
            .map_err(|errors| format!("documentation UTF-8 error: {errors:?}"))?;
        for declaration in source_documentation(&file)
            .map_err(|errors| format!("documentation syntax error: {errors:?}"))?
        {
            let identity = source
                .syntax
                .declarations
                .iter()
                .find(|linked| linked.range() == declaration.range)
                .map(|linked| linked.name().to_owned())
                .or_else(|| {
                    source
                        .syntax
                        .native_blocks
                        .iter()
                        .flat_map(|block| &block.members)
                        .find_map(|member| match member {
                            NativeMember::Opaque { name, range } if *range == declaration.range => {
                                Some(name.clone())
                            }
                            NativeMember::Record(record) if record.range == declaration.range => {
                                Some(record.name.clone())
                            }
                            NativeMember::Function(function)
                                if function.range == declaration.range =>
                            {
                                Some(function.name.clone())
                            }
                            _ => None,
                        })
                })
                .ok_or_else(|| {
                    format!(
                        "documentation has no checked declaration at `{logical}`:{}",
                        declaration.line
                    )
                })?;
            docs.insert(
                identity,
                Origin {
                    path: manifest.root.join(path),
                    declaration,
                },
            );
        }
    }
    Ok(())
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('`', "\\`")
        .replace('*', "\\*")
        .replace('_', "\\_")
}

fn link(path: &Path) -> String {
    let mut encoded = String::new();
    for byte in path.to_string_lossy().as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'-' | b'_') {
            encoded.push(*byte as char);
        } else {
            use std::fmt::Write;
            write!(encoded, "%{byte:02X}").unwrap();
        }
    }
    encoded
}

fn code(output: &mut String, text: &str, language: &str) {
    let longest = text
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(3.max(longest + 1));
    output.push_str(&format!("{fence}{language}\n{text}\n{fence}\n\n"));
}

fn checked_type(entry: &ApiEntry) -> Option<&str> {
    entry
        .function
        .as_ref()
        .map(|function| function.source_type.as_str())
        .or_else(|| {
            entry
                .source_template
                .as_ref()
                .filter(|template| !template.signature.is_empty())
                .map(|template| template.signature.as_str())
        })
        .or(entry.source_value.as_deref())
        .or_else(|| entry.constant.as_ref().map(|(ty, _)| ty.as_str()))
        .or_else(|| entry.native_type.as_ref().map(|ty| ty.canonical.as_str()))
}

pub(super) fn render(
    api: &PackageApi,
    manifest: &Manifest,
    artifact: &FrontendArtifact,
    output: &Path,
    snapshot: &Path,
) -> Result<String, String> {
    let mut docs = BTreeMap::new();
    for package in &artifact.sdk_packages {
        if let Some(source) = &package.source {
            collect(&package.manifest, source, &mut docs)?;
        }
    }
    if let Some(source) = &artifact.source_interface {
        collect(manifest, source, &mut docs)?;
    }
    // API identities canonicalize transparent aliases. Documentation follows
    // the public binding's declaration so aliases sharing a shape keep their
    // own comments and source links, including dependency reexports.
    let public_origins: BTreeMap<_, _> = artifact
        .source_interface
        .iter()
        .flat_map(|source| &source.modules)
        .flat_map(|(module, exports)| {
            exports
                .iter()
                .map(move |(name, target)| (format!("{module}.{name}"), target))
        })
        .collect();
    let output = output_identity(output)?;
    let parent = output
        .parent()
        .ok_or("Markdown output has no parent directory")?;
    let snapshot = output_identity(snapshot)?;
    let mut result = format!(
        "# {} API reference\n\nGenerated from checked public exports and captured source comments.\n\nLanguage: `{}` · target: `{}` · semantics: `{}`.\n\n[Machine-readable API v{} snapshot]({}).\n\n",
        escape(&api.package),
        api.language,
        api.target,
        api.semantics,
        api.version,
        link(&relative(parent, &snapshot))
    );
    for (alias, entry) in &api.exports {
        result.push_str(&format!("## {}\n\n", escape(alias)));
        if let Some(ty) = checked_type(entry) {
            result.push_str(&format!("Checked type: `{}`.\n\n", ty.replace('`', "\\`")));
        }
        if let Some(origin) = public_origins
            .get(alias)
            .and_then(|target| docs.get(*target))
            .or_else(|| docs.get(&entry.identity))
        {
            let declaration = &origin.declaration;
            result.push_str(&format!(
                "{} · [source]({}#L{}).\n\n",
                declaration.kind,
                link(&relative(
                    parent,
                    &std::path::absolute(&origin.path).map_err(|error| error.to_string())?
                )),
                declaration.line
            ));
            code(&mut result, &declaration.signature, "rinzu");
            if !declaration.documentation.is_empty() {
                result.push_str(&declaration.documentation);
                result.push_str("\n\n");
            }
            for member in &declaration.members {
                if member.is_private {
                    continue;
                }
                result.push_str(&format!("### {} {}\n\n", member.kind, escape(&member.name)));
                result.push_str(&format!(
                    "[source]({}#L{}).\n\n",
                    link(&relative(
                        parent,
                        &std::path::absolute(&origin.path).map_err(|error| error.to_string())?
                    )),
                    member.line
                ));
                code(&mut result, &member.signature, "rinzu");
                if !member.documentation.is_empty() {
                    result.push_str(&member.documentation);
                    result.push_str("\n\n");
                }
            }
        } else {
            // Intrinsics and other exports without a captured source declaration
            // still retain their checked schema rather than an invented header.
            code(
                &mut result,
                &serde_json::to_string_pretty(entry).map_err(|error| error.to_string())?,
                "json",
            );
        }
    }
    while result.ends_with("\n\n") {
        result.truncate(result.len() - 1);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_links_and_fences_preserve_special_names_and_literal_backticks() {
        assert_eq!(
            link(Path::new("../日本語 dir/a#b.rinzu")),
            "../%E6%97%A5%E6%9C%AC%E8%AA%9E%20dir/a%23b.rinzu"
        );
        let mut text = String::new();
        code(&mut text, "const sample = r\"\n```\n\";", "rinzu");
        assert!(text.starts_with("````rinzu\n"));
        assert!(text.ends_with("\n````\n\n"));
    }
}
