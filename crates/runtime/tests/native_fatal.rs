#![cfg(unix)]
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};
thread_local! { static ARMED: Cell<bool> = const { Cell::new(false) }; }
struct NoDiagnosticAllocation;
fn allocation_check() {
    if ARMED.try_with(Cell::get).unwrap_or(false) {
        // SAFETY: a distinct exit proves that the fatal path allocated.
        unsafe { libc::_exit(92) }
    }
}
// SAFETY: System's allocation contract is preserved until the child arms its
// final probe; an unexpected diagnostic allocation then exits with a distinct code.
unsafe impl GlobalAlloc for NoDiagnosticAllocation {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        allocation_check();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        allocation_check();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        allocation_check();
        unsafe { System.realloc(pointer, layout, size) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: NoDiagnosticAllocation = NoDiagnosticAllocation;
struct Cleanup;
impl Drop for Cleanup {
    fn drop(&mut self) {
        unsafe {
            libc::write(1, b"CLEANUP_RAN\n".as_ptr().cast(), 12);
        }
    }
}
fn configure_stderr(mode: &str) {
    if mode == "lock" {
        let ready = Arc::new(Barrier::new(2));
        let worker = ready.clone();
        std::thread::spawn(move || {
            let _locked = std::io::stderr().lock();
            worker.wait();
            loop {
                std::thread::park();
            }
        });
        ready.wait();
    } else if mode == "closed" {
        unsafe {
            libc::close(2);
        }
    } else if mode == "full" || mode == "broken" {
        unsafe {
            let mut pipes = [0; 2];
            assert_eq!(libc::pipe(pipes.as_mut_ptr()), 0);
            if mode == "full" {
                let flags = libc::fcntl(pipes[1], libc::F_GETFL);
                assert_eq!(
                    libc::fcntl(pipes[1], libc::F_SETFL, flags | libc::O_NONBLOCK),
                    0
                );
                let bytes = [0u8; 4096];
                while libc::write(pipes[1], bytes.as_ptr().cast(), bytes.len()) > 0 {}
                assert_eq!(libc::fcntl(pipes[1], libc::F_SETFL, flags), 0);
            } else {
                libc::close(pipes[0]);
                libc::signal(libc::SIGPIPE, libc::SIG_DFL);
            }
            assert_eq!(libc::dup2(pipes[1], 2), 2);
        }
    }
}
#[test]
fn fatal_child() {
    let Ok(mode) = std::env::var("RINZU_TEST_NATIVE_FATAL") else {
        return;
    };
    let (code, output) = mode.split_once(':').unwrap();
    let code = code.parse::<i32>().unwrap();
    configure_stderr(output);
    let _cleanup = Cleanup;
    ARMED.with(|armed| armed.set(true));
    rinzu_runtime::native_fatal::terminate(code);
}
fn bounded(command: &mut Command) -> Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            panic!("fatal waited for output/lock/other thread");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    child.wait_with_output().unwrap()
}
fn check(output: Output, code: i32, mode: &str) {
    assert_eq!(output.status.code(), Some(code), "{mode}: {output:?}");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("CLEANUP_RAN"));
    if mode == "open" || mode == "lock" {
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(if code == 71 {
                "resource unavailable (71)"
            } else {
                "contract violation (86)"
            }),
            "{output:?}"
        );
    } else {
        assert!(output.stderr.is_empty(), "{output:?}");
    }
}
#[test]
fn rust_fatal_never_allocates_waits_unwinds_or_loses_the_exit_to_sigpipe() {
    for code in [71, 86] {
        for mode in ["open", "full", "closed", "broken", "lock"] {
            check(
                bounded(
                    Command::new(std::env::current_exe().unwrap())
                        .args(["--exact", "fatal_child", "--nocapture"])
                        .env("RINZU_TEST_NATIVE_FATAL", format!("{code}:{mode}")),
                ),
                code,
                mode,
            );
        }
    }
}
#[test]
fn c_fatal_matches_rust_with_blocked_stdio_and_output_and_sanitizers() {
    let root = std::env::temp_dir().join(format!("rinzu-native-fatal-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for sanitized in [false, true] {
        let binary = root.join(if sanitized { "sanitized" } else { "normal" });
        let mut clang = Command::new("clang");
        clang
            .args([
                "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread", "-I",
            ])
            .arg(manifest.join("../../include"))
            .arg(manifest.join("tests/fixtures/native_fatal.c"));
        if sanitized {
            clang.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
        }
        let compiled = clang.arg("-o").arg(&binary).output().unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        for code in [71, 86] {
            for mode in ["open", "full", "closed", "broken", "lock"] {
                check(
                    bounded(
                        Command::new(&binary)
                            .arg(code.to_string())
                            .arg(mode)
                            .env("ASAN_OPTIONS", "detect_leaks=0")
                            .env("UBSAN_OPTIONS", "halt_on_error=1"),
                    ),
                    code,
                    mode,
                );
            }
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
