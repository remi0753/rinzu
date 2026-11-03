#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
// The shared module also provides source text for the permanent C SDK cases.
#[allow(dead_code)]
#[path = "fixtures/native_termination_sdk/interrupt.rs"]
mod interrupt;
#[allow(dead_code)]
#[path = "support/source_platform.rs"]
mod platform;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn create(root: &Path) -> PathBuf {
    std::fs::create_dir_all(root.join("app/src")).unwrap();
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for item in std::fs::read_dir(from).unwrap() {
            let item = item.unwrap();
            if ["build", ".rinzu"].contains(&item.file_name().to_str().unwrap_or("")) {
                continue;
            }
            if item.file_type().unwrap().is_dir() {
                copy(&item.path(), &to.join(item.file_name()));
            } else {
                std::fs::copy(item.path(), to.join(item.file_name())).unwrap();
            }
        }
    }
    let sdk = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sdk/macos");
    copy(&sdk, &root.join("macos"));
    copy(&sdk.join("../../include"), &root.join("include"));
    let declaration = root.join("macos/src/macos.rinzu");
    let mut source = std::fs::read_to_string(&declaration).unwrap();
    source.push_str("\nnative {fn interruptMark(marker:CInt) symbol \"rinzu_macos_interrupt_mark\";}\nexport interruptMark;\n");
    std::fs::write(declaration, source).unwrap();
    let header = root.join("macos/native/include/rinzu_macos.h");
    let mut source = std::fs::read_to_string(&header).unwrap();
    source.push_str("\nvoid rinzu_macos_interrupt_mark(int32_t);\n");
    std::fs::write(header, source).unwrap();
    let native = root.join("macos/native/src/macos.m");
    let source = std::fs::read_to_string(&native).unwrap();
    let cleanup = "session->handler = NULL;";
    assert_eq!(source.matches(cleanup).count(), 1);
    let mut source = source.replace(
        cleanup,
        "rinzu_macos_interrupt_mark(9);session->handler=NULL;",
    );
    source.push_str(include_str!("fixtures/macos_interrupt/markers.c"));
    std::fs::write(native, source).unwrap();
    std::fs::write(root.join("app/rinzu.toml"),"[package]\nname=\"macos_interrupt\"\nlanguage=\"0.2\"\n[dependencies]\n\"rinzu.macos\"={path=\"../macos\"}\n").unwrap();
    std::fs::write(
        root.join("app/src/main.rinzu"),
        include_str!("fixtures/macos_interrupt/main.rinzu"),
    )
    .unwrap();
    root.join("app")
}

fn preserve(mode: &str, policy: usize, output: &Output) {
    use std::os::unix::process::ExitStatusExt;
    let artifacts = std::env::var_os("RINZU_GATE_ARTIFACTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../artifacts/i7.6-r6"))
        .join("macos-interrupt");
    std::fs::create_dir_all(&artifacts).unwrap();
    let name = format!("{mode}-policy-{policy}");
    std::fs::write(artifacts.join(format!("{name}.stdout.log")), &output.stdout).unwrap();
    std::fs::write(artifacts.join(format!("{name}.stderr.log")), &output.stderr).unwrap();
    let evidence = serde_json::json!({
        "mode": mode,
        "inheritedPolicy": (["default", "ignored", "ignored-and-blocked"][policy]),
        "signalsSent": 1,
        "exitSignal": output.status.signal(),
        "terminationDeadlineSeconds": 3,
        "actualAppKitHandlerReached": true,
        "managedFinallyReached": false,
        "nativeFinallyReached": false,
    });
    std::fs::write(
        artifacts.join(format!("{name}.json")),
        serde_json::to_vec_pretty(&evidence).unwrap(),
    )
    .unwrap();
    println!("APPKIT_INTERRUPT={evidence}");
}

#[test]
fn actual_macos_loop_stops_on_one_default_interrupt_after_real_handler_in_jit_aot_signed_and_sanitizers()
 {
    let root = std::env::temp_dir().join(format!("rinzu-macos-interrupt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let app = create(&root);
    platform::success(&platform::bounded_tool(
        Command::new(frozen_cli::path()).arg("build").arg(&app),
    ));
    let binary = app.join("build/macos_interrupt");
    let sanitized = platform::sanitize(&binary);
    for policy in 0..3 {
        let mut command = Command::new(frozen_cli::path());
        command.arg("run").arg(&app);
        preserve(
            "compiled-jit",
            policy,
            &interrupt::terminate_once_with_startup_limit(&mut command, policy, true, 300),
        );
        for (mode, binary) in [("aot", &binary), ("sanitized-aot", &sanitized)] {
            let mut command = Command::new(binary);
            command
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .env("UBSAN_OPTIONS", "halt_on_error=1");
            preserve(
                mode,
                policy,
                &interrupt::terminate_once_with_startup_limit(&mut command, policy, true, 300),
            );
        }
    }
    platform::sign(&binary, &root);
    for policy in 0..3 {
        preserve(
            "signed-aot",
            policy,
            &interrupt::terminate_once_with_startup_limit(
                &mut Command::new(&binary),
                policy,
                true,
                300,
            ),
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}
