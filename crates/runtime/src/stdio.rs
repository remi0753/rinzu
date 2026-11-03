//! Length-aware synchronous standard output; the caller owns the native transition.

/// The same physical writer is compiled into standalone and compiled JIT images.
pub const C_SOURCE: &str = include_str!("stdio.c");

/// A run's output destination. Verification captures bytes without replacing
/// process-wide fd 1, so concurrent runs and native library output stay intact.
#[derive(Clone, Debug, Default)]
pub struct Output {
    capture: Option<std::sync::Arc<std::sync::Mutex<Vec<u8>>>>,
}
impl Output {
    pub fn new(capture: bool) -> Self {
        Self {
            capture: capture.then(|| std::sync::Arc::new(std::sync::Mutex::new(Vec::new()))),
        }
    }
    pub fn write(&self, bytes: &[u8], newline: bool) -> std::io::Result<()> {
        if let Some(capture) = &self.capture {
            let mut buffer = capture
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let count = bytes
                .len()
                .checked_add(usize::from(newline))
                .unwrap_or_else(|| crate::allocation::fatal());
            buffer
                .try_reserve(count)
                .unwrap_or_else(|_| crate::allocation::fatal());
            buffer.extend_from_slice(bytes);
            if newline {
                buffer.push(b'\n');
            }
            Ok(())
        } else {
            write_stdout(bytes, newline)
        }
    }
    /// Drain only after every physical writer in this run has returned.
    pub fn take(&self) -> Vec<u8> {
        self.capture.as_ref().map_or_else(Vec::new, |capture| {
            std::mem::take(
                &mut *capture
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            )
        })
    }
}

#[cfg(unix)]
unsafe extern "C" {
    fn rinzu_stdio_write(fd: i32, bytes: *const u8, length: usize, newline: i32) -> i32;
    fn rinzu_stdio_panic(bytes: *const u8, length: usize);
}

/// Best-effort, length-aware panic diagnostic. Failure to write must not replace
/// the original panic or skip its managed cleanup. The caller enters native-safe
/// state first and supplies an owned native snapshot of the message.
pub fn write_panic(message: &str) {
    #[cfg(unix)]
    // SAFETY: the native String remains alive through physical return.
    unsafe {
        rinzu_stdio_panic(message.as_ptr(), message.len());
    }
    #[cfg(not(unix))]
    let _ = message;
}

/// Write one complete record on the calling thread, including its optional LF.
/// The runtime must publish managed roots and enter native-safe state first.
pub fn write_stdout(bytes: &[u8], newline: bool) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        // SAFETY: the owned native slice stays alive through physical return.
        let status =
            unsafe { rinzu_stdio_write(1, bytes.as_ptr(), bytes.len(), i32::from(newline)) };
        if status == 0 {
            Ok(())
        } else {
            Err(std::io::Error::from_raw_os_error(status))
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (bytes, newline);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "stdout target is not supported",
        ))
    }
}
