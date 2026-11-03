//! Explicit library memory failures are fatal before loader diagnostics allocate.
use std::ffi::{CStr, c_void};

fn checked<T>(value: T, failed: bool) -> T {
    if failed && std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOMEM) {
        rinzu_runtime::allocation::fatal();
    }
    value
}

pub(crate) fn open(path: &CStr) -> *mut c_void {
    open_with_flags(path, libc::RTLD_NOW | libc::RTLD_LOCAL)
}

pub(crate) fn open_first(path: &CStr) -> *mut c_void {
    open_with_flags(path, libc::RTLD_NOW | libc::RTLD_LOCAL | libc::RTLD_FIRST)
}

fn open_with_flags(path: &CStr, flags: i32) -> *mut c_void {
    // SAFETY: errno is thread local; path is NUL terminated and valid for dlopen.
    unsafe {
        *libc::__error() = 0;
        #[cfg(test)]
        if tests::inject(tests::Operation::Open) {
            return checked(core::ptr::null_mut(), true);
        }
        let handle = libc::dlopen(path.as_ptr(), flags);
        checked(handle, handle.is_null())
    }
}

// SAFETY contract: handle must be a live dlopen handle owned by the caller.
pub(crate) unsafe fn symbol(handle: *mut c_void, name: &CStr) -> *mut c_void {
    // SAFETY: the caller retains handle; name is NUL terminated, errno thread local.
    unsafe {
        *libc::__error() = 0;
        #[cfg(test)]
        if tests::inject(tests::Operation::Symbol) {
            return checked(core::ptr::null_mut(), true);
        }
        let address = libc::dlsym(handle, name.as_ptr());
        checked(address, address.is_null())
    }
}

// SAFETY contract: close one owned live dlopen reference exactly once.
pub(crate) unsafe fn close(handle: *mut c_void) {
    // SAFETY: the caller owns handle; errno is thread local.
    unsafe {
        *libc::__error() = 0;
        #[cfg(test)]
        if tests::inject(tests::Operation::Close) {
            checked(-1, true);
            return;
        }
        let status = libc::dlclose(handle);
        checked(status, status != 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::process::Command;

    #[derive(Clone, Copy, Eq, PartialEq)]
    pub(super) enum Operation {
        Open,
        Symbol,
        Close,
    }
    thread_local! {
        static FAILURE: Cell<Option<(Operation, i32)>> = const { Cell::new(None) };
    }
    pub(super) fn inject(operation: Operation) -> bool {
        FAILURE.with(|failure| {
            if let Some((expected, code)) = failure.get()
                && expected == operation
            {
                failure.set(None);
                // SAFETY: __error returns this thread's errno location.
                unsafe { *libc::__error() = code };
                true
            } else {
                false
            }
        })
    }

    #[test]
    fn memory_failure_child() {
        let Ok(mode) = std::env::var("RINZU_TEST_LIBRARY_MEMORY_FAILURE") else {
            return;
        };
        let operation = match mode.as_str() {
            "open" => Operation::Open,
            "symbol" => Operation::Symbol,
            "close" => Operation::Close,
            _ => panic!("unknown library operation"),
        };
        let handle = if operation == Operation::Open {
            core::ptr::null_mut()
        } else {
            open(c"/usr/lib/libSystem.B.dylib")
        };
        FAILURE.with(|failure| failure.set(Some((operation, libc::ENOMEM))));
        match operation {
            Operation::Open => {
                open(c"/usr/lib/libSystem.B.dylib");
            }
            Operation::Symbol => {
                // SAFETY: retained successful libSystem handle.
                unsafe { symbol(handle, c"malloc") };
            }
            Operation::Close => {
                // SAFETY: retained successful libSystem handle owned here.
                unsafe { close(handle) };
            }
        }
        panic!("library ENOMEM returned to the loader");
    }

    #[test]
    fn explicit_library_enomem_exits_before_loader_diagnostics() {
        for mode in ["open", "symbol", "close"] {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "native_library::tests::memory_failure_child",
                    "--nocapture",
                ])
                .env("RINZU_TEST_LIBRARY_MEMORY_FAILURE", mode)
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(71), "{mode}: {output:?}");
            assert_eq!(
                output.stderr, b"rinzu: required runtime resource unavailable (71)\n",
                "{mode}: {output:?}"
            );
        }
    }

    #[test]
    fn missing_library_and_symbol_ignore_stale_memory_errno() {
        // SAFETY: set this thread's errno to prove each operation clears stale data.
        unsafe { *libc::__error() = libc::ENOMEM };
        assert!(open(c"/rinzu/absent-memory-audit-library.dylib").is_null());
        let handle = open(c"/usr/lib/libSystem.B.dylib");
        assert!(!handle.is_null());
        // SAFETY: handle is live and owned here; the missing symbol is NUL terminated.
        unsafe {
            *libc::__error() = libc::ENOMEM;
            assert!(symbol(handle, c"rinzu_absent_memory_audit_symbol").is_null());
            assert!(!symbol(handle, c"malloc").is_null());
            close(handle);
        }
        FAILURE.with(|failure| failure.set(Some((Operation::Open, libc::ENOENT))));
        assert!(open(c"/usr/lib/libSystem.B.dylib").is_null());
    }
}
