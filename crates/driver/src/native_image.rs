//! Fresh image identities, with one shared-cache UUID query per verification group.
use object::Object;
use rinzu_bindgen::native_binding::NativeProviderImage;
use rinzu_bindgen::source_package::sha256;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Default)]
pub struct ImageProbe {
    shared: BTreeMap<PathBuf, NativeProviderImage>,
    reports: BTreeMap<PathBuf, String>,
}

impl ImageProbe {
    pub fn prime<'a>(&mut self, paths: impl IntoIterator<Item = &'a Path>) -> Result<(), String> {
        self.prime_mode(paths, false)
    }

    pub fn prime_full<'a>(
        &mut self,
        paths: impl IntoIterator<Item = &'a Path>,
    ) -> Result<(), String> {
        self.prime_mode(paths, true)
    }

    pub fn report(&self, image: &NativeProviderImage) -> Option<&str> {
        let path = Path::new(&image.path);
        (image.sha256.is_none()
            && self
                .shared
                .get(path)
                .is_some_and(|current| current.uuid == image.uuid))
        .then(|| self.reports.get(path).map(String::as_str))
        .flatten()
    }

    fn prime_mode<'a>(
        &mut self,
        paths: impl IntoIterator<Item = &'a Path>,
        full: bool,
    ) -> Result<(), String> {
        let paths: BTreeSet<_> = paths
            .into_iter()
            .filter(|path| is_system(path) && !path.is_file())
            .map(Path::to_owned)
            .filter(|path| !self.shared.contains_key(path))
            .collect();
        if paths.is_empty() {
            return Ok(());
        }
        let _timing = rinzu_compiler::timing::span(if full {
            "native.image.full.batch"
        } else {
            "native.image.uuid.batch"
        });
        let paths: Vec<_> = paths.into_iter().collect();
        for chunk in paths.chunks(128) {
            let mut command = Command::new("xcrun");
            command.args(["dyld_info", "-arch", "arm64e", "-uuid"]);
            if full {
                command.args(["-dependents", "-exports", "-segments", "-function_starts"]);
            }
            command.args(chunk);
            let output = super::native_binding::run(command, "inspect native UUID/target batch")?;
            let expected = chunk.iter().cloned().collect();
            for (path, report) in split_reports(&output, &expected)? {
                let uuid = report_uuid(&report)?;
                if full {
                    self.reports.insert(path.clone(), report);
                }
                self.shared.insert(
                    path.clone(),
                    NativeProviderImage {
                        path: spelling(&path)?,
                        bundled: false,
                        sha256: None,
                        uuid,
                    },
                );
            }
        }
        Ok(())
    }

    pub fn get(&mut self, path: &Path) -> Result<NativeProviderImage, String> {
        let path = path.canonicalize().unwrap_or_else(|_| path.into());
        let bytes = if path.is_file() {
            Some(std::fs::read(&path).map_err(|error| error.to_string())?)
        } else {
            None
        };
        // Local files are read again even when the same spelling was seen in
        // this operation: callers still detect a concurrent content change.
        if let Some(bytes) = &bytes
            && let Ok(file) = object::File::parse(bytes.as_slice())
            && file.format() == object::BinaryFormat::MachO
            && file.architecture() == object::Architecture::Aarch64
        {
            let _timing = rinzu_compiler::timing::span("native.image.uuid.read");
            let uuid = file
                .mach_uuid()
                .map_err(|error| error.to_string())?
                .ok_or("native image UUID is absent")?;
            return Ok(NativeProviderImage {
                path: spelling(&path)?,
                bundled: false,
                sha256: Some(sha256(bytes)),
                uuid: uuid.iter().map(|byte| format!("{byte:02x}")).collect(),
            });
        }
        if bytes.is_none() && is_system(&path) {
            if let Some(image) = self.shared.get(&path) {
                return Ok(image.clone());
            }
            self.prime([path.as_path()])?;
            return self
                .shared
                .get(&path)
                .cloned()
                .ok_or("native image UUID is absent".into());
        }
        // Universal images and other formats retain the tool's slice selection
        // and diagnostics rather than guessing an architecture from a header.
        let mut command = Command::new("xcrun");
        command
            .args(["dyld_info", "-arch", inspection_arch(&path), "-uuid"])
            .arg(&path);
        let output = super::native_binding::run(
            command,
            &format!("inspect native UUID/target {}", path.display()),
        )?;
        let uuid = parse_uuids(&output, &BTreeSet::from([path.clone()]))?
            .remove(&path)
            .ok_or("native image UUID is absent")?;
        Ok(NativeProviderImage {
            path: spelling(&path)?,
            bundled: false,
            sha256: bytes.as_deref().map(sha256),
            uuid,
        })
    }
}

pub fn image_info(path: &Path) -> Result<NativeProviderImage, String> {
    ImageProbe::default().get(path)
}

fn spelling(path: &Path) -> Result<String, String> {
    let text = path.to_str().ok_or("native input path must be UTF-8")?;
    if text.contains(['\0', '\n', '\r']) {
        return Err("native input path contains a control character".into());
    }
    Ok(text.into())
}

fn is_system(path: &Path) -> bool {
    path.starts_with("/usr/lib") || path.starts_with("/System/Library")
}

pub(super) fn inspection_arch(path: &Path) -> &'static str {
    if is_system(path) { "arm64e" } else { "arm64" }
}

fn split_reports(
    output: &str,
    expected: &BTreeSet<PathBuf>,
) -> Result<BTreeMap<PathBuf, String>, String> {
    let mut current = None;
    let mut reports = BTreeMap::<PathBuf, String>::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('/') && trimmed.ends_with("]:") {
            let (path, arch) = trimmed
                .rsplit_once(" [")
                .ok_or("native image target is absent")?;
            if !matches!(arch, "arm64]:" | "arm64e]:") {
                return Err(format!(
                    "unsupported native image target: {trimmed}; expected arm64/aarch64-apple-darwin"
                ));
            }
            let path = PathBuf::from(path);
            if !expected.contains(&path) || reports.insert(path.clone(), String::new()).is_some() {
                return Err("ambiguous native image UUID/target output".into());
            }
            current = Some(path);
        }
        if let Some(path) = &current {
            let report = reports.get_mut(path).unwrap();
            report.push_str(line);
            report.push('\n');
        } else if !trimmed.is_empty() {
            return Err("native report has no image target".into());
        }
    }
    if reports.keys().ne(expected.iter()) {
        return Err("native image UUID/target output is incomplete".into());
    }
    Ok(reports)
}

fn report_uuid(report: &str) -> Result<String, String> {
    let mut found = None;
    for line in report.lines().map(str::trim) {
        if line.len() == 36
            && line.bytes().enumerate().all(|(index, byte)| {
                if matches!(index, 8 | 13 | 18 | 23) {
                    byte == b'-'
                } else {
                    byte.is_ascii_hexdigit()
                }
            })
            && found
                .replace(line.replace('-', "").to_lowercase())
                .is_some()
        {
            return Err("ambiguous native image UUID output".into());
        }
    }
    found.ok_or("native image UUID is absent".into())
}

fn parse_uuids(
    output: &str,
    expected: &BTreeSet<PathBuf>,
) -> Result<BTreeMap<PathBuf, String>, String> {
    split_reports(output, expected)?
        .into_iter()
        .map(|(path, report)| Ok((path, report_uuid(&report)?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uuid_batch_requires_one_uuid_and_supported_target_per_requested_image() {
        let paths = BTreeSet::from([PathBuf::from("/usr/lib/one"), PathBuf::from("/usr/lib/two")]);
        let one =
            "/usr/lib/one [arm64e]:\n    -uuid:\n        01234567-89AB-CDEF-0123-456789ABCDEF\n";
        let two =
            "/usr/lib/two [arm64e]:\n    -uuid:\n        FEDCBA98-7654-3210-FEDC-BA9876543210\n";
        let output = format!("{one}{two}");
        assert_eq!(parse_uuids(&output, &paths).unwrap().len(), 2);
        assert!(parse_uuids(one, &paths).is_err());
        assert!(parse_uuids(&format!("{output}{one}"), &paths).is_err());
        assert!(parse_uuids(&output.replace("arm64e", "x86_64"), &paths).is_err());
        assert!(parse_uuids(&output.replace("FEDCBA98", "NOT-UUID"), &paths).is_err());
    }
    #[test]
    fn combined_inspections_keep_each_image_report_separate() {
        let paths = BTreeSet::from([PathBuf::from("/usr/lib/one"), PathBuf::from("/usr/lib/two")]);
        let one = "/usr/lib/one [arm64e]:\n-uuid:\n01234567-89AB-CDEF-0123-456789ABCDEF\n-exports:\n  0x1234 _only_one [weak-def]\n";
        let two = "/usr/lib/two [arm64e]:\n-uuid:\nFEDCBA98-7654-3210-FEDC-BA9876543210\n-exports:\n  0x4321 _only_two\n";
        let output = format!("{one}{two}");
        let reports = split_reports(&output, &paths).unwrap();
        assert_eq!(reports[Path::new("/usr/lib/one")], one);
        assert_eq!(reports[Path::new("/usr/lib/two")], two);
        let uuids = parse_uuids(&output, &paths).unwrap();
        assert_ne!(
            uuids[Path::new("/usr/lib/one")],
            uuids[Path::new("/usr/lib/two")]
        );
        assert!(report_uuid(&format!("{one}01234567-89AB-CDEF-0123-456789ABCDEF\n")).is_err());
    }

    #[test]
    fn direct_mach_uuid_matches_target_tool_and_current_bytes() {
        let path = std::env::current_exe().unwrap().canonicalize().unwrap();
        let image = image_info(&path).unwrap();
        let mut command = Command::new("xcrun");
        command
            .args(["dyld_info", "-arch", "arm64", "-uuid"])
            .arg(&path);
        let output = super::super::native_binding::run(command, "test image UUID").unwrap();
        let expected = parse_uuids(&output, &BTreeSet::from([path.clone()])).unwrap();
        assert_eq!(image.uuid, expected[&path]);
        assert_eq!(image.sha256, Some(sha256(&std::fs::read(path).unwrap())));
    }
}
