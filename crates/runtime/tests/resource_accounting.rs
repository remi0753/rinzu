use rinzu_runtime::data::{ManagedData, NativeInputError};
use rinzu_runtime::heap::{CollectionKind, DomainHeap, FinalizerPolicy, HeapValue};
use rinzu_runtime::resource_accounting::{self as accounting, Kind};
use rinzu_runtime::resources::{ResourceKind, ResourceLedger, ResourceLimits};

#[test]
fn known_domain_input_credit_retention_and_unknown_measurements_are_distinct() {
    accounting::enable();
    let before = accounting::counter(Kind::Managed);
    {
        let mut heap = DomainHeap::new(11);
        let live = heap.allocate(1, &[], FinalizerPolicy::None).unwrap();
        heap.allocate(1, &[HeapValue::Integer(7)], FinalizerPolicy::None)
            .unwrap();
        assert_eq!(heap.allocated_bytes(), 64);
        assert_eq!(accounting::counter(Kind::Managed).live - before.live, 64);
        let root = heap.create_root(Some(live)).unwrap();
        heap.collect(CollectionKind::Full).unwrap();
        let measured = accounting::counter(Kind::Managed);
        assert_eq!(measured.allocated - before.allocated, 64);
        assert_eq!(measured.live - before.live, 24);
        assert_eq!(measured.released - before.released, 40);
        for _ in 0..3 {
            heap.collect(CollectionKind::Full).unwrap();
        }
        assert_eq!(accounting::counter(Kind::Managed), measured);
        heap.release_root(root).unwrap();
    }
    assert_eq!(accounting::counter(Kind::Managed).live, before.live);
    assert_eq!(
        accounting::counter(Kind::Managed).released - before.released,
        64
    );
    let input = accounting::counter(Kind::NativeInput);
    {
        let mut heap = DomainHeap::new(12);
        let mut data = ManagedData::new(12);
        // SAFETY: exactly four bytes remain readable through immediate copy.
        unsafe { data.receive_native_bytes(&mut heap, 4, [0, 255, 4, 0].as_ptr()) }.unwrap();
        // SAFETY: one invalid UTF-8 byte and a readable terminating NUL.
        assert_eq!(
            // SAFETY: one invalid UTF-8 byte and a readable terminating NUL.
            unsafe { data.receive_native_string(&mut heap, c"\xff".as_ptr().cast()) },
            Err(NativeInputError::InvalidUtf8)
        );
    }
    let after = accounting::counter(Kind::NativeInput);
    assert_eq!(after.live, input.live);
    assert_eq!(after.allocated - input.allocated, 5);
    assert_eq!(after.released - input.released, 5);
    let mut ledger = ResourceLedger::new(ResourceLimits::default());
    let credits = [
        (ResourceKind::LiveTask, 3),
        (ResourceKind::PendingOperation, 2),
        (ResourceKind::QueueSlot, 1),
    ];
    ledger.reserve(&credits).unwrap();
    assert_eq!(accounting::counter(Kind::Task).live, 3);
    ledger.release(&credits).unwrap();
    assert_eq!(accounting::counter(Kind::Task).live, 0);
    assert_eq!(accounting::counter(Kind::Task).peak, 3);
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        use rinzu_runtime::callback_return::{ReturnBuffer, stats};
        let before = stats();
        let normal = ReturnBuffer::string("ab", false);
        let fallback = ReturnBuffer::bytes(&[], 0, true);
        assert_eq!(normal.contents(), b"ab\0");
        assert_eq!(fallback.contents(), b"\0");
        let after = stats();
        assert_eq!(after.normal_bytes - before.normal_bytes, 3);
        assert_eq!(after.fallback_bytes - before.fallback_bytes, 1);
        assert_eq!(
            after.metadata_bytes - before.metadata_bytes,
            normal.metadata_bytes() * 2
        );
    }
    let rows = accounting::snapshot();
    let sdk = rows.iter().find(|row| row.id == "sdk_memory").unwrap();
    assert_eq!(sdk.status, "unknown");
    assert_eq!(sdk.live, None);
    #[cfg(target_os = "macos")]
    for id in ["rss", "virtual", "footprint"] {
        let row = rows.iter().find(|row| row.id == id).unwrap();
        assert_eq!(row.status, "measured");
        assert!(row.live.unwrap() > 0);
        assert_eq!(row.allocated, None);
    }
    accounting::disable();
}

#[cfg(unix)]
#[test]
fn c_counter_known_allocations_gc_input_and_retained_buffers() {
    use std::process::Command;
    let directory =
        std::env::temp_dir().join(format!("rinzu-resource-counters-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for sanitized in [false, true] {
        let binary = directory.join(if sanitized { "sanitized" } else { "normal" });
        let mut clang = Command::new("clang");
        clang
            .args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread"])
            .arg(manifest.join("tests/fixtures/resource_accounting.c"))
            .arg("-I")
            .arg(manifest.join("../../include"));
        if sanitized {
            clang.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
        }
        let output = clang.arg("-o").arg(&binary).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        for enabled in ["0", "1"] {
            let output = Command::new(&binary)
                .env("RINZU_MANAGED_INLINE_BYTES", enabled)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "inline={enabled} {:?}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                String::from_utf8(output.stdout)
                    .unwrap()
                    .contains("resource accounting passed")
            );
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn concurrent_native_domain_updates_and_sampler_pass_memory_and_race_sanitizers() {
    use std::process::Command;
    let directory =
        std::env::temp_dir().join(format!("rinzu-resource-sampler-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for (name, sanitizer) in [("memory", "address,undefined"), ("race", "thread")] {
        let binary = directory.join(name);
        let output = Command::new("clang")
            .args([
                "-std=c11",
                "-O1",
                "-g",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-pthread",
                "-fno-omit-frame-pointer",
            ])
            .arg(format!("-fsanitize={sanitizer}"))
            .arg(manifest.join("tests/fixtures/resource_sampler.c"))
            .arg("-I")
            .arg(manifest.join("../../include"))
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report = directory.join(format!("{name}.jsonl"));
        let output = Command::new(binary).arg(&report).output().unwrap();
        assert!(
            output.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let rows: Vec<serde_json::Value> = std::fs::read_to_string(report)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(rows.first().unwrap()["phase"], "start");
        assert_eq!(rows.last().unwrap()["phase"], "end");
        assert!(rows.iter().any(|row| row["phase"] == "sample"));
    }
    std::fs::remove_dir_all(directory).unwrap();
}
