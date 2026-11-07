//! A process exit must not depend on diagnostic allocation, locks or output.

/// One best-effort static diagnostic to a nonblocking pipe/socket/terminal,
/// followed by immediate termination. Never retry or run cleanup/user code.
#[cold]
pub fn terminate(code: i32) -> ! {
    let resource = code == 71;
    let message: &[u8] = if resource {
        b"rinzu: required runtime resource unavailable (71)\n"
    } else {
        b"rinzu: native data contract violation (86)\n"
    };
    // SAFETY: these bounded libc operations use stack/static storage. Never
    // write ordinary files, where O_NONBLOCK does not prevent a blocked write.
    // Ignore SIGPIPE process-wide until exit. macOS can deliver a closed
    // pipe's signal to another thread.
    unsafe {
        let mut status: libc::stat = std::mem::zeroed();
        if libc::fstat(libc::STDERR_FILENO, &mut status) == 0 {
            let kind = status.st_mode & libc::S_IFMT;
            if kind == libc::S_IFIFO
                || kind == libc::S_IFSOCK
                || (kind == libc::S_IFCHR && libc::isatty(libc::STDERR_FILENO) == 1)
            {
                let flags = libc::fcntl(libc::STDERR_FILENO, libc::F_GETFL);
                if flags >= 0
                    && libc::fcntl(libc::STDERR_FILENO, libc::F_SETFL, flags | libc::O_NONBLOCK)
                        == 0
                {
                    let mut ignored: libc::sigaction = std::mem::zeroed();
                    ignored.sa_sigaction = libc::SIG_IGN;
                    libc::sigemptyset(&mut ignored.sa_mask);
                    if libc::sigaction(libc::SIGPIPE, &ignored, std::ptr::null_mut()) == 0 {
                        libc::write(libc::STDERR_FILENO, message.as_ptr().cast(), message.len());
                    }
                }
            }
        }
        libc::_exit(if resource { 71 } else { 86 })
    }
}

pub fn invariant() -> ! {
    terminate(86)
}
pub fn resource() -> ! {
    terminate(71)
}
