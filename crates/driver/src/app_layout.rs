//! Physical paths only. Source descriptors, provider bytes and binding identities stay sealed.
use std::path::{Component, Path};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Layout {
    #[default]
    Sidecars,
    MacosApp,
}

impl Layout {
    pub fn file(self, relative: &str) -> Result<String, String> {
        let path = Path::new(relative);
        if path.as_os_str().is_empty()
            || path
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err("invalid deployed source-package path".into());
        }
        if self == Self::Sidecars {
            return Ok(relative.into());
        }
        if path
            .extension()
            .is_some_and(|extension| extension == "dylib")
        {
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or("non-UTF-8 native filename")?;
            Ok(format!("Frameworks/{name}"))
        } else {
            Ok(format!("Resources/SourcePackages/{relative}"))
        }
    }

    pub fn startup(self, bundle_literal: &str) -> String {
        if self == Self::Sidecars {
            return format!(
                "char *executable_directory = rinzu_source_executable_directory();\nchar *directory = rinzu_source_path(executable_directory, {}); free(executable_directory);\n",
                bundle_literal
            );
        }
        // Keep the existing canonical-path confinement, rooted at Contents for an app.
        // This changes no generic loader behavior and needs no external symlinks.
        "char *directory = rinzu_source_executable_directory();\nchar *macos = strrchr(directory, '/');\nif (!macos || strcmp(macos + 1, \"MacOS\")) rinzu_source_package_failure();\n*macos = 0;\nchar *contents = strrchr(directory, '/');\nif (!contents || strcmp(contents + 1, \"Contents\")) rinzu_source_package_failure();\n".into()
    }
}
