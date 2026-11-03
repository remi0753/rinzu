#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use rinzu_bindgen::source_package::validate_source_descriptor;

fn isolated(case: &str) -> bool {
    const ENV: &str = "RINZU_M4_CONSUMPTION_CHILD";
    if std::env::var(ENV).as_deref() == Ok(case) {
        return false;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("source_native_consumption_tests::{case}"),
            "--nocapture",
        ])
        .env(ENV, case)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    print!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

fn fixture(label: &str) -> std::path::PathBuf {
    let root =
        std::env::temp_dir().join(format!("rinzu-consumption-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    let root = root.canonicalize().unwrap();
    let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../sdk/consumption")
        .canonicalize()
        .unwrap();
    std::os::unix::fs::symlink(&sdk, root.join("consumption")).unwrap();
    std::fs::write(root.join("rinzu.toml"),
        "[package]\nname=\"consumption_app\"\nlanguage=\"0.5\"\n[dependencies]\n\"rinzu.consumption\"={path=\"consumption\"}\n").unwrap();
    std::fs::write(root.join("src/main.rinzu"),
        "import rinzu.consumption;fn label(index:CSize)->String fallback \"B\" {return \"A\";}fn data(index:CSize)->Bytes(4) fallback b\"\\x01\" {return b\"\\x01\\x02\\x03\";}fn execute()->Int {return Int(rinzu.consumption.strings(label,CSize(10))+rinzu.consumption.bytes4(data,CSize(10)));}fn main(){try {let result=execute();}catch unexpected {panic(\"failed\");}}").unwrap();
    root
}

#[test]
fn source_consumer_proof_binds_checked_ir_object_and_loaded_signature() {
    if isolated("source_consumer_proof_binds_checked_ir_object_and_loaded_signature") {
        return;
    }
    let root = fixture("proof");
    let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    let descriptor =
        validate_source_descriptor(&built.source_packages[0].artifact.expected_wire).unwrap();
    let report = rinzu_bindgen::consumption::decode(&descriptor.interface.package.build_inputs)
        .unwrap()
        .unwrap();
    assert!(report.analyzer.is_some(), "{report:?}");
    assert_eq!(report.functions.len(), 2);
    assert!(
        report.functions.values().all(|proof| proof.closed
            && proof.ir_digest.is_some()
            && proof.object_digest.is_some()),
        "{report:?}"
    );
    let loaded = built.load_source_packages().unwrap();
    assert_eq!(
        loaded
            .return_consumers(&artifact.ir.source_native_signatures)
            .unwrap()
            .len(),
        2
    );
    let before = rinzu_runtime::callback_return::scope_stats();
    let identity_before = rinzu_runtime::callback_entry::stats();
    let retained = rinzu_runtime::callback_return::stats();
    let run = rinzu_codegen::run_baseline_jit_with_package_bindings(
        &artifact.ir,
        "execute",
        Default::default(),
        None,
        Some(loaded),
    )
    .unwrap();
    assert_eq!(run.result, Ok(1420));
    assert!(run.native_calls.is_baseline());
    let after = rinzu_runtime::callback_return::scope_stats();
    assert_eq!(after.allocated - before.allocated, 22);
    assert_eq!(after.released - before.released, 22);
    assert_eq!(after.allocated_bytes - before.allocated_bytes, 66);
    assert_eq!(after.released_bytes - before.released_bytes, 66);
    assert_eq!(
        (after.live_bytes, after.active_scopes, after.metadata_bytes),
        (0, 0, 0)
    );
    assert_eq!(rinzu_runtime::callback_return::stats(), retained);
    let identity_after = rinzu_runtime::callback_entry::stats();
    assert_eq!(
        identity_after.retired_entries - identity_before.retired_entries,
        2
    );
    assert_eq!(
        identity_after.tombstone_bytes,
        identity_before.tombstone_bytes
    );
    let source = root.join("src/main.rinzu");
    let text = std::fs::read_to_string(&source).unwrap().replace(
        "fn main(){try {let result=execute();}catch unexpected {panic(\"failed\");}}",
        "async fn main(){try {let result=execute();if result!=1420 {panic(\"invalid\");}}catch unexpected {panic(\"failed\");}}",
    );
    std::fs::write(&source, text).unwrap();
    let (_, source_artifact) = rinzu_compiler::check_package(&root).unwrap();
    let run = rinzu_codegen::run_source_async_report(
        &source_artifact.ir,
        "main",
        rinzu_codegen::SourceAsyncOptions {
            source_packages: Some(built.load_source_packages().unwrap()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none() && run.native_calls.is_baseline());
    let after_source = rinzu_runtime::callback_return::scope_stats();
    let identity_source = rinzu_runtime::callback_entry::stats();
    assert_eq!(
        identity_source.retired_entries - identity_after.retired_entries,
        2
    );
    assert_eq!(
        identity_source.tombstone_bytes,
        identity_after.tombstone_bytes
    );
    assert_eq!(after_source.allocated - after.allocated, 22);
    assert_eq!(after_source.released - after.released, 22);
    assert_eq!(
        (
            after_source.live_bytes,
            after_source.active_scopes,
            after_source.metadata_bytes
        ),
        (0, 0, 0)
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn source_consumer_proof_rejects_unseen_escape_changed_include_and_stale_image() {
    let root = fixture("stale");
    // Mutate only a private copy, never the repository SDK symlink.
    let sdk = root.join("consumption");
    let original = sdk.canonicalize().unwrap();
    std::fs::remove_file(&sdk).unwrap();
    for name in [
        "rinzu.toml",
        "src/consumption.rinzu",
        "native/consumption.c",
        "native/consumption.h",
    ] {
        let path = sdk.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::copy(original.join(name), path).unwrap();
    }
    let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
    let first = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    let first_artifact = first.source_packages[0].artifact.clone();
    let source = sdk.join("native/consumption.c");
    let text = std::fs::read_to_string(&source).unwrap().replace(
        "sum += read_string(data);",
        "if (count == 999) m4_saved = data; sum += read_string(data);",
    );
    std::fs::write(&source, format!("static const char *m4_saved; const char *m4_saved_alias(void) {{return m4_saved;}}\n{text}")).unwrap();
    let header = sdk.join("native/consumption.h");
    let mut text = std::fs::read_to_string(&header).unwrap();
    text.push_str("\n/* changed included input */\n");
    std::fs::write(header, text).unwrap();
    let (_, changed) = rinzu_compiler::check_package(&root).unwrap();
    let second = crate::native_package::build_with_native_inputs(
        &changed.sdk_packages,
        &manifest.root,
        &changed.native_inputs,
    )
    .unwrap();
    let second_artifact = &second.source_packages[0].artifact;
    assert_ne!(first_artifact.expected_wire, second_artifact.expected_wire);
    let descriptor = validate_source_descriptor(&second_artifact.expected_wire).unwrap();
    let report = rinzu_bindgen::consumption::decode(&descriptor.interface.package.build_inputs)
        .unwrap()
        .unwrap();
    let string = &report.functions["rinzu.consumption::consumeString"];
    assert!(
        !string.closed
            && string
                .reasons
                .iter()
                .any(|reason| reason.contains("publish-pointer")),
        "{report:?}"
    );
    let selected = second
        .load_source_packages()
        .unwrap()
        .return_consumers(&changed.ir.source_native_signatures)
        .unwrap();
    assert_eq!(selected.len(), 1);
    assert!(selected.contains("rinzu.consumption::consumeBytes4"));
    let mixed = rinzu_codegen::source_package::SourcePackageArtifact {
        expected_wire: first_artifact.expected_wire,
        ..second_artifact.clone()
    };
    assert!(rinzu_codegen::source_package::LoadedSourcePackages::load(&[mixed]).is_err());
    let mut corrupted: serde_json::Value =
        serde_json::from_slice(&second_artifact.expected_wire).unwrap();
    corrupted["interface"]["package"]["build_inputs"][rinzu_bindgen::consumption::KEY] =
        serde_json::json!("{broken proof");
    assert!(validate_source_descriptor(&serde_json::to_vec(&corrupted).unwrap()).is_err());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn source_consumer_scopes_handle_nul_zero_padding_nested_fallback_worker_and_cancellation() {
    if isolated(
        "source_consumer_scopes_handle_nul_zero_padding_nested_fallback_worker_and_cancellation",
    ) {
        return;
    }
    use std::process::Command;
    let root = fixture("boundaries");
    let sdk = root.join("consumption");
    let original = sdk.canonicalize().unwrap();
    std::fs::remove_file(&sdk).unwrap();
    for name in [
        "rinzu.toml",
        "src/consumption.rinzu",
        "native/consumption.c",
        "native/consumption.h",
    ] {
        let path = sdk.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::copy(original.join(name), path).unwrap();
    }
    let append = |path: &std::path::Path, extra: &str| {
        let mut text = std::fs::read_to_string(path).unwrap();
        text.push_str(extra);
        std::fs::write(path, text).unwrap();
    };
    append(
        &sdk.join("native/consumption.h"),
        "\nuint64_t rinzu_consume_nul(const char *(*producer)(uint64_t),uint64_t count);\nuint64_t rinzu_consume_zero(const unsigned char *(*producer)(uint64_t),uint64_t count);\n",
    );
    append(
        &sdk.join("native/consumption.c"),
        r#"
uint64_t rinzu_consume_nul(const char *(*producer)(uint64_t),uint64_t count) {
  uint64_t sum=0;for(uint64_t i=0;i<count;i++) {
    const unsigned char *p=(const unsigned char *)producer(i);
    sum+=p[0]+p[1]+p[2]+p[3];sum+=p[0]+p[1]+p[2]+p[3];
  }return sum;
}

uint64_t rinzu_consume_zero(const unsigned char *(*producer)(uint64_t),uint64_t count) {
  uint64_t sum=0;for(uint64_t i=0;i<count;i++) {const unsigned char *p=producer(i);if(p)sum++;}return sum;
}
"#,
    );
    append(
        &sdk.join("src/consumption.rinzu"),
        "\nnative {fn nul(producer:fn(CSize)->String,count:CSize)->CSize symbol \"rinzu_consume_nul\";fn zero(producer:fn(CSize)->Bytes(0),count:CSize)->CSize symbol \"rinzu_consume_zero\";}\nexport nul;export zero;\n",
    );
    let prelude = r#"import rinzu.consumption;
fn data(index:CSize)->Bytes(4) fallback b"\xfe" {
  if index==CSize(1)||index==CSize(3) {let invalid=CInt8(300);}
  if index==CSize(2) {return b"\x04";}return b"\x01\x02\x03";
}
fn label(index:CSize)->String fallback "F\u{0}G" {
  let nested=rinzu.consumption.bytes4(data,CSize(4));if nested!=CSize(1036) {panic("invalidNested");}
  if index==CSize(1)||index==CSize(3) {let invalid=CInt8(300);}
  if index==CSize(2) {return "X\u{0}Y";}return "A\u{0}B";
}
fn zero(index:CSize)->Bytes(0) fallback b"" {if index==CSize(1)||index==CSize(3) {let invalid=CInt8(300);}return b"";}
fn empty(index:CSize)->String fallback "" {if index==CSize(1)||index==CSize(3) {let invalid=CInt8(300);}return "";}
fn execute()->Int {try {return Int(rinzu.consumption.nul(label,CSize(4))+rinzu.consumption.zero(zero,CSize(4))+rinzu.consumption.strings(empty,CSize(4))+rinzu.consumption.bytes4(data,CSize(4)));}catch unexpected {panic("unexpected failure"); return 0;}}
async fn main(){let result=await spawn execute();if result!=2220 {panic("invalid");}}
"#;
    std::fs::write(root.join("src/main.rinzu"), prelude).unwrap();
    let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    assert_eq!(
        built
            .load_source_packages()
            .unwrap()
            .return_consumers(&artifact.ir.source_native_signatures)
            .unwrap()
            .len(),
        4
    );
    let before = rinzu_runtime::callback_return::scope_stats();
    let jit = rinzu_codegen::run_baseline_jit_with_package_bindings(
        &artifact.ir,
        "execute",
        Default::default(),
        None,
        Some(built.load_source_packages().unwrap()),
    )
    .unwrap();
    assert_eq!(jit.result, Ok(2220));
    assert!(jit.native_calls.is_baseline());
    let after = rinzu_runtime::callback_return::scope_stats();
    assert_eq!(
        (
            after.allocated - before.allocated,
            after.released - before.released
        ),
        (24, 24)
    );
    assert_eq!(after.released_bytes - before.released_bytes, 78);
    let mut cancelled_after_callbacks = false;
    for cancel in [
        None,
        Some(2),
        Some(3),
        Some(4),
        Some(6),
        Some(8),
        Some(12),
        Some(20),
        Some(40),
    ] {
        let before = rinzu_runtime::callback_return::scope_stats();
        let run = rinzu_codegen::run_source_async_report(
            &artifact.ir,
            "main",
            rinzu_codegen::SourceAsyncOptions {
                source_packages: Some(built.load_source_packages().unwrap()),
                cancel_worker_after_polls: cancel,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(run.native_calls.is_baseline() && run.resources_returned);
        if cancel.is_none() {
            assert_eq!(run.last_awaited_result, Some(2220));
            assert!(run.failure_details.is_none());
        }
        if cancel.is_some()
            && run.failure_details.as_ref().is_some_and(|failure| {
                failure.primary.cause == rinzu_runtime::failure::FailureCause::Cancelled
            })
        {
            cancelled_after_callbacks |= run.native_calls.callback_entries > 0;
        }
        let after = rinzu_runtime::callback_return::scope_stats();
        assert_eq!(
            after.allocated - before.allocated,
            after.released - before.released
        );
        assert_eq!(
            (after.live_bytes, after.active_scopes, after.metadata_bytes),
            (0, 0, 0)
        );
        eprintln!(
            "BOUNDARY_SOURCE cancel={cancel:?} entries={} copies={} released={} bytes={} requested={}",
            run.native_calls.callback_entries,
            after.allocated - before.allocated,
            after.released - before.released,
            after.released_bytes - before.released_bytes,
            run.failure_details.is_some()
        );
    }
    assert!(cancelled_after_callbacks);
    let aot = crate::aot::build_package(&root, None).unwrap();
    let native = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime/src");
    let sanitized = aot.executable.with_extension("sanitized");
    let output = Command::new("clang")
        .args([
            "-std=c11",
            "-O1",
            "-g",
            "-fsanitize=address,undefined",
            "-fno-omit-frame-pointer",
            "-pthread",
        ])
        .arg(&aot.object)
        .arg(aot.executable.with_extension("runtime.c"))
        .arg(native.join("callback_entry.c"))
        .arg(native.join("callback_template.S"))
        .arg("-I")
        .arg(native.join("../../../include"))
        .arg("-o")
        .arg(&sanitized)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for binary in [&aot.executable, &sanitized] {
        let output = Command::new(binary)
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .env("ASAN_OPTIONS", "detect_leaks=0:halt_on_error=1")
            .env("UBSAN_OPTIONS", "halt_on_error=1")
            .output()
            .unwrap();
        let evidence = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.status.success(), "{evidence}");
        assert!(
            evidence.contains("RINZU_CALLBACK_SCOPES=24,24,0,78,78,0,0,"),
            "{evidence}"
        );
        assert!(
            evidence.contains("RINZU_CALLBACK_RETURNS=0,0,0,0"),
            "{evidence}"
        );
        assert!(
            !evidence.contains("ERROR: AddressSanitizer") && !evidence.contains("runtime error:"),
            "{evidence}"
        );
        eprintln!(
            "BOUNDARY_AOT {} scopes=24/24 bytes=78 live=0",
            binary.display()
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_consumer_growth_reports_physical_release_and_copy_costs() {
    if isolated("source_consumer_growth_reports_physical_release_and_copy_costs") {
        return;
    }
    let root = fixture("growth");
    let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    let path = root.join("src/main.rinzu");
    let base = std::fs::read_to_string(&path)
        .unwrap()
        .replace("fn execute()->Int {", "fn single()->Int {");
    rinzu_runtime::resource_accounting::enable();
    let mut rows = Vec::new();
    for repeats in [1, 10, 1000] {
        std::fs::write(&path,format!("{base}\nfn execute()->Int {{var i=0;var total=0;while i<{repeats} {{total=total+single();i=i+1;}}return total;}}" )).unwrap();
        let (_, artifact) = rinzu_compiler::check_package(&root).unwrap();
        let before = rinzu_runtime::callback_return::scope_stats();
        let started = std::time::Instant::now();
        let identity_before = rinzu_runtime::callback_entry::stats();
        let run = rinzu_codegen::run_baseline_jit_with_package_bindings(
            &artifact.ir,
            "execute",
            Default::default(),
            None,
            Some(built.load_source_packages().unwrap()),
        )
        .unwrap();
        let seconds = started.elapsed().as_secs_f64();
        assert_eq!(run.result, Ok(1420 * repeats));
        assert!(run.native_calls.is_baseline());
        let after = rinzu_runtime::callback_return::scope_stats();
        let expected = repeats as usize;
        assert_eq!(after.released - before.released, 22 * expected);
        assert_eq!(after.released_bytes - before.released_bytes, 66 * expected);
        assert_eq!(after.copied_bytes - before.copied_bytes, 52 * expected);
        assert_eq!(
            (after.live_bytes, after.active_scopes, after.metadata_bytes),
            (0, 0, 0)
        );
        let snapshot = rinzu_runtime::resource_accounting::snapshot();
        let identity_after = rinzu_runtime::callback_entry::stats();
        assert_eq!(
            identity_after.retired_entries - identity_before.retired_entries,
            2 * expected
        );
        assert_eq!(
            identity_after.tombstone_bytes,
            identity_before.tombstone_bytes
        );
        let control = snapshot
            .iter()
            .find(|row| row.id == "callback_control")
            .unwrap();
        assert_eq!(
            control.allocated.unwrap() - control.released.unwrap(),
            control.live.unwrap()
        );
        assert_eq!(
            snapshot
                .iter()
                .find(|row| row.id == "callback_retired_slots")
                .unwrap()
                .subset_of,
            Some("callback_control")
        );
        assert_eq!(
            snapshot
                .iter()
                .find(|row| row.id == "callback_scoped")
                .unwrap()
                .live,
            Some(0)
        );
        rows.push(serde_json::json!({"repeats":repeats,"full_jit_run_seconds":seconds,
            "released":after.released-before.released,"released_bytes":after.released_bytes-before.released_bytes,
            "copy_bytes":after.copied_bytes-before.copied_bytes,"peak_bytes":after.peak_bytes,
            "identity":{"entries":identity_after.entries,"retired":identity_after.retired_entries,
                "control_bytes":identity_after.control_bytes,"freed_bytes":identity_after.released_control_bytes,
                "tombstone_bytes":identity_after.tombstone_bytes,"virtual_bytes":identity_after.virtual_bytes},
            "categories":snapshot}));
    }
    let aot = crate::aot::build_package(&root, None).unwrap();
    let report = root.join("aot-resource.jsonl");
    let output = std::process::Command::new(&aot.executable)
        .env("RINZU_RESOURCE_REPORT", &report)
        .env("RINZU_TRANSPORT_EVIDENCE", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let snapshots = std::fs::read_to_string(&report)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    let end = snapshots.last().unwrap();
    let categories = end["categories"].as_array().unwrap();
    let scoped = categories
        .iter()
        .find(|row| row["id"] == "callback_scoped")
        .unwrap();
    let identity = |id: &str| categories.iter().find(|row| row["id"] == id).unwrap();
    assert_eq!(identity("callback_retired_entries")["live"], 2000);
    assert_eq!(identity("callback_tombstone")["live"], 0);
    assert_eq!(identity("callback_retired_slots")["live"], 64000);
    let control = identity("callback_control");
    assert_eq!(control["live"], 65536);
    assert_eq!(
        control["allocated"].as_u64().unwrap() - control["released"].as_u64().unwrap(),
        65536
    );
    assert_eq!(
        (
            scoped["allocated"].as_u64(),
            scoped["released"].as_u64(),
            scoped["live"].as_u64()
        ),
        (Some(66000), Some(66000), Some(0))
    );
    assert_eq!(
        categories
            .iter()
            .find(|row| row["id"] == "callback_return_copy")
            .unwrap()["allocated"],
        52000
    );
    // Historical evidence must never be replaced by a later regression run.
    if let Some(evidence_path) = std::env::var_os("RINZU_M4_GROWTH_EVIDENCE") {
        std::fs::write(evidence_path,serde_json::to_vec_pretty(&serde_json::json!({"scope":"production JIT run incl compilation; AOT 1000 repeats; payload vs identity separate","jit":rows,"aot":snapshots})).unwrap()).unwrap();
    }
    rinzu_runtime::resource_accounting::disable();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_consumer_saved_callback_capability_retains_only_its_full_control() {
    if isolated("source_consumer_saved_callback_capability_retains_only_its_full_control") {
        return;
    }
    let root = fixture("capability");
    let sdk = root.join("consumption");
    let original = sdk.canonicalize().unwrap();
    std::fs::remove_file(&sdk).unwrap();
    for name in [
        "rinzu.toml",
        "src/consumption.rinzu",
        "native/consumption.c",
        "native/consumption.h",
    ] {
        let path = sdk.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::copy(original.join(name), path).unwrap();
    }
    let source = sdk.join("native/consumption.c");
    let text = std::fs::read_to_string(&source).unwrap().replace(
        "const char *data = producer(index);",
        "if (count == 999) m4_saved_callback = producer; const char *data = producer(index);",
    );
    std::fs::write(&source, format!("#include <stdint.h>\nstatic const char *(*m4_saved_callback)(uint64_t);\nconst char *m4_late_callback(uint64_t i) {{return m4_saved_callback ? m4_saved_callback(i) : 0;}}\n{text}")).unwrap();
    let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    let loaded = built.load_source_packages().unwrap();
    let selected = loaded
        .return_consumers(&artifact.ir.source_native_signatures)
        .unwrap();
    assert_eq!(selected.len(), 1);
    assert!(selected.contains("rinzu.consumption::consumeBytes4"));
    let before = rinzu_runtime::callback_entry::stats();
    let run = rinzu_codegen::run_baseline_jit_with_package_bindings(
        &artifact.ir,
        "execute",
        Default::default(),
        None,
        Some(loaded),
    )
    .unwrap();
    assert_eq!(run.result, Ok(1420));
    assert!(run.native_calls.is_baseline());
    let after = rinzu_runtime::callback_entry::stats();
    assert_eq!(after.entries - before.entries, 2);
    assert_eq!(after.retired_entries - before.retired_entries, 1);
    assert!(after.tombstone_bytes > before.tombstone_bytes);
    assert_eq!(after.live_connections, before.live_connections);
    eprintln!(
        "CAPABILITY_MIXED entries=2 retired=1 unknown_tombstone_bytes={}",
        after.tombstone_bytes - before.tombstone_bytes
    );
    std::fs::remove_dir_all(root).unwrap();
}
