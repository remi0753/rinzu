//! Initialize private typed slots only from their checked explicit providers.
use anyhow::{Context, Result, ensure};
use rinzu_bindgen::native_binding::{DIGEST_KEY, INITIALIZER, NativeBindingPlan};
use rinzu_bindgen::source_package::{SourceDescriptor, sha256};
use std::ffi::{CStr, CString, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

pub(crate) struct ProviderHandle(*mut c_void);
impl Drop for ProviderHandle {
    fn drop(&mut self) {
        // SAFETY: each temporary handle owns exactly one successful dlopen reference.
        unsafe { crate::native_library::close(self.0) };
    }
}

unsafe extern "C" {
    fn _dyld_image_count() -> u32;
    fn _dyld_get_image_name(index: u32) -> *const libc::c_char;
    fn _dyld_get_image_header(index: u32) -> *const u8;
}

fn canonical(path: &Path) -> Result<PathBuf> {
    match rinzu_runtime::allocation::checked_io(path.canonicalize()) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(path.into()),
        Err(error) => Err(error.into()),
    }
}

fn same_image(expected: &Path, actual: &Path) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let expected = canonical(expected)?;
    let actual = canonical(actual)?;
    if expected == actual {
        return Ok(true);
    }
    let a = rinzu_runtime::allocation::checked_io(std::fs::metadata(&expected));
    let b = rinzu_runtime::allocation::checked_io(std::fs::metadata(&actual));
    Ok(matches!((a,b), (Ok(a), Ok(b)) if a.dev() == b.dev() && a.ino() == b.ino()))
}

fn provider_path(facade: &Path, path: &str, bundled: bool) -> Result<PathBuf> {
    if !bundled {
        return canonical(Path::new(path));
    }
    let root = facade.parent().context("native facade has no directory")?;
    let path = canonical(&root.join(path))?;
    ensure!(
        path.parent() == Some(root),
        "bundled provider escaped facade directory"
    );
    Ok(path)
}

fn check_bytes(path: &Path, expected: Option<&str>) -> Result<()> {
    if let Some(expected) = expected {
        let bytes = rinzu_runtime::allocation::checked_io(std::fs::read(path))?;
        ensure!(
            sha256(&bytes) == expected,
            "native provider byte digest mismatch: {}",
            path.display()
        );
    }
    Ok(())
}

/// Read only loader-owned Mach-O metadata. The header and commands are mapped
/// by dyld; no pointer supplied by user code is accepted here.
unsafe fn image_uuid(header: *const u8) -> Result<String> {
    ensure!(!header.is_null(), "native image header is absent");
    // SAFETY: caller supplies a dyld-owned mach_header_64.
    let word = |offset: usize| unsafe { header.add(offset).cast::<u32>().read_unaligned() };
    ensure!(
        word(0) == 0xfeed_facf && word(4) == 0x0100_000c,
        "native loaded image target mismatch"
    );
    let count = word(16) as usize;
    let size = word(20) as usize;
    ensure!(
        count <= 16_384 && size <= 16 * 1024 * 1024,
        "invalid native image command bounds"
    );
    let mut offset = 32;
    let end = 32 + size;
    for _ in 0..count {
        ensure!(offset + 8 <= end, "truncated native image command");
        let command = word(offset);
        let length = word(offset + 4) as usize;
        ensure!(
            length >= 8 && length <= end - offset,
            "invalid native image command length"
        );
        if command == 0x1b {
            ensure!(length == 24, "invalid native image UUID command");
            // SAFETY: LC_UUID's sixteen bytes lie inside the checked mapped command.
            let uuid = unsafe { std::slice::from_raw_parts(header.add(offset + 8), 16) };
            return Ok(uuid.iter().map(|byte| format!("{byte:02x}")).collect());
        }
        offset += length;
    }
    anyhow::bail!("loaded native image UUID is absent")
}

fn check_loaded_image(path: &Path, expected_uuid: &str) -> Result<()> {
    // SAFETY: dyld supplies readable process-lifetime image names and headers.
    unsafe {
        for index in 0.._dyld_image_count() {
            let name = _dyld_get_image_name(index);
            if !name.is_null() && same_image(path, Path::new(CStr::from_ptr(name).to_str()?))? {
                ensure!(
                    image_uuid(_dyld_get_image_header(index))? == expected_uuid,
                    "loaded native provider UUID mismatch: {}",
                    path.display()
                );
                return Ok(());
            }
        }
    }
    anyhow::bail!(
        "explicit native provider was not loaded: {}",
        path.display()
    )
}

fn check_address(path: &Path, uuid: &str, address: *mut c_void) -> Result<()> {
    ensure!(!address.is_null(), "native provider symbol address is null");
    // SAFETY: Dl_info is POD; dladdr reads a resolved code pointer and writes this POD.
    let mut info: libc::Dl_info = unsafe { core::mem::zeroed() };
    // SAFETY: address is resolved from a live handle and info is writable POD storage.
    let found = unsafe { libc::dladdr(address, &mut info) };
    ensure!(
        found != 0 && !info.dli_fname.is_null() && !info.dli_fbase.is_null(),
        "native provider address has no image"
    );
    // SAFETY: successful dladdr returns a mapped Mach-O base and a static C name.
    unsafe {
        ensure!(
            same_image(path, Path::new(CStr::from_ptr(info.dli_fname).to_str()?))?,
            "native symbol belongs to another provider: {}",
            path.display()
        );
        ensure!(
            image_uuid(info.dli_fbase.cast())? == uuid,
            "native symbol provider UUID mismatch"
        );
    }
    Ok(())
}

pub(crate) fn initialize(
    facade: &Path,
    facade_handle: *mut c_void,
    descriptor: &SourceDescriptor,
) -> Result<Vec<ProviderHandle>> {
    let inputs = &descriptor.interface.package.build_inputs;
    let Some(plan) = NativeBindingPlan::decode(inputs).map_err(anyhow::Error::msg)? else {
        return Ok(Vec::new());
    };
    plan.validate_functions(&descriptor.interface.functions)
        .map_err(anyhow::Error::msg)?;
    // Loading a root can load its transitive images before their own turn.
    for image in &plan.images {
        let path = provider_path(facade, &image.path, image.bundled)?;
        check_bytes(&path, image.sha256.as_deref())?;
    }
    let mut handles = Vec::new();
    for image in &plan.images {
        let path = provider_path(facade, &image.path, image.bundled)?;
        check_bytes(&path, image.sha256.as_deref())?;
        let path_string = CString::new(path.as_os_str().as_bytes())?;
        let handle = crate::native_library::open_first(&path_string);
        ensure!(
            !handle.is_null(),
            "cannot load explicit native provider: {}",
            path.display()
        );
        let handle = ProviderHandle(handle);
        check_loaded_image(&path, &image.uuid)?;
        check_bytes(&path, image.sha256.as_deref())?;
        handles.push(handle);
    }
    let mut addresses = Vec::new();
    for slot in &plan.slots {
        let owner = provider_path(facade, &slot.owner_path, plan.owner_bundled(slot))?;
        check_bytes(&owner, slot.owner_sha256.as_deref())?;
        let name = CString::new(slot.resolver.as_deref().unwrap_or(&slot.symbol))?;
        // SAFETY: this retained handle searches only the explicit root and re-exports.
        let mut address = unsafe { crate::native_library::symbol(handles[slot.image].0, &name) };
        if slot.resolver.is_some() {
            check_address(&owner, &slot.owner_uuid, address)?;
            // SAFETY: the digest-verified generated provider owns this zero-argument
            // uintptr_t getter; it returns an address without calling user native code.
            let get_address: unsafe extern "C" fn() -> usize =
                unsafe { core::mem::transmute(address) };
            // SAFETY: the verified generated getter follows the private ABI above.
            address = unsafe { get_address() } as *mut c_void;
        }
        check_address(&owner, &slot.owner_uuid, address).with_context(|| {
            format!(
                "native binding {} / {} / {}",
                slot.declaration, slot.symbol, slot.definition
            )
        })?;
        addresses.push(address as usize);
    }
    let name = CString::new(INITIALIZER)?;
    // SAFETY: facade_handle is owned by the caller for this transaction.
    let address = unsafe { crate::native_library::symbol(facade_handle, &name) };
    ensure!(
        !address.is_null(),
        "private native binding initializer is missing"
    );
    // An ambient/dependency initializer must never receive this plan.
    // SAFETY: Dl_info is C POD with a valid all-zero representation.
    let mut info: libc::Dl_info = unsafe { core::mem::zeroed() };
    // SAFETY: address is resolved from a live handle and info is writable POD storage.
    let found = unsafe { libc::dladdr(address, &mut info) };
    ensure!(
        found != 0 && !info.dli_fname.is_null(),
        "native initializer image is unknown"
    );
    // SAFETY: successful dladdr supplies a readable NUL-terminated image name.
    let initializer_image = unsafe { CStr::from_ptr(info.dli_fname) }.to_str()?;
    ensure!(
        same_image(facade, Path::new(initializer_image))?,
        "native initializer belongs to another facade"
    );
    let digest = CString::new(
        inputs
            .get(DIGEST_KEY)
            .context("native binding plan digest is absent")?
            .as_str(),
    )?;
    // SAFETY: byte-verified generated facade defines this private initializer ABI.
    let initialize: unsafe extern "C" fn(*const libc::c_char, *const usize, usize) -> i32 =
        unsafe { core::mem::transmute(address) };
    // SAFETY: the verified facade owns this ABI; both input arrays remain readable.
    let accepted = unsafe { initialize(digest.as_ptr(), addresses.as_ptr(), addresses.len()) };
    ensure!(
        accepted == 1,
        "native plan injection rejected (digest/count/rebinding)"
    );
    Ok(handles)
}
