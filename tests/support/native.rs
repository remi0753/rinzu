//! Native compiler inputs for the shared C fixtures.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

pub fn clang() -> Command {
    if cfg!(target_os = "linux") {
        let mut command =
            Command::new(std::env::var_os("RINZU_CLANG").unwrap_or_else(|| "clang".into()));
        if cfg!(all(target_arch = "x86_64", target_env = "gnu")) {
            command.args(["-target", "x86_64-unknown-linux-gnu"]);
        }
        command.arg("-D_GNU_SOURCE");
        if let Some(root) = std::env::var_os("RINZU_SYSROOT") {
            command.arg("--sysroot").arg(root);
        }
        command
    } else {
        Command::new("clang")
    }
}

pub fn asan_options_after_teardown() -> &'static str {
    if cfg!(target_os = "linux") {
        "detect_leaks=1:abort_on_error=1"
    } else {
        "detect_leaks=0:abort_on_error=1"
    }
}

pub fn shared_library(stem: &str, directory: &Path) -> (Command, PathBuf) {
    let path = directory.join(format!("{stem}{}", std::env::consts::DLL_SUFFIX));
    let mut command = clang();
    if cfg!(target_os = "macos") {
        command.arg("-dynamiclib");
    } else {
        command.args(["-shared", "-fPIC", "-Wl,--build-id,-z,noexecstack"]);
    }
    command.arg("-o").arg(&path);
    (command, path)
}

pub fn callback_template(runtime: &Path) -> PathBuf {
    runtime.join(if cfg!(target_os = "linux") {
        "src/arch/x86_64/callback_template.S"
    } else {
        "src/callback_template.S"
    })
}
