use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    pub schema: String,
    pub identifier: String,
    pub name: String,
    #[serde(default = "version")]
    pub version: String,
    #[serde(default = "build")]
    pub build: String,
    #[serde(default = "minimum")]
    pub minimum_system_version: String,
    pub icon: Option<PathBuf>,
    #[serde(default)]
    pub resources: Vec<PathBuf>,
}

fn version() -> String {
    "0.1.0".into()
}
fn build() -> String {
    "1".into()
}
fn minimum() -> String {
    "11.0".into()
}

pub fn relative(path: &Path) -> Result<&str, String> {
    let text = path.to_str().ok_or("resource paths must be UTF-8")?;
    if text.is_empty()
        || text.len() > 4096
        || text.chars().any(char::is_control)
        || text
            .split('/')
            .any(|s| s.is_empty() || matches!(s, "." | ".."))
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(format!(
            "expected a normalized package-relative path: {}",
            path.display()
        ));
    }
    Ok(text)
}

fn numeric(text: &str, count: std::ops::RangeInclusive<usize>) -> bool {
    let parts = text.split('.').collect::<Vec<_>>();
    text.len() <= 32
        && count.contains(&parts.len())
        && parts.iter().all(|part| {
            !part.is_empty() && part.len() <= 5 && part.bytes().all(|b| b.is_ascii_digit())
        })
}

impl Config {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let config: Self =
            serde_json::from_slice(bytes).map_err(|e| format!("invalid macos-app.json: {e}"))?;
        if config.schema != "rinzu.macos-app-v1" {
            return Err("unsupported macOS app configuration schema".into());
        }
        if config.identifier.len() > 255
            || config.identifier.split('.').count() < 2
            || config
                .identifier
                .split('.')
                .any(|s| s.is_empty() || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        {
            return Err(
                "bundle identifier must contain dot-separated ASCII alphanumeric/hyphen components"
                    .into(),
            );
        }
        if config.name.trim().is_empty()
            || config.name.len() > 128
            || config
                .name
                .chars()
                .any(|c| c.is_control() || c == '/' || c == ':')
            || matches!(config.name.as_str(), "." | "..")
        {
            return Err(
                "bundle name must be a nonempty file name of at most 128 UTF-8 bytes".into(),
            );
        }
        if !numeric(&config.version, 3..=3)
            || !numeric(&config.build, 1..=3)
            || config
                .build
                .split('.')
                .enumerate()
                .any(|(index, part)| part.len() > if index == 0 { 4 } else { 2 })
            || config
                .build
                .split('.')
                .next()
                .and_then(|s| s.parse::<u32>().ok())
                .is_none_or(|n| n == 0)
        {
            return Err("version requires three numeric components; build requires one to three and a positive first component".into());
        }
        if !numeric(&config.minimum_system_version, 2..=3)
            || config
                .minimum_system_version
                .split('.')
                .next()
                .and_then(|s| s.parse::<u32>().ok())
                .is_none_or(|n| n < 11)
        {
            return Err("minimumSystemVersion must be a numeric macOS version >= 11.0".into());
        }
        if config.resources.len() > 64 {
            return Err("at most 64 resource roots are supported".into());
        }
        for path in config.resources.iter().chain(config.icon.iter()) {
            relative(path)?;
        }
        if config
            .icon
            .as_ref()
            .is_some_and(|p| !p.extension().is_some_and(|s| s == "png" || s == "icns"))
        {
            return Err("app icon must be a 1024x1024 PNG or ICNS file".into());
        }
        Ok(config)
    }

    pub fn plist(&self, executable: &str) -> String {
        let mut entries = vec![
            ("CFBundleExecutable", executable),
            ("CFBundleIdentifier", self.identifier.as_str()),
            ("CFBundleName", self.name.as_str()),
            ("CFBundleDisplayName", self.name.as_str()),
            ("CFBundlePackageType", "APPL"),
            ("CFBundleInfoDictionaryVersion", "6.0"),
            ("CFBundleShortVersionString", self.version.as_str()),
            ("CFBundleVersion", self.build.as_str()),
            (
                "LSMinimumSystemVersion",
                self.minimum_system_version.as_str(),
            ),
            ("NSPrincipalClass", "NSApplication"),
            ("RinzuResourceDirectory", "app"),
        ];
        if self.icon.is_some() {
            entries.push(("CFBundleIconFile", "Icon.icns"));
        }
        let mut xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n".to_owned();
        for (key, value) in entries {
            xml.push_str(&format!(
                "<key>{key}</key><string>{}</string>\n",
                escape(value)
            ));
        }
        xml.push_str("<key>NSHighResolutionCapable</key><true/>\n</dict></plist>\n");
        xml
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
