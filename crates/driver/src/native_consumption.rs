//! Optional LLVM21 body analysis. Unknown input always retains callback payload.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use rinzu_bindgen::consumption::{Proof, Report};
use rinzu_bindgen::source_package::sha256;
use rinzu_compiler::{ResolvedSdkPackage, SourceAbiType, SourceNativeSignature};

pub const IR_FLAGS: &[&str] = &[
    "-O1",
    "-fno-builtin",
    "-fno-inline",
    "-fvisibility=hidden",
    "-S",
    "-emit-llvm",
];
const SOURCE: &str = include_str!("../../../tools/native_consumption.cpp");
struct Checker {
    _directory: PathBuf,
    executable: PathBuf,
    digest: String,
    inputs: BTreeMap<PathBuf, String>,
}

fn output(command: &mut Command) -> Result<String, String> {
    let result = command.output().map_err(|error| error.to_string())?;
    if !result.status.success() {
        return Err(String::from_utf8_lossy(&result.stderr).into_owned());
    }
    String::from_utf8(result.stdout).map_err(|error| error.to_string())
}

fn checker() -> Result<&'static Checker, &'static str> {
    static CHECKER: OnceLock<Result<Checker, String>> = OnceLock::new();
    CHECKER
        .get_or_init(|| {
            let config = std::env::var_os("RINZU_LLVM_CONFIG")
                .map_or_else(
                    || {
                        [
                            "llvm-config-21",
                            "/opt/homebrew/opt/llvm@21/bin/llvm-config",
                            "/usr/local/opt/llvm@21/bin/llvm-config",
                        ]
                        .iter()
                        .find(|path| {
                            Command::new(path)
                                .arg("--version")
                                .output()
                                .is_ok_and(|result| result.status.success())
                        })
                        .map(PathBuf::from)
                    },
                    |path| Some(path.into()),
                )
                .ok_or("LLVM21 analysis tool unavailable")?;
            let config = if config.is_file() {
                config
            } else {
                std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                    .map(|directory| directory.join(&config))
                    .find(|path| path.is_file())
                    .ok_or("LLVM21 config executable is not a file")?
            }
            .canonicalize()
            .map_err(|error| error.to_string())?;
            let version = output(Command::new(&config).arg("--version"))?;
            if !version.trim().starts_with("21.") {
                return Err("consumption analysis requires LLVM21".into());
            }
            let bindir = output(Command::new(&config).arg("--bindir"))?;
            let include = output(Command::new(&config).arg("--includedir"))?;
            let libdir = output(Command::new(&config).arg("--libdir"))?;
            let directory = std::env::temp_dir().join(format!(
                "rinzu-consumption-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|error| error.to_string())?
                    .as_nanos()
            ));
            std::fs::create_dir(&directory).map_err(|error| error.to_string())?;
            let source = directory.join("consumption.cpp");
            let executable = directory.join("consumption");
            std::fs::write(&source, SOURCE).map_err(|error| error.to_string())?;
            output(
                Command::new(Path::new(bindir.trim()).join("clang++"))
                    .args(["-std=c++17", "-O2", "-fno-exceptions"])
                    .arg("-I")
                    .arg(include.trim())
                    .arg("-L")
                    .arg(libdir.trim())
                    .arg(&source)
                    .arg("-lLLVM-21")
                    .arg("-o")
                    .arg(&executable),
            )?;
            let library = Path::new(libdir.trim()).join("libLLVM-21.dylib");
            let mut inputs = BTreeMap::new();
            for path in [
                &config,
                &Path::new(bindir.trim()).join("clang++"),
                &executable,
                &library,
            ] {
                inputs.insert(
                    path.canonicalize().map_err(|error| error.to_string())?,
                    sha256(&std::fs::read(path).map_err(|error| error.to_string())?),
                );
            }
            let identity = [
                sha256(SOURCE.as_bytes()),
                sha256(&std::fs::read(&executable).map_err(|error| error.to_string())?),
                sha256(&std::fs::read(library).map_err(|error| error.to_string())?),
                sha256(
                    &serde_json::to_vec(&inputs.values().collect::<Vec<_>>())
                        .map_err(|error| error.to_string())?,
                ),
            ];
            let digest = sha256(&serde_json::to_vec(&identity).map_err(|error| error.to_string())?);
            Ok(Checker {
                _directory: directory,
                executable,
                digest,
                inputs,
            })
        })
        .as_ref()
        .map_err(String::as_str)
}

pub fn parameters(signature: &SourceNativeSignature) -> Vec<usize> {
    signature
        .parameters
        .iter()
        .enumerate()
        .filter_map(|(index, ty)| match ty {
            SourceAbiType::Callback { result, .. }
                if matches!(
                    result.as_ref(),
                    SourceAbiType::String | SourceAbiType::Bytes(_)
                ) =>
            {
                Some(index)
            }
            _ => None,
        })
        .collect()
}

pub fn verify(report: &Report) -> Result<(), String> {
    let Some(expected) = &report.analyzer else {
        return Ok(());
    };
    let checker = checker().map_err(str::to_owned)?;
    if expected != &checker.digest {
        return Err("native proof tool identity changed".into());
    }
    for (path, digest) in &checker.inputs {
        if sha256(&std::fs::read(path).map_err(|error| error.to_string())?) != *digest {
            return Err("native proof tool input changed; check again".into());
        }
    }
    Ok(())
}

/// Creates retained records before any native address is exposed.
pub fn prepare(package: &ResolvedSdkPackage) -> Report {
    let mut functions = BTreeMap::new();
    for signature in &package.source.as_ref().unwrap().signatures {
        let rinzu_compiler::ast::NativeBinding::Symbol(symbol) = &signature.binding else {
            continue;
        };
        let positions = parameters(signature);
        if !positions.is_empty() && !signature.is_async && !signature.is_supplant {
            functions.insert(
                signature.name.clone(),
                Proof {
                    symbol: symbol.clone(),
                    parameters: positions,
                    region: "native-call".into(),
                    ir_digest: None,
                    object_digest: None,
                    closed: false,
                    reasons: vec!["unsupported-build-or-signature".into()],
                },
            );
        }
    }
    Report {
        rule: rinzu_bindgen::consumption::RULE.into(),
        analyzer: None,
        functions,
    }
}

pub fn supported(package: &ResolvedSdkPackage, report: &mut Report) -> bool {
    if report.functions.is_empty()
        || package.manifest.native.sources.len() != 1
        || package.manifest.native.sources[0]
            .extension()
            .is_none_or(|extension| extension != "c")
    {
        return false;
    }
    match checker() {
        Ok(checker) => {
            report.analyzer = Some(checker.digest.clone());
            true
        }
        Err(reason) => {
            for proof in report.functions.values_mut() {
                proof.reasons = vec![reason.to_owned()];
            }
            false
        }
    }
}

pub fn analyze(
    package: &ResolvedSdkPackage,
    report: &mut Report,
    ir: &Path,
    object: &Path,
) -> Result<(), String> {
    let checker = checker().map_err(str::to_owned)?;
    verify(report)?;
    let ir_digest = sha256(&std::fs::read(ir).map_err(|error| error.to_string())?);
    let object_digest = sha256(&std::fs::read(object).map_err(|error| error.to_string())?);
    for signature in &package.source.as_ref().unwrap().signatures {
        let Some(proof) = report.functions.get_mut(&signature.name) else {
            continue;
        };
        proof.ir_digest = Some(ir_digest.clone());
        proof.object_digest = Some(object_digest.clone());
        if !matches!(
            signature.result,
            SourceAbiType::Scalar(_) | SourceAbiType::Unit
        ) || signature
            .parameters
            .iter()
            .filter(|ty| matches!(ty, SourceAbiType::Callback { .. }))
            .count()
            != proof.parameters.len()
        {
            continue;
        }
        let mut command = Command::new(&checker.executable);
        command
            .arg(ir)
            .arg(&proof.symbol)
            .args(proof.parameters.iter().map(usize::to_string));
        let result = output(&mut command).and_then(|text| {
            let rows: Vec<serde_json::Value> =
                serde_json::from_str(&text).map_err(|error| error.to_string())?;
            if rows.len() != 1 {
                return Err("invalid consumption analyzer output".into());
            }
            Ok(rows.into_iter().next().unwrap())
        });
        match result {
            Ok(row) => {
                proof.closed = row["closed"].as_bool().unwrap_or(false);
                proof.reasons = serde_json::from_value(row["reasons"].clone())
                    .unwrap_or_else(|_| vec!["invalid-analyzer-reasons".into()]);
                if !proof.closed && proof.reasons.is_empty() {
                    proof.reasons.push("analysis-unknown".into());
                }
            }
            Err(reason) => {
                proof.closed = false;
                proof.reasons = vec![format!("analysis-unavailable:{reason}")];
            }
        }
    }
    Ok(())
}
