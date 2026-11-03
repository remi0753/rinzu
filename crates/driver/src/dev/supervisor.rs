use std::process::{Child, Command, ExitStatus};
use std::sync::atomic::{AtomicU32, Ordering};

static INTERRUPTS: AtomicU32 = AtomicU32::new(0);
extern "C" fn handler(_: i32) {
    INTERRUPTS.fetch_add(1, Ordering::SeqCst);
}
pub(super) fn interrupts() -> u32 {
    INTERRUPTS.load(Ordering::SeqCst)
}

pub(super) struct Signals {
    old: Vec<(i32, libc::sigaction)>,
}
impl Signals {
    pub fn install() -> Result<Self, String> {
        INTERRUPTS.store(0, Ordering::SeqCst);
        let mut guard = Self { old: Vec::new() };
        for number in [libc::SIGINT, libc::SIGTERM] {
            // SAFETY: POD actions and masks initialized before sigaction. The
            // handler changes only a lock-free atomic; restoration uses old POD.
            let installed = unsafe {
                let mut action: libc::sigaction = std::mem::zeroed();
                let mut old = std::mem::zeroed();
                action.sa_sigaction = handler as *const () as usize;
                if libc::sigemptyset(&mut action.sa_mask) != 0
                    || libc::sigaction(number, &action, &mut old) != 0
                {
                    false
                } else {
                    guard.old.push((number, old));
                    true
                }
            };
            if !installed {
                return Err(format!(
                    "install dev signal flag: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
        Ok(guard)
    }
}
impl Drop for Signals {
    fn drop(&mut self) {
        for (number, old) in &self.old {
            // SAFETY: restore the action returned by install for this signal.
            unsafe {
                libc::sigaction(*number, old, std::ptr::null_mut());
            }
        }
    }
}

pub(super) struct OwnedChild {
    child: Child,
    status: Option<ExitStatus>,
    group: bool,
    group_owned: bool,
}
impl OwnedChild {
    pub fn spawn(command: &mut Command, group: bool) -> Result<Self, String> {
        if group {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let child = command.spawn().map_err(|e| format!("start dev run: {e}"))?;
        Ok(Self {
            child,
            status: None,
            group,
            group_owned: group,
        })
    }
    pub fn pid(&self) -> u32 {
        self.child.id()
    }
    pub fn poll(&mut self) -> Result<Option<ExitStatus>, String> {
        if self.status.is_none() {
            self.status = self
                .child
                .try_wait()
                .map_err(|e| format!("wait dev run: {e}"))?;
        }
        Ok(self.status)
    }
    pub fn group_alive(&mut self) -> Result<bool, String> {
        if !self.group_owned {
            return Ok(false);
        }
        // SAFETY: signal 0 inspects only the dedicated group created above.
        if unsafe { libc::kill(-(self.pid() as i32), 0) } == 0 {
            return Ok(true);
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            self.group_owned = false;
            Ok(false)
        } else {
            Err(format!("inspect owned dev group: {error}"))
        }
    }
    pub fn notify(&mut self) -> Result<(), String> {
        if self.poll()?.is_some() {
            return Ok(());
        }
        // SAFETY: this unreaped Child reserves its PID; notify it once from the loop.
        if unsafe { libc::kill(self.pid() as i32, libc::SIGTERM) } == 0 {
            Ok(())
        } else {
            Err(format!(
                "notify dev run: {}",
                std::io::Error::last_os_error()
            ))
        }
    }
    pub fn force(&mut self) -> Result<(), String> {
        self.poll()?;
        if self.group && self.group_alive()? {
            // SAFETY: this group is still owned; it was never reused after disappearance.
            if unsafe { libc::kill(-(self.pid() as i32), libc::SIGKILL) } != 0
                && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
            {
                return Err("force owned dev group failed".into());
            }
        } else if self.status.is_none() {
            self.child.kill().map_err(|e| e.to_string())?;
        }
        if self.status.is_none() {
            self.status = Some(
                self.child
                    .wait()
                    .map_err(|e| format!("reap dev run: {e}"))?,
            );
        }
        Ok(())
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.status.is_none() || self.group_owned {
            eprintln!("dev emergency owner cleanup; settlement unconfirmed");
            let _ = self.force();
        }
    }
}
