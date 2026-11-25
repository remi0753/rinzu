//! Keep publication checks tied to one executable even when another Cargo
//! invocation replaces the workspace binary during a long integration suite.
// Some integration targets only exercise this helper on selected platforms.
#![allow(dead_code)]

static CLI: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

#[cfg(unix)]
unsafe extern "C" {
    fn atexit(callback: extern "C" fn()) -> std::ffi::c_int;
}

#[cfg(unix)]
extern "C" fn cleanup() {
    if let Some(directory) = CLI.get().and_then(|path| path.parent()) {
        let _ = std::fs::remove_dir_all(directory);
    }
}

pub fn path() -> &'static std::path::Path {
    CLI.get_or_init(|| {
        let root =
            std::env::temp_dir().join(format!("rinzu-integration-cli-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("rinzu");
        // A cross-built test binary and its CLI can move together to the target host.
        let mut cli = std::path::PathBuf::from(env!("CARGO_BIN_EXE_rinzu"));
        if !cli.is_file() {
            let test = std::env::current_exe().unwrap();
            cli = test.parent().unwrap().parent().unwrap().join("rinzu");
        }
        std::fs::copy(cli, &path).unwrap();
        #[cfg(unix)]
        // SAFETY: the callback reads a process-lifetime OnceLock and only
        // removes this process's private copy after its tests have exited.
        if unsafe { atexit(cleanup) } != 0 {
            let _ = std::fs::remove_dir_all(root);
            panic!("could not register frozen CLI cleanup");
        }
        path
    })
}
