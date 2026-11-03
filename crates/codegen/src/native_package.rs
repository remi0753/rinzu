//! Validated native SDK package loading shared by JIT and AOT runners.

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail, ensure};
use rinzu_abi::{
    ABI_VERSION, NATIVE_PACKAGE_ENTRY_V1, NativeFunctionV1, NativeInvocationContextV1,
    NativePackageDescriptorV1, NativePackageEntryV1, STATUS_OK, is_compatible,
};
use rinzu_compiler::NativeImport;

#[derive(Clone, Debug)]
pub struct NativePackageArtifact {
    pub package_name: String,
    pub library: PathBuf,
    pub imports: Vec<NativeImport>,
}

#[derive(Clone, Copy)]
struct LoadedImport {
    function: NativeFunctionV1,
}

pub struct LoadedNativePackages {
    imports: HashMap<u64, LoadedImport>,
    exports: HashMap<String, u64>,
    #[cfg(target_os = "macos")]
    handles: Vec<*mut core::ffi::c_void>,
}

impl std::fmt::Debug for LoadedNativePackages {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LoadedNativePackages")
            .field("imports", &self.imports.len())
            .field("exports", &self.exports)
            .finish_non_exhaustive()
    }
}

impl LoadedNativePackages {
    pub fn load(artifacts: &[NativePackageArtifact]) -> Result<Self> {
        #[cfg(not(target_os = "macos"))]
        {
            let _ = artifacts;
            bail!("native SDK package loading currently requires macOS")
        }
        #[cfg(target_os = "macos")]
        {
            let mut loaded = Self {
                imports: HashMap::new(),
                exports: HashMap::new(),
                handles: Vec::new(),
            };
            for artifact in artifacts {
                loaded.load_one(artifact)?;
            }
            Ok(loaded)
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.imports.is_empty()
    }

    #[must_use]
    pub fn import_key(&self, export: &str) -> Option<u64> {
        self.exports.get(export).copied()
    }

    #[must_use]
    pub(crate) fn export_bindings(&self) -> HashMap<String, u64> {
        self.exports.clone()
    }

    pub fn invoke(
        &self,
        import_key: u64,
        invocation: &NativeInvocationContextV1,
        arguments: &[i64],
    ) -> Result<i64> {
        let import = self
            .imports
            .get(&import_key)
            .ok_or_else(|| anyhow!("native import key 0x{import_key:016x} is not loaded"))?;
        let mut result = 0_i64;
        // SAFETY: load validates the package entry table and signature string;
        // arguments and invocation storage remain live for this synchronous call.
        let status = unsafe {
            (import.function)(invocation, arguments.as_ptr(), arguments.len(), &mut result)
        };
        ensure!(
            status == STATUS_OK,
            "native package call failed with status {status}"
        );
        Ok(result)
    }

    #[cfg(target_os = "macos")]
    fn load_one(&mut self, artifact: &NativePackageArtifact) -> Result<()> {
        use std::os::unix::ffi::OsStrExt;

        let path = CString::new(artifact.library.as_os_str().as_bytes())
            .context("native package path contains NUL")?;
        let handle = crate::native_library::open(&path);
        if handle.is_null() {
            bail!(
                "load native package {}: {}",
                artifact.library.display(),
                dlerror()
            );
        }
        let entry_name = CString::new(NATIVE_PACKAGE_ENTRY_V1).expect("entry has no NUL");
        // SAFETY: handle is live and entry_name is NUL-terminated.
        let address = unsafe { crate::native_library::symbol(handle, &entry_name) };
        if address.is_null() {
            // SAFETY: handle was returned by dlopen and has not been closed.
            unsafe { crate::native_library::close(handle) };
            bail!(
                "native package {} has no {} entry: {}",
                artifact.library.display(),
                NATIVE_PACKAGE_ENTRY_V1,
                dlerror()
            );
        }
        // SAFETY: the fixed entry symbol has NativePackageEntryV1 ABI by contract.
        let entry: NativePackageEntryV1 = unsafe { core::mem::transmute(address) };
        // SAFETY: the package promises process-lifetime descriptor storage.
        let descriptor = unsafe { entry() };
        if descriptor.is_null() {
            // SAFETY: handle was returned by dlopen and has not been closed.
            unsafe { crate::native_library::close(handle) };
            bail!("native package entry returned a null descriptor");
        }
        // SAFETY: the pointer is non-null and points at the fixed descriptor prefix.
        let descriptor = unsafe { &*descriptor };
        self.validate_descriptor(artifact, descriptor)
            .inspect_err(|_| {
                // SAFETY: validation failed before any function pointer was published.
                unsafe { crate::native_library::close(handle) };
            })?;
        self.handles.push(handle);
        Ok(())
    }

    fn validate_descriptor(
        &mut self,
        artifact: &NativePackageArtifact,
        descriptor: &NativePackageDescriptorV1,
    ) -> Result<()> {
        ensure!(
            usize::try_from(descriptor.struct_size)? >= size_of::<NativePackageDescriptorV1>(),
            "native package descriptor is truncated"
        );
        ensure!(
            is_compatible(descriptor.abi_version),
            "native package ABI 0x{:08x} is incompatible with 0x{ABI_VERSION:08x}",
            descriptor.abi_version
        );
        ensure!(
            !descriptor.package_name.is_null(),
            "native package name is null"
        );
        // SAFETY: descriptor validation requires a NUL-terminated static package name.
        let package_name = unsafe { CStr::from_ptr(descriptor.package_name) }
            .to_str()
            .context("native package name is not UTF-8")?;
        ensure!(
            package_name == artifact.package_name,
            "native artifact names `{package_name}`, expected `{}`",
            artifact.package_name
        );
        ensure!(
            descriptor.import_count <= 1024,
            "native import table is unbounded"
        );
        ensure!(
            descriptor.import_count == 0 || !descriptor.imports.is_null(),
            "native import table is null"
        );
        // SAFETY: package descriptor promises import_count readable static entries.
        let imports =
            unsafe { std::slice::from_raw_parts(descriptor.imports, descriptor.import_count) };
        ensure!(
            imports.len() == artifact.imports.len(),
            "native import count mismatch for {}",
            artifact.package_name
        );
        for expected in &artifact.imports {
            let native = imports
                .iter()
                .find(|native| native.import_key == expected.import_key)
                .ok_or_else(|| {
                    anyhow!(
                        "native package {} omitted import {}",
                        artifact.package_name,
                        expected.export_name
                    )
                })?;
            ensure!(
                !native.canonical_key.is_null() && !native.signature.is_null(),
                "native import strings are null"
            );
            // SAFETY: descriptor contract requires NUL-terminated static strings.
            let canonical = unsafe { CStr::from_ptr(native.canonical_key) }
                .to_str()
                .context("native canonical key is not UTF-8")?;
            // SAFETY: descriptor contract requires NUL-terminated static strings.
            let signature = unsafe { CStr::from_ptr(native.signature) }
                .to_str()
                .context("native signature is not UTF-8")?;
            ensure!(
                canonical == expected.canonical_key && signature == expected.abi_signature,
                "native import metadata mismatch for {}",
                expected.export_name
            );
            ensure!(native.address != 0, "native import address is null");
            // SAFETY: descriptor signature has been matched to the uniform v1 ABI.
            let function: NativeFunctionV1 = unsafe { core::mem::transmute(native.address) };
            ensure!(
                self.imports
                    .insert(expected.import_key, LoadedImport { function })
                    .is_none(),
                "duplicate native import key 0x{:016x}",
                expected.import_key
            );
            ensure!(
                self.exports
                    .insert(expected.export_name.clone(), expected.import_key)
                    .is_none(),
                "duplicate native export {}",
                expected.export_name
            );
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
impl Drop for LoadedNativePackages {
    fn drop(&mut self) {
        self.imports.clear();
        self.exports.clear();
        for handle in self.handles.drain(..).rev() {
            // SAFETY: handles were returned by dlopen and are closed exactly once.
            unsafe { crate::native_library::close(handle) };
        }
    }
}

#[cfg(target_os = "macos")]
fn dlerror() -> String {
    // SAFETY: dlerror returns null or a process-owned NUL-terminated string.
    let error = unsafe { libc::dlerror() };
    if error.is_null() {
        "unknown dynamic loader error".into()
    } else {
        // SAFETY: checked non-null and dlerror guarantees a C string.
        unsafe { CStr::from_ptr(error) }
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rinzu_abi::{NativeImportV1, STATUS_OK};

    unsafe extern "C" fn test_native(
        _invocation: *const NativeInvocationContextV1,
        _arguments: *const i64,
        _argument_count: usize,
        out_result: *mut i64,
    ) -> i32 {
        if !out_result.is_null() {
            // SAFETY: the test passes writable result storage.
            unsafe { *out_result = 0 };
        }
        STATUS_OK
    }

    fn empty_loader() -> LoadedNativePackages {
        LoadedNativePackages {
            imports: HashMap::new(),
            exports: HashMap::new(),
            #[cfg(target_os = "macos")]
            handles: Vec::new(),
        }
    }

    #[test]
    fn descriptor_validation_rejects_an_incompatible_abi() {
        let artifact = NativePackageArtifact {
            package_name: "rinzu.test".into(),
            library: PathBuf::from("unused"),
            imports: Vec::new(),
        };
        let descriptor = NativePackageDescriptorV1 {
            struct_size: size_of::<NativePackageDescriptorV1>() as u32,
            abi_version: 2 << 24,
            package_name: c"rinzu.test".as_ptr(),
            metadata_digest: std::ptr::null(),
            metadata_digest_size: 0,
            reserved: 0,
            import_count: 0,
            imports: std::ptr::null(),
        };
        let error = empty_loader()
            .validate_descriptor(&artifact, &descriptor)
            .expect_err("incompatible ABI must be rejected");
        assert!(error.to_string().contains("incompatible"), "{error:#}");
    }

    #[test]
    fn descriptor_validation_rejects_metadata_drift_before_publication() {
        let expected = NativeImport {
            export_name: "rinzu.test.run".into(),
            import_key: 7,
            canonical_key: "rinzu.test@0.2|rinzu.test.run|i64()".into(),
            abi_signature: "i64()".into(),
            symbol: "test_native".into(),
            runtime_service: None,
        };
        let artifact = NativePackageArtifact {
            package_name: "rinzu.test".into(),
            library: PathBuf::from("unused"),
            imports: vec![expected],
        };
        let imports = [NativeImportV1 {
            import_key: 7,
            canonical_key: c"rinzu.test@0.2|rinzu.test.run|changed".as_ptr(),
            signature: c"i64()".as_ptr(),
            address: test_native as *const () as usize,
        }];
        let descriptor = NativePackageDescriptorV1 {
            struct_size: size_of::<NativePackageDescriptorV1>() as u32,
            abi_version: ABI_VERSION,
            package_name: c"rinzu.test".as_ptr(),
            metadata_digest: std::ptr::null(),
            metadata_digest_size: 0,
            reserved: 0,
            import_count: imports.len(),
            imports: imports.as_ptr(),
        };
        let mut loader = empty_loader();
        let error = loader
            .validate_descriptor(&artifact, &descriptor)
            .expect_err("metadata drift must be rejected");
        assert!(error.to_string().contains("metadata mismatch"), "{error:#}");
        assert!(loader.imports.is_empty());
        assert!(loader.exports.is_empty());
    }
}
