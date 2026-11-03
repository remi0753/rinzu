#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use rinzu_runtime::callback_return::{ReturnBuffer, stats};
use rinzu_runtime::data::ManagedData;
use rinzu_runtime::heap::{CollectionKind, DomainHeap};
use std::process::Command;

fn owner(domain: u64) -> Vec<ReturnBuffer> {
    let mut heap = DomainHeap::new(domain);
    let mut data = ManagedData::new(domain);
    let text = data.create_string(&mut heap, "日本語\0x").unwrap();
    let mut buffers = Vec::new();
    for _ in 0..1031 {
        let value = data.string(&heap, text).unwrap();
        buffers.push(ReturnBuffer::string(&value, false));
        data.retain_roots(&mut heap, [text]).unwrap();
        heap.collect(CollectionKind::Full).unwrap();
    }
    buffers.push(ReturnBuffer::bytes(&[0, 255], 8, true));
    buffers.push(ReturnBuffer::string("", false));
    buffers.push(ReturnBuffer::bytes(&[], 0, true));
    data.release_all(&mut heap).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), 0);
    assert_eq!(data.root_count(), 0);
    buffers
}
fn verify(buffers: &[ReturnBuffer]) {
    for buffer in &buffers[..1031] {
        assert_eq!(
            buffer.contents(),
            b"\xe6\x97\xa5\xe6\x9c\xac\xe8\xaa\x9e\0x\0"
        );
    }
    assert_eq!(buffers[1031].contents(), [0, 255, 0, 0, 0, 0, 0, 0]);
    assert_eq!(buffers[1032].contents(), [0]);
    assert_eq!(buffers[1033].contents(), [0]);
    assert!(buffers.iter().all(|buffer| !buffer.as_ptr().is_null()));
}
#[test]
fn native_return_copies_survive_gc_owner_exit_growth_and_concurrent_owners() {
    let before = stats();
    let mut all = vec![owner(1)];
    let workers = (2..10)
        .map(|domain| std::thread::spawn(move || owner(domain)))
        .collect::<Vec<_>>();
    for worker in workers {
        all.push(worker.join().unwrap());
    }
    let mut addresses = std::collections::HashSet::new();
    for buffers in &all {
        verify(buffers);
        for buffer in buffers {
            assert!(addresses.insert(buffer.bits()));
        }
    }
    let after = stats();
    assert_eq!(after.normal - before.normal, 1032 * 9);
    assert_eq!(after.fallback - before.fallback, 2 * 9);
    assert_eq!(after.bytes - before.bytes, (12 * 1031 + 8 + 1 + 1) * 9);
    assert_eq!(after.metadata_bytes - before.metadata_bytes, 1034 * 9 * 24);
    drop(all);
    assert_eq!(stats(), after);
}

#[test]
fn c_return_arena_signed_sanitized_growth_copy_ownership_and_allocation_faults() {
    let root = std::env::temp_dir().join(format!("rinzu-callback-return-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for sanitized in [false, true] {
        let binary = root.join(if sanitized { "sanitized" } else { "signed" });
        let mut clang = Command::new("clang");
        clang
            .args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread"])
            .arg(manifest.join("tests/fixtures/callback_return.c"))
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
        if !sanitized {
            for args in [
                vec!["--force", "--sign", "-", "--options", "runtime"],
                vec!["--verify", "--strict"],
            ] {
                let output = Command::new("codesign")
                    .args(args)
                    .arg(&binary)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
        let output = Command::new(&binary)
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("PASS return arena 6189 buffers"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("runtime error:"));
        for (mode, status) in [
            ("normal-node", 71),
            ("normal-buffer", 71),
            ("fallback-node", 71),
            ("fallback-buffer", 71),
            ("size", 71),
            ("prefix", 86),
            ("null", 86),
            ("string-padding", 86),
            ("kind", 86),
        ] {
            let output = Command::new(&binary)
                .arg(mode)
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(status),
                "{mode}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.stdout.is_empty());
            assert!(!String::from_utf8_lossy(&output.stderr).contains("ERROR: AddressSanitizer"));
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
