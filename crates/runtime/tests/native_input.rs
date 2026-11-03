#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "common/native_probe.rs"]
mod native_probe;
use rinzu_runtime::data::{ManagedData, NativeInputError};
use rinzu_runtime::heap::{CollectionKind, DomainHeap};

#[test]
fn dynamic_results_copy_binary_metadata_and_payload_before_external_mutation() {
    use rinzu_runtime::data::NativeByteArray;
    let mut heap = DomainHeap::new(13);
    let mut data = ManagedData::new(13);
    for length in [0, 1, 7, 8, 9, 256, 257, 4096, 4164, 65536] {
        let mut original: Vec<u8> = (0..length).map(|i| (i * 139) as u8).collect();
        let expected = original.clone();
        let mut view = NativeByteArray {
            data: if length == 0 {
                std::ptr::null_mut()
            } else {
                original.as_mut_ptr()
            },
            length,
        };
        // SAFETY: descriptor and all length bytes are readable until reception ends.
        let result = unsafe { data.receive_native_byte_array(&mut heap, &view) }.unwrap();
        original.fill(90);
        view.data = std::ptr::null_mut();
        view.length = usize::MAX;
        assert!(view.data.is_null());
        assert_eq!(view.length, usize::MAX);
        heap.collect(CollectionKind::Full).unwrap();
        assert_eq!(data.bytes(&heap, result, length as u64).unwrap(), expected);
        data.release(&mut heap, result).unwrap();
    }
    assert_eq!(
        // SAFETY: NULL is a specified recoverable result, not a native read fault.
        unsafe { data.receive_native_byte_array(&mut heap, std::ptr::null()) },
        Err(NativeInputError::NullByteArray)
    );
    data.release_all(&mut heap).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), 0);
}

#[test]
fn external_copies_preserve_n_data_ownership_and_string_error_boundaries() {
    let mut heap = DomainHeap::new(1);
    let mut data = ManagedData::new(1);
    let mut original = [0, 255, 7, 0];
    // SAFETY: exactly four readable bytes remain stable throughout reception.
    let bytes = unsafe { data.receive_native_bytes(&mut heap, 4, original.as_ptr()) }.unwrap();
    original.fill(42);
    // SAFETY: N=0 requires no readable native memory, including NULL.
    let empty = unsafe { data.receive_native_bytes(&mut heap, 0, std::ptr::null()) }.unwrap();
    // SAFETY: external NUL-terminated String, unrelated trailing native bytes.
    let text = unsafe { data.receive_native_string(&mut heap, b"A\0ignored".as_ptr()) }.unwrap();
    let count = heap.object_count();
    assert_eq!(
        // SAFETY: NULL is an explicitly recoverable native String input.
        unsafe { data.receive_native_string(&mut heap, std::ptr::null()) },
        Err(NativeInputError::NullString)
    );
    assert_eq!(
        // SAFETY: readable NUL-terminated invalid UTF-8 is rejected before body.
        unsafe { data.receive_native_string(&mut heap, c"\xff".as_ptr().cast()) },
        Err(NativeInputError::InvalidUtf8)
    );
    assert_eq!(heap.object_count(), count);
    for _ in 0..3 {
        data.retain_roots(&mut heap, [bytes, empty, text]).unwrap();
        heap.collect(CollectionKind::Full).unwrap();
        assert_eq!(data.bytes(&heap, bytes, 4).unwrap(), [0, 255, 7, 0]);
        assert_eq!(data.byte_length(&heap, empty, 0).unwrap(), 0);
        assert_eq!(data.string(&heap, text).unwrap(), "A");
    }
    data.release_all(&mut heap).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(data.root_count(), 0);
    assert_eq!(heap.object_count(), 0);
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn native_input_fatal_child() {
    use rinzu_runtime::callback_entry::{LengthCallback, LengthKind};
    use rinzu_runtime::data::ManagedRoots;
    let Ok(mode) = std::env::var("RINZU_NATIVE_INPUT_FAULT") else {
        return;
    };
    let mut heap = DomainHeap::new(1);
    let mut data = ManagedData::new(1);
    let mut roots = ManagedRoots::default();
    let callback = LengthCallback::prepare(
        &mut heap,
        &mut data,
        &mut roots,
        if mode == "string-null" {
            LengthKind::Utf8
        } else if mode == "zero-no-read" {
            LengthKind::Bytes(0)
        } else if mode == "size" {
            LengthKind::Bytes(u64::MAX)
        } else {
            LengthKind::Bytes(4)
        },
    );
    let entry = callback.entry();
    if mode == "closed" {
        drop(callback);
    }
    if mode == "read" || mode == "zero-no-read" {
        // SAFETY: a private unreadable page safely injects a native read fault in
        // this subprocess without touching unrelated process/system memory.
        let page = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                16384,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        };
        assert_ne!(page, libc::MAP_FAILED);
        // SAFETY: deliberate native contract fault contained in the subprocess.
        let result = unsafe { entry(page.cast()) };
        if mode == "zero-no-read" {
            assert_eq!(result, 0);
            // SAFETY: the subprocess owns this unused private mapping.
            assert_eq!(unsafe { libc::munmap(page, 16384) }, 0);
            return;
        }
    } else {
        // SAFETY: deliberate NULL/closed/oversize native boundary fault, tested
        // only in this disposable child. Positive NULL is never assumed to be OOM.
        unsafe {
            entry(if mode == "size" {
                b"x".as_ptr()
            } else {
                std::ptr::null()
            })
        };
    }
    panic!("fatal native boundary returned");
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn native_input_faults_do_not_return_or_become_recoverable_errors() {
    use std::os::unix::process::ExitStatusExt;
    use std::process::Command;
    for (mode, code, signal) in [
        ("zero-no-read", Some(0), false),
        ("null", Some(86), false),
        ("closed", Some(86), false),
        ("size", Some(71), false),
        ("read", None, true),
        ("string-null", Some(86), false),
    ] {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "native_input_fatal_child"])
            .env("RINZU_NATIVE_INPUT_FAULT", mode)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            code,
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.status.signal().is_some(), signal, "{mode}");
        assert!(
            !String::from_utf8_lossy(&output.stderr).contains("fatal native boundary returned")
        );
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn c_receivers_and_length_entries_pass_signed_aot_sanitizers_and_fault_gates() {
    use std::path::Path;
    use std::process::Command;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let directory = std::env::temp_dir().join(format!("rinzu-native-input-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    for sanitize in [false, true] {
        let binary = directory.join(if sanitize { "sanitized" } else { "signed" });
        let mut command = Command::new("clang");
        command
            .args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread"])
            .arg("-I")
            .arg(root.join("../../include"))
            .arg(root.join("tests/fixtures/native_input.c"))
            .arg(root.join("src/callback_entry.c"))
            .arg(root.join("src/callback_template.S"));
        if sanitize {
            command.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
        }
        let output = command.arg("-o").arg(&binary).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if !sanitize {
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
        let output = native_probe::run(&mut Command::new(&binary), false);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("PASS native length Main/Worker"));
        for (mode, code) in [
            ("array-zero-unreadable", Some(0)),
            ("array-null-payload", Some(86)),
            ("array-size", Some(71)),
            ("array-oom", Some(71)),
            ("array-read", None),
            ("array-descriptor", None),
            ("zero-no-read", Some(0)),
            ("null", Some(86)),
            ("closed", Some(86)),
            ("size", Some(71)),
            ("read", None),
            ("string-null", None),
            ("oom-0", None),
            ("oom-1", None),
            ("oom-3", None),
            ("oom-4", None),
        ] {
            let output = native_probe::run(
                Command::new(&binary).arg(mode),
                sanitize
                    && matches!(
                        mode,
                        "read" | "string-null" | "array-read" | "array-descriptor"
                    ),
            );
            if sanitize && mode == "read" {
                assert!(String::from_utf8_lossy(&output.stderr).contains("AddressSanitizer"));
            }
            if let Some(code) = code {
                assert_eq!(
                    output.status.code(),
                    Some(code),
                    "{mode}: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            } else {
                assert!(!output.status.success(), "{mode} returned from fault");
            }
            assert!(output.stdout.is_empty(), "{mode} entered successful body");
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}
