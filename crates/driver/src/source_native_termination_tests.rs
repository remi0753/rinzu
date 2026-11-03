#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod fixture {
    include!("../tests/fixtures/native_termination_sdk/setup.rs");
}
mod assertions {
    include!("../tests/fixtures/native_termination_sdk/assertions.rs");
}
mod interrupt {
    include!("../tests/fixtures/native_termination_sdk/interrupt.rs");
}
mod pump_fixture {
    include!("../tests/fixtures/native_pump_sdk/setup.rs");
}
mod pump_assertions {
    include!("../tests/fixtures/native_pump_sdk/assertions.rs");
}

#[test]
fn compiled_async_validates_and_opens_each_native_facade_once() {
    const CHILD: &str = "RINZU_FACADE_INITIALIZER_ROOT";
    if let Some(root) = std::env::var_os(CHILD) {
        let root = std::path::PathBuf::from(root);
        let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
        let built = crate::native_package::build_with_native_inputs(
            &artifact.sdk_packages,
            &manifest.root,
            &artifact.native_inputs,
        )
        .unwrap();
        for name in ["inspection-one", "inspection-two"] {
            crate::source_package_bundle::save(&built.source_packages, &root.join(name)).unwrap();
        }
        assert!(
            !root.join("initialized").exists(),
            "saving bundles must not execute native initializers"
        );
        std::fs::remove_dir_all(built.directory).unwrap();
        let jit = crate::compiled_async::run(&manifest, &artifact, None, None, true).unwrap();
        assert_eq!(jit.exit_code, 0, "{}", jit.output);
        assert!(jit.output.contains("\"resources\":\"baseline\""));
        assert_eq!(
            std::fs::read_to_string(root.join("initialized")).unwrap(),
            "initialized\n"
        );
        return;
    }
    let root =
        std::env::temp_dir().join(format!("rinzu-facade-initializer-{}", std::process::id()));
    fixture::create(&root);
    let api = root.join("termination/api.c");
    let mut contents = std::fs::read_to_string(&api).unwrap();
    contents.push_str("\n__attribute__((constructor)) static void initialize_facade(void){const char *path=getenv(\"RINZU_FACADE_INITIALIZER_LOG\");assert(path);FILE *file=fopen(path,\"a\");assert(file);assert(fputs(\"initialized\\n\",file)>=0);assert(!fclose(file));}\n");
    std::fs::write(api, contents).unwrap();
    std::fs::write(root.join("app/src/main.rinzu"),"import termination; fn callback() {} async fn main(){try{let value=await termination.runForever(callback,\"interrupt 日本語\",b\"\\x00\\xAA\\x00\\x7F\");if value!=7 {panic(\"bad\");}}catch errors{panic(\"failed\");}}").unwrap();
    // This physical loop returns immediately; no interrupt or native-stop
    // behavior is involved in the initializer/loader regression.
    let api = root.join("termination/api.c");
    let contents = std::fs::read_to_string(&api).unwrap().replace(
        "entry();\n  callback();\n  return forever(input, bytes);",
        "(void)input;(void)bytes;callback();return 7;",
    );
    std::fs::write(api, contents).unwrap();
    let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","source_native_termination_tests::compiled_async_validates_and_opens_each_native_facade_once","--nocapture"]).env(CHILD,&root).env("RINZU_FACADE_INITIALIZER_LOG",root.join("initialized")).output().unwrap();
    assert!(output.status.success(), "{output:?}");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn blocked_worker_keeps_roots_and_admission_while_other_main_cleanup_calls_ordinary_stop() {
    const CHILD: &str = "RINZU_OWNER_PUMP_SOURCE_ROOT";
    if let Some(root) = std::env::var_os(CHILD) {
        let root = std::path::PathBuf::from(root);
        let shutdown = std::env::var("RINZU_OWNER_PUMP_MODE").unwrap() == "shutdown";
        let (cancel, shutdown_limit) = if shutdown {
            (None, Some(80))
        } else {
            (Some(80), None)
        };
        let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
        let built = crate::native_package::build_with_native_inputs(
            &artifact.sdk_packages,
            &manifest.root,
            &artifact.native_inputs,
        )
        .unwrap();
        let source = rinzu_codegen::run_source_async_report(
            &artifact.ir,
            "main",
            rinzu_codegen::SourceAsyncOptions {
                gc_stress: true,
                source_packages: Some(built.load_source_packages().unwrap()),
                package_root: manifest.root.clone(),
                cancel_worker_after_polls: cancel,
                shutdown_worker_after_polls: shutdown_limit,
                completion_timeout: std::time::Duration::from_millis(25),
                record_transport_evidence: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            source.failure_details.as_ref().unwrap().primary.cause,
            if shutdown {
                rinzu_runtime::failure::FailureCause::Shutdown
            } else {
                rinzu_runtime::failure::FailureCause::Cancelled
            }
        );
        assert!(source.resources_returned && source.native_calls.is_baseline());
        assert_eq!(source.native_calls.returned, 7);
        assert_eq!(source.worker_returns, 2);
        assert_eq!(source.worker_heaps.len(), 2);
        assert!(
            source
                .worker_heaps
                .iter()
                .all(|worker| worker.heap.objects == 0 && worker.heap.roots == 0)
        );
        assert!(
            source
                .worker_heaps
                .iter()
                .any(|worker| worker.heap.collections > 50 && worker.heap.peak_frames >= 2)
        );
        assert!(source.heap.collections > 50);
        let jit = crate::compiled_async::run(&manifest, &artifact, None, None, true).unwrap();
        assert_eq!(jit.exit_code, 1, "{}", jit.output);
        crate::aot::compare_source_async_values_and_transport(&source, &jit.output).unwrap();
        crate::aot::compare_source_async_failures(&source, &jit.output).unwrap();
        println!("OWNER_PUMP_RESOURCES=baseline");
        return;
    }
    let root = std::env::temp_dir().join(format!("rinzu-owner-pump-source-{}", std::process::id()));
    pump_fixture::create(&root);
    for mode in ["cancel", "shutdown"] {
        let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","source_native_termination_tests::blocked_worker_keeps_roots_and_admission_while_other_main_cleanup_calls_ordinary_stop","--nocapture"]).env(CHILD,&root).env("RINZU_OWNER_PUMP_MODE",mode).env(if mode=="shutdown" {"RINZU_SHUTDOWN_WORKER_AFTER_POLLS"} else {"RINZU_CANCEL_WORKER_AFTER_POLLS"},"80").output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("OWNER_PUMP_RESOURCES=baseline"));
        pump_assertions::progress(&String::from_utf8_lossy(&output.stderr), 2);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn embedded_source_and_compiled_jit_preserve_the_hosts_interrupt_disposition_and_mask() {
    const CHILD: &str = "RINZU_INTERRUPT_EMBEDDED_ROOT";
    if let Some(root) = std::env::var_os(CHILD) {
        let root = std::path::PathBuf::from(root);
        // SAFETY: this isolated child changes only its native process policy.
        unsafe {
            assert_ne!(libc::signal(libc::SIGINT, libc::SIG_IGN), libc::SIG_ERR);
            let mut set = std::mem::zeroed();
            assert_eq!(libc::sigemptyset(&mut set), 0);
            assert_eq!(libc::sigaddset(&mut set, libc::SIGINT), 0);
            assert_eq!(
                libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()),
                0
            );
        }
        let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
        let source =
            rinzu_codegen::run_source_async_report(&artifact.ir, "main", Default::default())
                .unwrap();
        assert!(source.resources_returned && source.failure_details.is_none());
        let jit = crate::compiled_async::run(&manifest, &artifact, None, None, true).unwrap();
        assert_eq!(jit.exit_code, 0, "{}", jit.output);
        // SAFETY: native snapshots confirm that neither embedded execution
        // changed the host's policy or unblocked its thread's interrupt.
        unsafe {
            let mut action = std::mem::zeroed();
            let mut mask = std::mem::zeroed();
            assert_eq!(
                libc::sigaction(libc::SIGINT, std::ptr::null(), &mut action),
                0
            );
            assert_eq!(action.sa_sigaction, libc::SIG_IGN);
            assert_eq!(libc::pthread_sigmask(0, std::ptr::null(), &mut mask), 0);
            assert_eq!(libc::sigismember(&mask, libc::SIGINT), 1);
        }
        return;
    }
    let root =
        std::env::temp_dir().join(format!("rinzu-interrupt-embedded-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("rinzu.toml"),
        "[package]\nname=\"embedded\"\nlanguage=\"0.2\"\n",
    )
    .unwrap();
    std::fs::write(root.join("src/main.rinzu"),"async fn value()->Int {return 7;}async fn main(){let actual=await value();if actual!=7 {panic(\"wrong\");}}").unwrap();
    let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","source_native_termination_tests::embedded_source_and_compiled_jit_preserve_the_hosts_interrupt_disposition_and_mask","--nocapture"]).env(CHILD,&root).output().unwrap();
    assert!(output.status.success(), "{output:?}");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_permanent_native_blocks_terminate_on_one_default_interrupt_after_inherited_ignore_and_mask()
 {
    const CHILD: &str = "RINZU_INTERRUPT_SOURCE_ROOT";
    if let Some(root) = std::env::var_os(CHILD) {
        rinzu_runtime::standalone::initialize_interrupt();
        let root = std::path::PathBuf::from(root);
        let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
        let built = crate::native_package::build_with_native_inputs(
            &artifact.sdk_packages,
            &manifest.root,
            &artifact.native_inputs,
        )
        .unwrap();
        let _ = rinzu_codegen::run_source_async_report(
            &artifact.ir,
            "main",
            rinzu_codegen::SourceAsyncOptions {
                gc_stress: true,
                source_packages: Some(built.load_source_packages().unwrap()),
                ..Default::default()
            },
        )
        .unwrap();
        panic!("permanent native block returned");
    }
    let root = std::env::temp_dir().join(format!("rinzu-interrupt-source-{}", std::process::id()));
    fixture::create(&root);
    for place in ["supplant", "main", "worker"] {
        std::fs::write(root.join("app/src/main.rinzu"), interrupt::source(place)).unwrap();
        for policy in 0..3 {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command.args(["--exact", "source_native_termination_tests::source_permanent_native_blocks_terminate_on_one_default_interrupt_after_inherited_ignore_and_mask", "--nocapture"]).env(CHILD,&root);
            interrupt::terminate_once(&mut command, policy, place == "supplant");
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_callback_fail_fast_has_no_native_return_parent_cleanup_or_implicit_unit_fallback() {
    const CHILD: &str = "RINZU_TERMINATION_SOURCE_ROOT";
    if let Some(root) = std::env::var_os(CHILD) {
        let root = std::path::PathBuf::from(root);
        let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
        let built = crate::native_package::build_with_native_inputs(
            &artifact.sdk_packages,
            &manifest.root,
            &artifact.native_inputs,
        )
        .unwrap();
        let source = rinzu_codegen::run_source_async_report(
            &artifact.ir,
            "main",
            rinzu_codegen::SourceAsyncOptions {
                gc_stress: true,
                source_packages: Some(built.load_source_packages().unwrap()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            source.failure_details.is_none()
                && source.resources_returned
                && source.native_calls.is_baseline()
        );
        return;
    }
    let root =
        std::env::temp_dir().join(format!("rinzu-termination-source-{}", std::process::id()));
    fixture::create(&root);
    for mode in 0..9 {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "source_native_termination_tests::source_callback_fail_fast_has_no_native_return_parent_cleanup_or_implicit_unit_fallback", "--nocapture"])
            .env(CHILD, &root).env("RINZU_TERMINATION_MODE", mode.to_string()).output().unwrap();
        assertions::boundary(&output, mode);
    }
    std::fs::remove_dir_all(root).unwrap();
}
