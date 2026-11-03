//! A runtime image stays loaded after use when it owns process-lifetime closed
//! callback bridges and template metadata. Managed/JIT code has a shorter pin.
use anyhow::{Result, ensure};
use std::ffi::{CString, c_void};
use std::marker::PhantomData;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::rc::Rc;

pub struct CompiledRuntimeLibrary {
    handle: *mut c_void,
    process_image: bool,
    owner_only: PhantomData<Rc<()>>,
}
impl CompiledRuntimeLibrary {
    pub fn open(path: &Path) -> Result<Self> {
        let path = CString::new(path.as_os_str().as_bytes())?;
        let handle = crate::native_library::open(&path);
        ensure!(
            !handle.is_null(),
            "could not load compiled async runtime image"
        );
        Ok(Self {
            handle,
            process_image: false,
            owner_only: PhantomData,
        })
    }
    pub fn symbol(&self, name: &str) -> Result<usize> {
        let name = CString::new(name)?;
        // SAFETY: this instance owns a live library handle through lookup.
        let address = unsafe { crate::native_library::symbol(self.handle, &name) };
        ensure!(
            !address.is_null(),
            "compiled runtime symbol is missing: {name:?}"
        );
        Ok(address as usize)
    }
    /// Retain the immutable image for closed bridge/template process lifetime.
    /// This retains no managed context or JIT address after call settlement.
    pub fn retain_process_image(&mut self) {
        self.process_image = true;
    }
}
impl Drop for CompiledRuntimeLibrary {
    fn drop(&mut self) {
        if !self.process_image {
            // SAFETY: exactly one owned successful dlopen reference.
            unsafe { crate::native_library::close(self.handle) };
        }
    }
}
