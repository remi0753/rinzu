//! Transactional v2 table validation shared by future JIT facade/AOT startup paths.
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CStr, c_void};
use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use rinzu_abi::{SourceImportV2, SourcePackageDescriptorV2};
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use rinzu_bindgen::source_package::sha256;
use rinzu_bindgen::source_package::{SourceDescriptor, decode_key, validate_source_descriptor};

#[derive(Clone, Debug)]
pub struct SourcePackageArtifact {
    /// None for service-only interfaces.
    pub library: Option<PathBuf>,
    pub expected_wire: Vec<u8>,
    /// Final facade digest supplied by the build, not embedded in itself.
    pub facade_digest: Option<String>,
}

#[derive(Debug)]
pub struct ValidatedSourceImport {
    pub identity: String,
    pub symbol: String,
    address: *const c_void,
}

impl ValidatedSourceImport {
    /// Address for signature-specific generated glue; no uniform invocation ABI.
    #[must_use]
    pub fn glue_address(&self) -> *const c_void {
        self.address
    }
}

#[derive(Debug, Default)]
pub struct LoadedSourcePackages {
    imports: BTreeMap<[u8; 32], ValidatedSourceImport>,
    descriptors: Vec<SourceDescriptor>,
}

impl LoadedSourcePackages {
    /// Closed consumers are bound to this exact validated image and all callback
    /// parameters. Absence of metadata keeps the historical process lifetime.
    pub fn return_consumers(
        &self,
        signatures: &[rinzu_compiler::SourceNativeSignature],
    ) -> Result<BTreeSet<String>> {
        return_consumers(&self.descriptors, signatures)
    }
    /// Resolve checked source identities against the exact validated public ABI.
    /// Returned addresses remain process-pinned with their facade, independently
    /// of this loader's lifetime; no dependency/ambient symbol fallback is used.
    pub(crate) fn source_bindings(
        &self,
        module: &rinzu_compiler::IrModule,
    ) -> Result<BTreeMap<String, (rinzu_compiler::SourceNativeSignature, usize)>> {
        crate::source_native_record_layouts(module, &self.descriptors)?;
        let mut bindings = BTreeMap::new();
        for signature in module
            .source_native_signatures
            .iter()
            .filter(|signature| !signature.is_async)
        {
            let record = self
                .descriptors
                .iter()
                .flat_map(|descriptor| &descriptor.interface.functions)
                .find(|record| record.identity == signature.name)
                .ok_or_else(|| {
                    anyhow::anyhow!("missing source native declaration {}", signature.name)
                })?;
            ensure!(
                record.kind
                    == if signature.is_supplant {
                        rinzu_bindgen::source_package::FunctionKind::Supplant
                    } else {
                        rinzu_bindgen::source_package::FunctionKind::Native
                    }
                    && record.source_type == signature.canonical_type()
                    && record.parameter_requirements
                        == signature
                            .parameters
                            .iter()
                            .map(rinzu_compiler::SourceAbiType::canonical_name)
                            .collect::<Vec<_>>(),
                "source native signature disagrees with validated artifact: {}",
                signature.name
            );
            let import = self
                .imports
                .values()
                .find(|import| import.identity == signature.name)
                .ok_or_else(|| {
                    anyhow::anyhow!("missing source native binding {}", signature.name)
                })?;
            ensure!(
                !bindings.contains_key(&signature.name),
                "duplicate source native identity"
            );
            bindings.insert(
                signature.name.clone(),
                (signature.clone(), import.glue_address() as usize),
            );
        }
        Ok(bindings)
    }

    #[must_use]
    pub fn imports(&self) -> &BTreeMap<[u8; 32], ValidatedSourceImport> {
        &self.imports
    }
    #[must_use]
    pub fn descriptors(&self) -> &[SourceDescriptor] {
        &self.descriptors
    }

    /// Validate all packages before returning any binding. Successful facades are
    /// process-lifetime pinned; failed transactions close every temporary handle.
    pub fn load(artifacts: &[SourcePackageArtifact]) -> Result<Self> {
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            let _ = artifacts;
            anyhow::bail!("source-package v2 loader requires macOS arm64");
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            use std::ffi::CString;
            use std::os::unix::ffi::OsStrExt;
            let mut staged = Self::default();
            let mut handles = Vec::new();
            let mut providers = Vec::new();
            let mut packages = BTreeSet::new();
            for artifact in artifacts {
                let expected = validate_source_descriptor(&artifact.expected_wire)
                    .map_err(anyhow::Error::msg)?;
                ensure!(
                    packages.insert(expected.interface.package.package.clone()),
                    "duplicate package identity"
                );
                let has_native = expected
                    .interface
                    .functions
                    .iter()
                    .any(|function| function.symbol.is_some());
                if !has_native {
                    ensure!(
                        artifact.library.is_none() && artifact.facade_digest.is_none(),
                        "service-only package must not supply code artifact"
                    );
                    staged.descriptors.push(expected);
                    continue;
                }
                let path = rinzu_runtime::allocation::checked_io(
                    artifact
                        .library
                        .as_ref()
                        .context("native package has no facade")?
                        .canonicalize(),
                )?;
                let bytes = rinzu_runtime::allocation::checked_io(std::fs::read(&path))?;
                ensure!(
                    artifact.facade_digest.as_deref() == Some(sha256(&bytes).as_str()),
                    "facade byte digest mismatch"
                );
                let c_path = CString::new(path.as_os_str().as_bytes())?;
                let handle = crate::native_library::open(&c_path);
                ensure!(
                    !handle.is_null(),
                    "cannot load explicit source-package facade"
                );
                let handle = Facade(handle);
                providers.extend(crate::native_binding::initialize(
                    &path, handle.0, &expected,
                )?);
                let entry_name = CString::new(rinzu_abi::SOURCE_PACKAGE_ENTRY_V2)?;
                // SAFETY: live handle and static NUL-terminated entry symbol.
                let entry_address = unsafe { crate::native_library::symbol(handle.0, &entry_name) };
                ensure!(
                    !entry_address.is_null(),
                    "missing rinzu_source_package_v2; v1 fallback forbidden"
                );
                // Reject a dependency/ambient entry instead of this artifact's own entry.
                let mut info: libc::Dl_info = unsafe {
                    // SAFETY: Dl_info is a C POD whose all-zero representation is valid.
                    core::mem::zeroed()
                };
                // SAFETY: valid symbol pointer; writable Dl_info storage.
                let identified = unsafe { libc::dladdr(entry_address, &mut info) };
                ensure!(
                    identified != 0 && !info.dli_fname.is_null(),
                    "unknown source entry image"
                );
                // SAFETY: successful dladdr returns a NUL-terminated image filename.
                let image = unsafe { CStr::from_ptr(info.dli_fname) }.to_str()?;
                ensure!(
                    rinzu_runtime::allocation::checked_io(PathBuf::from(image).canonicalize())?
                        == path,
                    "source entry belongs to another image"
                );
                // SAFETY: fixed v2 entry symbol follows the separate source package ABI.
                let entry: rinzu_abi::SourcePackageEntryV2 =
                    unsafe { core::mem::transmute(entry_address) };
                // SAFETY: package contract provides process-lifetime readable table storage.
                let bindings = unsafe { validate_source_table(entry(), &artifact.expected_wire) }?;
                for binding in bindings.values() {
                    let symbol = CString::new(binding.symbol.as_str())?;
                    // SAFETY: live explicit facade handle and a checked symbol string.
                    let address = unsafe { crate::native_library::symbol(handle.0, &symbol) };
                    ensure!(
                        address.cast_const() == binding.glue_address(),
                        "source import address disagrees with its symbol"
                    );
                    // SAFETY: the non-null validated native address has readable dladdr metadata.
                    let identified = unsafe { libc::dladdr(address, &mut info) };
                    ensure!(
                        identified != 0 && !info.dli_fname.is_null(),
                        "unknown source import image"
                    );
                    // SAFETY: successful dladdr supplies a NUL-terminated image filename.
                    let image = unsafe { CStr::from_ptr(info.dli_fname) }.to_str()?;
                    ensure!(
                        rinzu_runtime::allocation::checked_io(PathBuf::from(image).canonicalize())?
                            == path,
                        "source import belongs to another image"
                    );
                }
                merge(&mut staged.imports, bindings)?;
                staged.descriptors.push(expected);
                handles.push(handle);
            }
            // Publish only after every package has validated. No dynamic unload for v2.
            for handle in handles {
                core::mem::forget(handle);
            }
            for handle in providers {
                core::mem::forget(handle);
            }
            Ok(staged)
        }
    }

    /// AOT startup uses the same validator and publication transaction as JIT.
    /// # Safety
    /// Every entry/table must follow the readable static-storage v2 ABI contract.
    pub unsafe fn from_aot_tables(
        tables: &[(*const SourcePackageDescriptorV2, &[u8])],
    ) -> Result<Self> {
        let mut staged = Self::default();
        let mut packages = BTreeSet::new();
        for (table, wire) in tables {
            let descriptor = validate_source_descriptor(wire).map_err(anyhow::Error::msg)?;
            ensure!(
                packages.insert(descriptor.interface.package.package.clone()),
                "duplicate package identity"
            );
            // SAFETY: caller guarantees each table's readable static storage.
            merge(&mut staged.imports, unsafe {
                validate_source_table(*table, wire)
            }?)?;
            staged.descriptors.push(descriptor);
        }
        Ok(staged)
    }
}

pub fn return_consumers(
    descriptors: &[SourceDescriptor],
    signatures: &[rinzu_compiler::SourceNativeSignature],
) -> Result<BTreeSet<String>> {
    use rinzu_compiler::SourceAbiType;
    let mut selected = BTreeSet::new();
    for descriptor in descriptors {
        let Some(report) =
            rinzu_bindgen::consumption::decode(&descriptor.interface.package.build_inputs)
                .map_err(anyhow::Error::msg)?
        else {
            continue;
        };
        for (identity, proof) in report.functions {
            if !proof.closed {
                continue;
            }
            let Some(signature) = signatures
                .iter()
                .find(|signature| signature.name == identity)
            else {
                continue; // An application need not import every exported native function.
            };
            let parameters = signature
                .parameters
                .iter()
                .enumerate()
                .filter_map(|(index, ty)| {
                    matches!(ty, SourceAbiType::Callback { result, .. }
                    if matches!(result.as_ref(), SourceAbiType::String | SourceAbiType::Bytes(_)))
                    .then_some(index)
                })
                .collect::<Vec<_>>();
            ensure!(
                !signature.is_async
                    && !signature.is_supplant
                    && matches!(
                        signature.result,
                        SourceAbiType::Scalar(_) | SourceAbiType::Unit
                    )
                    && signature
                        .parameters
                        .iter()
                        .filter(|ty| matches!(ty, SourceAbiType::Callback { .. }))
                        .count()
                        == parameters.len()
                    && proof.parameters == parameters
                    && matches!(&signature.binding, rinzu_compiler::ast::NativeBinding::Symbol(symbol) if *symbol == proof.symbol),
                "consumption proof disagrees with native signature: {identity}"
            );
            ensure!(
                selected.insert(identity),
                "duplicate consumption proof identity"
            );
        }
    }
    Ok(selected)
}

fn merge(
    destination: &mut BTreeMap<[u8; 32], ValidatedSourceImport>,
    bindings: BTreeMap<[u8; 32], ValidatedSourceImport>,
) -> Result<()> {
    for (key, binding) in bindings {
        ensure!(
            !destination.contains_key(&key),
            "duplicate import identity across packages"
        );
        destination.insert(key, binding);
    }
    Ok(())
}

/// Validate before exposing any untyped address, with exact expected wire/keys.
/// # Safety
/// The pointer must be null or point to a readable v2 prefix; after a valid
/// prefix, all count/pointer pairs and strings must describe readable storage.
/// Native packages are trusted code, not a memory-sandboxed wire protocol.
pub unsafe fn validate_source_table(
    table: *const SourcePackageDescriptorV2,
    expected_wire: &[u8],
) -> Result<BTreeMap<[u8; 32], ValidatedSourceImport>> {
    let expected = validate_source_descriptor(expected_wire).map_err(anyhow::Error::msg)?;
    ensure!(!table.is_null(), "null source descriptor");
    #[repr(C)]
    struct Prefix {
        size: u32,
        major: u16,
        minor: u16,
    }
    // SAFETY: caller guarantees readable fixed prefix; do not read the tail yet.
    let prefix = unsafe { &*table.cast::<Prefix>() };
    ensure!(
        prefix.size as usize == size_of::<SourcePackageDescriptorV2>()
            && prefix.major == rinzu_abi::SOURCE_PACKAGE_MAJOR
            && prefix.minor == rinzu_abi::SOURCE_PACKAGE_MINOR,
        "source table size/version mismatch"
    );
    // SAFETY: matching prefix and caller contract guarantee the entire table.
    let table = unsafe { &*table };
    ensure!(
        table.descriptor_len == expected_wire.len() && !table.descriptor.is_null(),
        "descriptor byte length/null mismatch"
    );
    // SAFETY: validated length and caller's readable storage contract.
    let wire = unsafe { core::slice::from_raw_parts(table.descriptor, table.descriptor_len) };
    let actual = validate_source_descriptor(wire).map_err(anyhow::Error::msg)?;
    ensure!(
        actual == expected,
        "descriptor target/interface/digest/header mismatch"
    );
    let native = actual
        .interface
        .functions
        .iter()
        .zip(&actual.import_keys)
        .filter_map(|(function, key)| {
            function
                .symbol
                .as_ref()
                .map(|symbol| (function, symbol, key))
        })
        .collect::<Vec<_>>();
    ensure!(
        table.import_count == native.len(),
        "import table count mismatch"
    );
    let entries: &[SourceImportV2] = if table.import_count == 0 {
        &[]
    } else {
        ensure!(!table.imports.is_null(), "null import table");
        // SAFETY: count checked against expected records and readable storage contract.
        unsafe { core::slice::from_raw_parts(table.imports, table.import_count) }
    };
    let mut staged = BTreeMap::new();
    let mut symbols = BTreeSet::new();
    for entry in entries {
        ensure!(
            !entry.symbol.is_null() && !entry.address.is_null(),
            "null native symbol/address"
        );
        // SAFETY: package promises static NUL-terminated symbol storage.
        let symbol = unsafe { CStr::from_ptr(entry.symbol) }.to_str()?;
        let (function, _, key) = native
            .iter()
            .find(|(_, expected_symbol, _)| expected_symbol.as_str() == symbol)
            .context("unexpected native symbol")?;
        ensure!(
            entry.import_key == decode_key(key).map_err(anyhow::Error::msg)?,
            "native import key mismatch"
        );
        ensure!(
            symbols.insert(symbol) && !staged.contains_key(&entry.import_key),
            "duplicate native symbol/key"
        );
        staged.insert(
            entry.import_key,
            ValidatedSourceImport {
                identity: function.identity.clone(),
                symbol: symbol.into(),
                address: entry.address,
            },
        );
    }
    Ok(staged)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
struct Facade(*mut c_void);
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
impl Drop for Facade {
    fn drop(&mut self) {
        // SAFETY: owns one successful dlopen handle that has not been closed.
        unsafe { crate::native_library::close(self.0) };
    }
}
