pub fn source(place: &str) -> String {
    let body = match place {
        "supplant" => {
            "let blocked=await termination.runForever(callback,\"interrupt 日本語\",b\"\\x00\\xAA\\x00\\x7F\");"
        }
        "main" => {
            "let blocked=termination.forever(\"interrupt 日本語\",b\"\\x00\\xAA\\x00\\x7F\");"
        }
        "worker" => "let worker=spawn work();let blocked=await worker;",
        _ => panic!("unknown interrupt fixture"),
    };
    format!(
        "import termination;\nfn callback()->Unit fallback () {{termination.mark(1);}}\nfn work()->CInt {{let retained=[\"Worker roots\",\"no physical return\"];try {{return termination.forever(\"interrupt 日本語\",b\"\\x00\\xAA\\x00\\x7F\");}}finally {{termination.mark(2);}}}}\nasync fn main(){{try {{let retained=[\"Main roots\",\"no fabricated cleanup\"];try {{{body}termination.mark(5);}}finally {{termination.mark(4);}}}}catch errors{{panic(\"failed\");}}}}"
    )
}

pub fn inherit(command: &mut std::process::Command, policy: usize) {
    use std::os::unix::process::CommandExt;
    // SAFETY: only native signal/mask/process-group syscalls execute between
    // fork and exec. The test process's own disposition and mask are untouched.
    unsafe {
        command.pre_exec(move || {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if policy != 0 && libc::signal(libc::SIGINT, libc::SIG_IGN) == libc::SIG_ERR {
                return Err(std::io::Error::last_os_error());
            }
            if policy == 2 {
                let mut set = std::mem::zeroed();
                if libc::sigemptyset(&mut set) != 0
                    || libc::sigaddset(&mut set, libc::SIGINT) != 0
                    || libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) != 0
                {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
}

pub fn terminate_once(
    command: &mut std::process::Command,
    policy: usize,
    callback: bool,
) -> std::process::Output {
    terminate_once_with_startup_limit(command, policy, callback, 45)
}

pub fn terminate_once_with_startup_limit(
    command: &mut std::process::Command,
    policy: usize,
    callback: bool,
    startup_seconds: u64,
) -> std::process::Output {
    use std::io::Read;
    use std::os::unix::process::ExitStatusExt;
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    inherit(command, policy);
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stream = child.stderr.take().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        let mut chunk = [0; 512];
        let mut ready = false;
        loop {
            let count = stream.read(&mut chunk).unwrap();
            if count == 0 {
                break;
            }
            output.extend_from_slice(&chunk[..count]);
            if !ready && String::from_utf8_lossy(&output).contains("TERMINATION_MARK=10\n") {
                ready = true;
                let _ = sender.send(());
            }
        }
        output
    });
    if receiver
        .recv_timeout(Duration::from_secs(startup_seconds))
        .is_err()
    {
        child.kill().unwrap();
        let status = child.wait().unwrap();
        let stderr = reader.join().unwrap();
        panic!(
            "native block not reached: {status:?} {}",
            String::from_utf8_lossy(&stderr)
        );
    }
    // One foreground-process-group interrupt, with no forwarding or retry.
    // SAFETY: this child's private process group was installed before exec.
    assert_eq!(unsafe { libc::kill(-(child.id() as i32), libc::SIGINT) }, 0);
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > Duration::from_secs(3) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("one SIGINT did not terminate the process");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let stderr = String::from_utf8(reader.join().unwrap()).unwrap();
    assert_eq!(status.signal(), Some(libc::SIGINT), "{stderr}");
    assert_eq!(
        stderr.contains("TERMINATION_MARK=1\n"),
        callback,
        "{stderr}"
    );
    for id in [2, 3, 4, 5, 9] {
        assert!(
            !stderr.contains(&format!("TERMINATION_MARK={id}\n")),
            "{stderr}"
        );
    }
    assert!(
        !stderr.contains("ERROR: AddressSanitizer") && !stderr.contains("runtime error:"),
        "{stderr}"
    );
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    assert!(
        !stdout.contains("RINZU_NATIVE_CALLS=") && !stdout.contains("\"resources\":\"baseline\""),
        "{stdout}"
    );
    std::process::Output {
        status,
        stdout: stdout.into_bytes(),
        stderr: stderr.into_bytes(),
    }
}
