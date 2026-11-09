use std::process::{Command, Output, Stdio};

pub fn bounded(command: &mut Command) -> Output {
    bounded_for(command, 60)
}
pub fn bounded_tool(command: &mut Command) -> Output {
    bounded_for(command, 300)
}
fn bounded_for(command: &mut Command, seconds: u64) -> Output {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let read = |mut pipe: Box<dyn std::io::Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes).unwrap();
            bytes
        })
    };
    let out = read(Box::new(stdout));
    let err = read(Box::new(stderr));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            #[cfg(unix)]
            {
                let group = libc::pid_t::try_from(child.id()).unwrap();
                // SAFETY: spawn placed this command and its descendants in their own group.
                if unsafe { libc::killpg(group, libc::SIGKILL) } != 0 {
                    let error = std::io::Error::last_os_error();
                    assert_eq!(error.raw_os_error(), Some(libc::ESRCH), "{error}");
                }
            }
            #[cfg(not(unix))]
            child.kill().unwrap();
            break child.wait().unwrap();
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    Output {
        status,
        stdout: out.join().unwrap(),
        stderr: err.join().unwrap(),
    }
}
