#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use core::ffi::c_void;
use rinzu_runtime::callback_entry::{
    CallbackAbiCursor, CallbackFrame, FrameCallback, PointerU64Callback, stats,
};
use rinzu_runtime::scalar::ScalarKind;
use std::path::Path;
use std::process::Command;
static CALLBACK_TEST_GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());

type MixedEntry = unsafe extern "C" fn(
    i8,
    f32,
    u16,
    f64,
    i32,
    u64,
    *const u8,
    i16,
    u8,
    i64,
    bool,
    u32,
    i8,
    u16,
    *const u8,
    f32,
    f64,
    f32,
    f64,
    f32,
    f64,
    f32,
    f64,
    f32,
) -> f64;

const MIXED_KINDS: [Option<ScalarKind>; 24] = [
    Some(ScalarKind::I8),
    Some(ScalarKind::F32),
    Some(ScalarKind::U16),
    Some(ScalarKind::F64),
    Some(ScalarKind::I32),
    Some(ScalarKind::U64),
    None,
    Some(ScalarKind::I16),
    Some(ScalarKind::U8),
    Some(ScalarKind::I64),
    Some(ScalarKind::Bool),
    Some(ScalarKind::U32),
    Some(ScalarKind::I8),
    Some(ScalarKind::U16),
    None,
    Some(ScalarKind::F32),
    Some(ScalarKind::F64),
    Some(ScalarKind::F32),
    Some(ScalarKind::F64),
    Some(ScalarKind::F32),
    Some(ScalarKind::F64),
    Some(ScalarKind::F32),
    Some(ScalarKind::F64),
    Some(ScalarKind::F32),
];
const MIXED_RESULT: u64 = 0xfff8_0102_0304_0506;

struct MixedContext {
    identity: u64,
    calls: usize,
}
unsafe extern "C" fn mixed_handler(raw: *mut c_void, frame: *const CallbackFrame) -> u64 {
    // SAFETY: the test pins context until the exact mixed entry closes.
    let context = unsafe { &mut *raw.cast::<MixedContext>() };
    // SAFETY: the bridge supplies the live frame before calling this handler.
    let frame = unsafe { &*frame };
    let expected = [
        i8::MIN as i64 as u64,
        0x8000_0000,
        u16::MAX as u64,
        0x7ff8_1234_5678_9abc,
        i32::MIN as i64 as u64,
        u64::MAX,
        bytes_pointer(),
        i16::MIN as i64 as u64,
        u8::MAX as u64,
        i64::MIN as u64,
        1,
        u32::MAX as u64,
        (-7_i64) as u64,
        64523,
        bytes_pointer(),
        0x7f80_0000,
        0xfff0_0000_0000_0000,
        0x7fc1_2345,
        0x8000_0000_0000_0000,
        0,
        42.5_f64.to_bits(),
        1.25_f32.to_bits() as u64,
        2.5_f64.to_bits(),
        (-2.25_f32).to_bits() as u64,
    ];
    let mut cursor = CallbackAbiCursor::default();
    for (index, kind) in MIXED_KINDS.into_iter().enumerate() {
        assert_eq!(
            // SAFETY: this sequence is exactly MixedEntry's fixed Darwin ABI.
            unsafe { frame.word(&mut cursor, kind) },
            expected[index],
            "parameter {index}"
        );
    }
    assert_eq!((cursor.integers, cursor.floats, cursor.stack), (8, 8, 44));
    context.calls += 1;
    MIXED_RESULT ^ context.identity
}
static MIXED_BYTES: [u8; 4] = [0, 255, 7, 0];
fn bytes_pointer() -> u64 {
    MIXED_BYTES.as_ptr() as u64
}

unsafe fn call_mixed(entry: *const c_void) -> u64 {
    // SAFETY: every entry passed here was prepared for precisely MixedEntry.
    let entry: MixedEntry = unsafe { std::mem::transmute(entry) };
    // SAFETY: scalar arguments use their exact types, pointer inputs remain live.
    unsafe {
        entry(
            i8::MIN,
            -0.0,
            u16::MAX,
            f64::from_bits(0x7ff8_1234_5678_9abc),
            i32::MIN,
            u64::MAX,
            MIXED_BYTES.as_ptr(),
            i16::MIN,
            u8::MAX,
            i64::MIN,
            true,
            u32::MAX,
            -7,
            64523,
            MIXED_BYTES.as_ptr(),
            f32::INFINITY,
            f64::NEG_INFINITY,
            f32::from_bits(0x7fc1_2345),
            -0.0,
            0.0,
            42.5,
            1.25,
            2.5,
            -2.25,
        )
        .to_bits()
    }
}

#[test]
fn generic_frame_entry_preserves_mixed_registers_packed_stack_and_float_result() {
    let _gate = CALLBACK_TEST_GATE.lock().unwrap();
    assert_eq!(std::mem::size_of::<CallbackFrame>(), 136);
    assert_eq!(std::mem::offset_of!(CallbackFrame, stack), 128);
    let before = stats();
    let mut contexts = (0..1031)
        .map(|identity| MixedContext { identity, calls: 0 })
        .collect::<Vec<_>>();
    let entries = contexts
        .iter_mut()
        .map(|context| {
            // SAFETY: Vec never moves its elements until all callbacks close.
            unsafe { FrameCallback::prepare(mixed_handler, std::ptr::from_mut(context).cast()) }
        })
        .collect::<Vec<_>>();
    let mut addresses = std::collections::BTreeSet::new();
    for (index, entry) in entries.iter().enumerate() {
        assert!(addresses.insert(entry.entry() as usize));
        for _ in 0..3 {
            assert_eq!(
                // SAFETY: the preparing thread owns the live exact mixed signature.
                unsafe { call_mixed(entry.entry()) },
                MIXED_RESULT ^ index as u64
            );
        }
        assert!(entry.close());
        assert!(entry.settled());
        assert_eq!(contexts[index].calls, 3);
    }
    std::thread::scope(|scope| {
        let workers = (0..8)
            .map(|worker| {
                scope.spawn(move || {
                    let mut addresses = Vec::new();
                    for identity in (worker * 129)..((worker + 1) * 129) {
                        let mut context = MixedContext { identity, calls: 0 };
                        // SAFETY: context is pinned on this physical Worker stack.
                        let entry = unsafe {
                            FrameCallback::prepare(
                                mixed_handler,
                                std::ptr::from_mut(&mut context).cast(),
                            )
                        };
                        addresses.push(entry.entry() as usize);
                        assert_eq!(
                            // SAFETY: same preparing Worker and exact native signature.
                            unsafe { call_mixed(entry.entry()) },
                            MIXED_RESULT ^ identity
                        );
                        assert!(entry.close());
                        assert_eq!(context.calls, 1);
                    }
                    addresses
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            for address in worker.join().unwrap() {
                assert!(addresses.insert(address));
            }
        }
    });
    let after = stats();
    assert_eq!(after.entries - before.entries, 2063);
    assert_eq!(after.invocations - before.invocations, 1031 * 3 + 8 * 129);
    assert_eq!(after.live_connections, before.live_connections);
    assert_eq!(after.in_flight, 0);
}

#[test]
fn generic_decoder_retains_noncanonical_bool_for_receiver_error_gate() {
    let frame = CallbackFrame {
        integers: [2, 0xffff_ffff_0000_00fe, 0, 0, 0, 0, 0, 0],
        floats: [0; 8],
        stack: std::ptr::null(),
    };
    let mut cursor = CallbackAbiCursor::default();
    assert_eq!(
        // SAFETY: synthetic register-only frame models raw C Bool then signed i8.
        unsafe { frame.word(&mut cursor, Some(ScalarKind::Bool)) },
        2
    );
    assert_eq!(
        // SAFETY: narrow signed register upper bits do not determine the value.
        unsafe { frame.word(&mut cursor, Some(ScalarKind::I8)) },
        (-2_i64) as u64
    );
}

unsafe extern "C" fn handler(raw: *mut c_void, input: *const u8) -> u64 {
    // SAFETY: the fixture pins an identity and supplies four readable bytes.
    let identity = unsafe { *(raw as *const u64) };
    // SAFETY: the native fixture's exact argument contract is Bytes(4).
    let input = unsafe { std::slice::from_raw_parts(input, 4) };
    assert_eq!(input, [0, 255, 7, 0]);
    identity + 4
}

#[test]
fn proven_control_retirement_is_once_and_keeps_entry_addresses_unique() {
    let _gate = CALLBACK_TEST_GATE.lock().unwrap();
    let before = stats();
    let mut addresses = std::collections::BTreeSet::new();
    let mut identity = 20_u64;
    for _ in 0..64 {
        // SAFETY: pinned exact context/input, confined to this physical Origin.
        let mut callback = unsafe {
            PointerU64Callback::prepare(handler, std::ptr::from_mut(&mut identity).cast())
        };
        assert!(addresses.insert(callback.entry() as usize));
        // SAFETY: no capability escapes this direct invocation.
        assert_eq!(unsafe { callback.entry()(MIXED_BYTES.as_ptr()) }, 24);
        assert!(callback.close());
        // SAFETY: the direct call returned, no template/receipt/future reader.
        assert!(unsafe { callback.retire() });
        // SAFETY: the once handle is now empty, a completed no-op.
        assert!(unsafe { callback.retire() });
        assert!(callback.close() && callback.settled());
        drop(callback);
    }
    let after = stats();
    assert_eq!(after.retired_entries - before.retired_entries, 64);
    assert_eq!(
        after.retired_slot_bytes - before.retired_slot_bytes,
        64 * 32
    );
    assert!(after.released_control_bytes > before.released_control_bytes);
    assert_eq!(after.tombstone_bytes, before.tombstone_bytes);
    assert_eq!(after.live_connections, before.live_connections);
    assert_eq!(
        after.control_bytes - before.control_bytes,
        (after.blocks - before.blocks) * 16384
    );
}

#[test]
fn exact_pointer_entry_grows_retains_identity_and_settles_on_caller_threads() {
    let _gate = CALLBACK_TEST_GATE.lock().unwrap();
    let before = stats();
    let mut identities = (0..1031_u64).collect::<Vec<_>>();
    let entries = identities
        .iter_mut()
        .map(|identity| {
            // SAFETY: identities never move until all entries close; handler has
            // the exact one-pointer-to-u64 ABI and never unwinds on valid input.
            unsafe { PointerU64Callback::prepare(handler, std::ptr::from_mut(identity).cast()) }
        })
        .collect::<Vec<_>>();
    let pointers = entries
        .iter()
        .map(|entry| entry.entry() as usize)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(pointers.len(), 1031);
    let input = [0, 255, 7, 0];
    for _ in 0..3 {
        for (index, entry) in entries.iter().enumerate() {
            // SAFETY: open entry, preparing thread and exact four-byte input.
            assert_eq!(unsafe { entry.entry()(input.as_ptr()) }, index as u64 + 4);
        }
    }
    for entry in &entries {
        assert!(entry.close());
        assert!(entry.close());
        assert!(entry.settled());
    }
    drop(entries);
    std::thread::scope(|scope| {
        for index in 0..8_u64 {
            scope.spawn(move || {
                let mut identity = index;
                // SAFETY: Worker owns its pinned identity and the input contract.
                let callback = unsafe {
                    PointerU64Callback::prepare(handler, std::ptr::from_mut(&mut identity).cast())
                };
                // SAFETY: this is the same physical Worker thread as preparation.
                assert_eq!(unsafe { callback.entry()(input.as_ptr()) }, index + 4);
                assert!(callback.close());
            });
        }
    });
    let after = stats();
    assert_eq!(after.entries - before.entries, 1039);
    assert!(after.blocks - before.blocks >= 2);
    assert_eq!(after.virtual_bytes, after.blocks * 32768);
    assert!(after.control_bytes > after.blocks * 16384);
    assert_eq!(after.live_connections, before.live_connections);
    assert_eq!(after.in_flight, 0);
    assert_eq!(after.invocations - before.invocations, 1031 * 3 + 8);
}

#[test]
fn length_entries_copy_into_real_main_and_worker_heaps_before_body_and_gc() {
    use rinzu_runtime::callback_entry::{LengthCallback, LengthKind};
    use rinzu_runtime::data::{ManagedData, ManagedRoots};
    use rinzu_runtime::heap::{CollectionKind, DomainHeap};
    let _gate = CALLBACK_TEST_GATE.lock().unwrap();
    let before = stats();
    let run = |domain| {
        let mut heap = DomainHeap::new(domain);
        let mut data = ManagedData::new(domain);
        let mut roots = ManagedRoots::default();
        let caller = data.create_string(&mut heap, "caller\0root").unwrap();
        roots.enter_frame();
        roots.publish([caller]).unwrap();
        for (kind, input, length) in [
            (LengthKind::Bytes(0), &b""[..], 0),
            (LengthKind::Bytes(4), &b"\0\xff\x07\0"[..], 4),
            (LengthKind::Utf8, &b"A\0ignored"[..], 1),
            (LengthKind::Utf8, "日本語😀\0".as_bytes(), 13),
        ] {
            let callback = LengthCallback::prepare(&mut heap, &mut data, &mut roots, kind);
            for _ in 0..32 {
                assert_eq!(
                    // SAFETY: exact readable input; Bytes(0) accepts NULL without
                    // reading or shortcutting managed copy/body preparation.
                    unsafe {
                        callback.entry()(if length == 0 {
                            std::ptr::null()
                        } else {
                            input.as_ptr()
                        })
                    },
                    length
                );
            }
            let evidence = callback.evidence();
            assert_eq!(evidence.prepared_inputs, 32);
            assert_eq!(evidence.body_entries, 32);
            assert_eq!(evidence.copied_bytes, length as usize * 32);
            assert_eq!(evidence.collections, 64);
            assert!(evidence.moved_objects >= 32);
            drop(callback);
            assert_eq!(data.string(&heap, caller).unwrap(), "caller\0root");
            assert_eq!(heap.object_count(), 1);
            assert_eq!(data.root_count(), 1);
            assert_eq!(roots.frame_count(), 1);
        }
        roots.leave_frame().unwrap();
        data.retain_roots(&mut heap, roots.live()).unwrap();
        heap.collect(CollectionKind::Full).unwrap();
        assert_eq!(heap.object_count(), 0);
        assert_eq!(data.root_count(), 0);
    };
    run(100);
    std::thread::scope(|scope| {
        for domain in 101..109 {
            scope.spawn(move || run(domain));
        }
    });
    assert_eq!(stats().live_connections, before.live_connections);
    assert_eq!(stats().in_flight, 0);
}

#[test]
fn production_entry_passes_signed_aot_sanitizers_and_fatal_subprocess_gates() {
    run_c_fixture(
        "callback_entry",
        "PASS runtime callback entries=2064 blocks=5 connections=0 in_flight=0",
        &[],
    );
}

#[test]
fn production_generic_frame_passes_signed_aot_sanitizers_and_fatal_subprocess_gates() {
    run_c_fixture(
        "callback_frame",
        "PASS runtime generic callback entries=2077 blocks=5 connections=0 in_flight=0",
        &[
            ("null-handler", 78),
            ("null-context", 78),
            ("bad-kind", 86),
            ("null-stack", 86),
            ("stack-overflow", 71),
        ],
    );
}

fn run_c_fixture(fixture: &str, marker: &str, extra_faults: &[(&str, i32)]) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let directory =
        std::env::temp_dir().join(format!("rinzu-runtime-{fixture}-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    for sanitize in [false, true] {
        let binary = directory.join(if sanitize { "sanitized" } else { "signed" });
        let mut command = Command::new("clang");
        command
            .args([
                "-std=c11",
                "-O2",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-DRINZU_CALLBACK_TESTING",
                "-pthread",
            ])
            .arg("-I")
            .arg(root.join("../../include"))
            .arg(root.join(format!("tests/fixtures/{fixture}.c")))
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
            let output = Command::new("codesign")
                .args(["-d", "--entitlements", "-"])
                .arg(&binary)
                .output()
                .unwrap();
            assert!(output.status.success());
            assert!(output.stdout.is_empty(), "unexpected entitlements");
        }
        for mode in ["normal", "merged-control"] {
            let output = Command::new(&binary)
                .arg(mode)
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{mode}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                String::from_utf8_lossy(&output.stdout).contains(marker),
                "{fixture}/{mode}: {}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
        for (mode, status) in [
            ("closed", 86),
            ("wrong-thread", 86),
            ("prepared", 86),
            ("prepared-close", 86),
            ("wrong-thread-open", 86),
            ("null-open", 86),
            ("double-open", 86),
            ("reopen", 86),
            ("oom-control", 71),
            ("oom-block", 71),
            ("resource", 71),
            ("permission", 78),
        ]
        .iter()
        .chain(extra_faults)
        {
            let output = Command::new(&binary)
                .arg(mode)
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(*status),
                "{mode}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.stdout.is_empty(), "{mode} returned from fatal gate");
            assert!(
                output.stderr.is_empty(),
                "{mode} emitted a fatal diagnostic"
            );
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}
