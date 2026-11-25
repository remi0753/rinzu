use std::io;
use std::process::{Command, ExitStatus};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(unix)]
use std::process::{Child, ChildStdin};
#[cfg(unix)]
use std::sync::atomic::Ordering;

pub(super) struct WorkerExit {
    pub status: ExitStatus,
    pub forced: Option<&'static str>,
    pub stdout: (Vec<u8>, u64),
    pub stderr: (Vec<u8>, u64),
}

#[cfg(unix)]
pub(super) fn wait(
    command: &mut Command,
    input: &[u8],
    started: Instant,
    timeout: Duration,
    limit: usize,
) -> io::Result<WorkerExit> {
    let mut child = Running {
        process: command.process_group(0).spawn()?,
        finished: false,
    };
    let mut stdin = child.process.stdin.take();
    set_nonblocking(stdin.as_ref().expect("piped stdin").as_fd())?;
    let mut stdout = Capture::new(child.process.stdout.take().expect("piped stdout"), limit)?;
    let mut stderr = Capture::new(child.process.stderr.take().expect("piped stderr"), limit)?;
    let mut request = input;

    let (status, forced) = loop {
        let forced = if super::INTERRUPTED.load(Ordering::SeqCst) != 0 {
            Some("interrupted")
        } else if started.elapsed() >= timeout {
            Some("timeout")
        } else {
            None
        };
        if forced.is_some() || child.has_exited()? {
            child.kill_group();
            break (child.process.wait()?, forced);
        }

        let sent = write_request(&mut stdin, &mut request)?;
        let out = stdout.read_once(usize::MAX)?;
        let err = stderr.read_once(usize::MAX)?;
        if sent + out + err == 0 {
            std::thread::sleep(
                Duration::from_millis(10).min(timeout.saturating_sub(started.elapsed())),
            );
        }
    };
    child.finished = true;
    drop(stdin);
    // A descendant may still hold a pipe open. Collect the queued output without
    // waiting for EOF or for any future writes.
    stdout.drain_queued()?;
    stderr.drain_queued()?;
    Ok(WorkerExit {
        status,
        forced,
        stdout: (stdout.bytes, stdout.total),
        stderr: (stderr.bytes, stderr.total),
    })
}

#[cfg(not(unix))]
pub(super) fn wait(
    _command: &mut Command,
    _input: &[u8],
    _started: Instant,
    _timeout: Duration,
    _limit: usize,
) -> io::Result<WorkerExit> {
    Err(io::Error::other(
        "isolated package tests require a Unix process supervisor",
    ))
}

#[cfg(unix)]
struct Running {
    process: Child,
    finished: bool,
}

#[cfg(unix)]
impl Running {
    fn has_exited(&self) -> io::Result<bool> {
        // SAFETY: siginfo_t is a C POD. Zero it for the no-event case.
        let mut event: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // Keep the worker's PID reserved until the process group has been signaled.
        // SAFETY: event is a live output buffer and the PID identifies this child.
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                self.process.id() as libc::id_t,
                &mut event,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result == -1 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                return Ok(false);
            }
            return Err(error);
        }
        Ok(event.si_signo == libc::SIGCHLD)
    }

    fn kill_group(&mut self) {
        // SAFETY: the worker was started with a new process group whose ID is its PID.
        unsafe { libc::kill(-(self.process.id() as i32), libc::SIGKILL) };
        let _ = self.process.kill();
    }
}

#[cfg(unix)]
impl Drop for Running {
    fn drop(&mut self) {
        if !self.finished {
            self.kill_group();
            let _ = self.process.wait();
        }
    }
}

#[cfg(unix)]
fn set_nonblocking(fd: BorrowedFd<'_>) -> io::Result<()> {
    // SAFETY: fcntl reads and updates the flags of a live file descriptor.
    let updated = unsafe {
        let fd = fd.as_raw_fd();
        let flags = libc::fcntl(fd, libc::F_GETFL);
        flags >= 0 && libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) >= 0
    };
    if !updated {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
fn write_request(stdin: &mut Option<ChildStdin>, remaining: &mut &[u8]) -> io::Result<usize> {
    let Some(pipe) = stdin else {
        return Ok(0);
    };
    match pipe.write(remaining) {
        Ok(n) => {
            *remaining = &remaining[n..];
            if remaining.is_empty() {
                *stdin = None;
            }
            Ok(n)
        }
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {
            *stdin = None;
            Ok(0)
        }
        Err(error) if no_progress(&error) => Ok(0),
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn no_progress(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    )
}

#[cfg(unix)]
struct Capture<R> {
    pipe: R,
    limit: usize,
    bytes: Vec<u8>,
    total: u64,
    eof: bool,
}

#[cfg(unix)]
impl<R: AsFd + Read> Capture<R> {
    fn new(pipe: R, limit: usize) -> io::Result<Self> {
        set_nonblocking(pipe.as_fd())?;
        Ok(Self {
            pipe,
            limit,
            bytes: Vec::new(),
            total: 0,
            eof: false,
        })
    }

    fn read_once(&mut self, maximum: usize) -> io::Result<usize> {
        if self.eof || maximum == 0 {
            return Ok(0);
        }
        let mut buffer = [0; 8192];
        let capacity = maximum.min(buffer.len());
        let n = match self.pipe.read(&mut buffer[..capacity]) {
            Ok(n) => n,
            Err(error) if no_progress(&error) => return Ok(0),
            Err(error) => return Err(error),
        };
        self.eof = n == 0;
        self.total = self
            .total
            .checked_add(n as u64)
            .ok_or_else(|| io::Error::other("test output byte count overflow"))?;
        let keep = n.min(self.limit - self.bytes.len());
        self.bytes.extend_from_slice(&buffer[..keep]);
        Ok(n)
    }

    fn drain_queued(&mut self) -> io::Result<()> {
        if self.eof {
            return Ok(());
        }
        let mut count: libc::c_int = 0;
        // SAFETY: FIONREAD writes the queued byte count to a live c_int.
        if unsafe { libc::ioctl(self.pipe.as_fd().as_raw_fd(), libc::FIONREAD, &mut count) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut remaining = count.max(0) as usize;
        while remaining != 0 {
            let n = self.read_once(remaining)?;
            if n == 0 {
                break;
            }
            remaining -= n;
        }
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn captures_queued_output_without_waiting_for_the_writer_to_close() {
        let (reader, mut writer) = io::pipe().unwrap();
        writer.write_all(b"worker output").unwrap();
        let mut capture = Capture::new(reader, 6).unwrap();
        capture.drain_queued().unwrap();
        assert_eq!(capture.bytes, b"worker");
        assert_eq!(capture.total, 13);
        assert!(!capture.eof);
        writer.write_all(b"later output").unwrap();
    }

    #[test]
    fn deadline_covers_a_worker_that_never_reads_its_request() {
        use std::process::Stdio;

        let mut command = Command::new("/bin/sleep");
        command
            .arg("5")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let stopped = wait(
            &mut command,
            &vec![0; super::super::OUTPUT_LIMIT],
            Instant::now(),
            Duration::from_millis(30),
            16,
        )
        .unwrap();
        assert_eq!(stopped.forced, Some("timeout"));
        assert!(!stopped.status.success());
    }
}
