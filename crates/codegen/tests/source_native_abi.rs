#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use rinzu_codegen::source_native_abi::{NativeAbiStubs, emit_object};
use rinzu_compiler::{FileId, SourceFile, compile_source};
use rinzu_runtime::callback_entry::{LengthCallback, LengthKind};
use rinzu_runtime::data::{ManagedData, ManagedRoots};
use rinzu_runtime::heap::DomainHeap;
use std::ffi::CString;
use std::path::Path;
use std::process::Command;

fn signatures() -> Vec<rinzu_compiler::SourceNativeSignature> {
    compile_source(&SourceFile::new(
        FileId(0),
        "native-abi.rinzu",
        include_str!("fixtures/native_abi.rinzu"),
    ))
    .unwrap()
    .ir
    .source_native_signatures
}

#[test]
fn signature_stubs_preserve_c_registers_stack_widths_bits_and_one_pointer_callback() {
    let signatures = signatures();
    let stubs = NativeAbiStubs::new(&signatures).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let directory =
        std::env::temp_dir().join(format!("rinzu-native-abi-jit-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let library = directory.join("fixture.dylib");
    let output = Command::new("clang")
        .args(["-O2", "-Wall", "-Wextra", "-Werror", "-dynamiclib"])
        .arg(root.join("tests/fixtures/native_abi.c"))
        .arg("-o")
        .arg(&library)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let library = CString::new(library.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: the test pins this exact fixture until all typed invocations return.
    let handle = unsafe { libc::dlopen(library.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    assert!(!handle.is_null());
    let invoke = |name: &str, words: &[u64]| {
        let signature = signatures
            .iter()
            .find(|signature| signature.name == name)
            .unwrap();
        let rinzu_compiler::ast::NativeBinding::Symbol(symbol) = &signature.binding else {
            unreachable!()
        };
        let symbol = CString::new(symbol.as_str()).unwrap();
        // SAFETY: live fixture handle and an exact declared C symbol.
        let address = unsafe { libc::dlsym(handle, symbol.as_ptr()) } as usize;
        // SAFETY: Clang's fixture matches the source's complete typed C signature;
        // argument words carry the documented scalar/pointer representations.
        unsafe { stubs.invoke(name, address, words) }.unwrap()
    };
    for (name, bits) in [
        ("signed8", i8::MIN as u64),
        ("signed16", i16::MIN as u64),
        ("signed32", i32::MIN as u64),
        ("signed64", i64::MIN as u64),
        ("unsigned8", u8::MAX as u64),
        ("unsigned16", u16::MAX as u64),
        ("unsigned32", u32::MAX as u64),
        ("unsigned64", u64::MAX),
    ] {
        assert_eq!(invoke(name, &[bits]), bits);
    }
    for bits in [0x80000000, 0x7fc01234, 0x3f800001, 0x7f7fffff] {
        assert_eq!(invoke("echoFloat", &[bits]), bits);
    }
    for bits in [
        0x8000000000000000,
        0x7ff8123400004321,
        0x3ff0000000000001,
        0x7fefffffffffffff,
    ] {
        assert_eq!(invoke("echoDouble", &[bits]), bits);
    }
    assert_eq!(invoke("boolean", &[0]), 0);
    assert_eq!(invoke("boolean", &[1]), 1);
    assert_eq!(invoke("empty", &[]), 0);
    assert_eq!(invoke("count", &[]), 1);
    assert_eq!(
        invoke(
            "mixed",
            &[
                i8::MIN as u64,
                u8::MAX as u64,
                i16::MIN as u64,
                u16::MAX as u64,
                i32::MIN as u64,
                u32::MAX as u64,
                i64::MIN as u64,
                u64::MAX,
                0x3f800001,
                0x8000000000000000,
                1
            ]
        ),
        u64::MAX
    );
    let words = (1..=10)
        .map(|number| {
            if number % 2 == 1 {
                (number as f32).to_bits() as u64
            } else {
                (number as f64).to_bits()
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(invoke("floating", &words), 55_f64.to_bits());
    let mut heap = DomainHeap::new(17);
    let mut data = ManagedData::new(17);
    let mut roots = ManagedRoots::default();
    let callback = LengthCallback::prepare(&mut heap, &mut data, &mut roots, LengthKind::Bytes(4));
    let input = [0_u8, 255, 7, 0];
    assert_eq!(
        invoke(
            "invoke",
            &[callback.entry() as usize as u64, input.as_ptr() as u64]
        ),
        4
    );
    assert_eq!(callback.evidence().prepared_inputs, 1);
    assert_eq!(callback.evidence().body_entries, 1);
    drop(callback);
    assert_eq!(heap.object_count(), 0);
    assert_eq!(data.root_count(), 0);
    // SAFETY: all fixture calls have returned; its addresses are no longer used.
    assert_eq!(unsafe { libc::dlclose(handle) }, 0);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn native_abi_validation_refuses_missing_arity_null_entry_and_duplicate_identity() {
    let signatures = signatures();
    let stubs = NativeAbiStubs::new(&signatures).unwrap();
    // SAFETY: the deliberately invalid entries/counts must be refused before a call.
    assert!(unsafe { stubs.invoke("missing", 1, &[]) }.is_err());
    // SAFETY: invalid null entry is checked before attempting external invocation.
    assert!(unsafe { stubs.invoke("empty", 0, &[]) }.is_err());
    // SAFETY: argument count is checked before accessing this deliberately bogus address.
    assert!(unsafe { stubs.invoke("mixed", 1, &[]) }.is_err());
    let duplicate = [signatures[0].clone(), signatures[0].clone()];
    assert!(NativeAbiStubs::new(&duplicate).is_err());
    assert!(emit_object(&duplicate).is_err());
}

#[test]
fn shared_signature_stubs_pass_signed_aot_and_native_sanitizers_on_main_and_workers() {
    let object = emit_object(&signatures()).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let directory =
        std::env::temp_dir().join(format!("rinzu-native-abi-aot-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("stubs.o"), object.object).unwrap();
    let mut source = include_str!("fixtures/native_abi.c").to_owned();
    source.push_str(include_str!("fixtures/native_abi_aot.c"));
    for (name, symbol) in object.symbols {
        source = source.replace(&format!("STUB_{name}("), &format!("{symbol}("));
    }
    assert!(!source.contains("STUB_"));
    std::fs::write(directory.join("main.c"), source).unwrap();
    for sanitize in [false, true] {
        let binary = directory.join(if sanitize { "sanitized" } else { "signed" });
        let mut command = Command::new("clang");
        command
            .args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread"])
            .arg("-I")
            .arg(root.join("../../include"))
            .arg(directory.join("main.c"))
            .arg(directory.join("stubs.o"))
            .arg(root.join("../runtime/src/callback_entry.c"))
            .arg(root.join("../runtime/src/callback_template.S"));
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
        let output = Command::new(&binary)
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains("PASS physical native ABI AOT/Main/Worker")
        );
    }
    std::fs::remove_dir_all(directory).unwrap();
}
