#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "common/native_probe.rs"]
mod native_probe;
use rinzu_runtime::data::{
    ManagedData, ManagedSlot, NativeRecordBuffer, NativeRecordError, NativeRecordField,
    NativeRecordLayout,
};
use rinzu_runtime::heap::{CollectionKind, DomainHeap};
use rinzu_runtime::scalar::{ScalarKind, ScalarValue};
use rinzu_runtime::value::{ValueSchema, ValueType};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;
thread_local! { static BUDGET: Cell<isize> = const { Cell::new(-1) }; }
struct ControlledAllocator;
fn allowed() -> bool {
    BUDGET
        .try_with(|budget| match budget.get() {
            -1 => true,
            0 => false,
            count => {
                budget.set(count - 1);
                true
            }
        })
        .unwrap_or(true)
}
// SAFETY: System retains all allocator contracts; only this test thread's
// explicit budget injects failure, leaving concurrent Owner threads unaffected.
unsafe impl GlobalAlloc for ControlledAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !allowed() {
            return std::ptr::null_mut();
        }
        // SAFETY: delegate the supplied GlobalAlloc layout unchanged.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: this pointer was allocated by System with this exact layout.
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if !allowed() {
            return std::ptr::null_mut();
        }
        // SAFETY: retain System's pointer/layout/reallocation contract.
        unsafe { System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: ControlledAllocator = ControlledAllocator;

fn schema() -> Arc<ValueSchema> {
    let mut types = (0..11)
        .map(|kind| ValueType::Scalar(ScalarKind::from_tag(kind).unwrap()))
        .collect::<Vec<_>>();
    types.push(ValueType::Record {
        identity: "Record".into(),
        fields: (0..11).collect(),
    });
    Arc::new(ValueSchema::new(types).unwrap())
}
fn values() -> Vec<ScalarValue> {
    [
        0xffff_ffff_ffff_ff80,
        0xffff_ffff_ffff_8000,
        0xffff_ffff_8000_0000,
        0x8000_0000_0000_0000,
        255,
        65535,
        u32::MAX as u64,
        u64::MAX,
        0x8000_0000,
        0x7ff8_0000_0000_002a,
        1,
    ]
    .into_iter()
    .enumerate()
    .map(|(kind, bits)| {
        ScalarValue::from_bits(ScalarKind::from_tag(kind as u32).unwrap(), bits).unwrap()
    })
    .collect()
}

#[test]
fn invalid_layouts_and_nominal_scalar_schema_are_rejected() {
    let field = |offset| NativeRecordField {
        kind: ScalarKind::I32,
        offset,
    };
    for (size, align, fields) in [
        (0, 4, vec![field(0)]),
        (4, 3, vec![field(0)]),
        (4, 4, vec![]),
        (4, 4, vec![field(1)]),
        (4, 4, vec![field(4)]),
        (8, 4, vec![field(0), field(0)]),
        (4, 1, vec![field(0)]),
        (usize::MAX, 8, vec![field(0)]),
    ] {
        assert_eq!(
            NativeRecordLayout::new(11, size, align, fields),
            Err(NativeRecordError::InvalidLayout)
        );
    }
    let layout = NativeRecordLayout::new(11, 4, 4, vec![field(0)]).unwrap();
    assert_eq!(
        layout.validate_schema(&schema()),
        Err(NativeRecordError::InvalidLayout)
    );
    assert!(NativeRecordBuffer::new(&layout, &values()).is_err());
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn binary(sanitize: bool) -> std::path::PathBuf {
    use std::path::Path;
    use std::process::Command;
    use std::sync::OnceLock;
    static NORMAL: OnceLock<std::path::PathBuf> = OnceLock::new();
    static SANITIZED: OnceLock<std::path::PathBuf> = OnceLock::new();
    let slot = if sanitize { &SANITIZED } else { &NORMAL };
    slot.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let directory =
            std::env::temp_dir().join(format!("rinzu-native-record-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join(if sanitize { "sanitized" } else { "signed" });
        let mut command = Command::new("clang");
        command
            .args([
                "--target=aarch64-apple-darwin",
                "-std=c11",
                "-O2",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-pthread",
            ])
            .arg("-I")
            .arg(root.join("../../include"))
            .arg(root.join("tests/fixtures/native_record.c"));
        if sanitize {
            command.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
        }
        let output = command.arg("-o").arg(&path).output().unwrap();
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
                    .arg(&path)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
        path
    })
    .clone()
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn layout() -> NativeRecordLayout {
    let output = std::process::Command::new(binary(false))
        .arg("layout")
        .output()
        .unwrap();
    assert!(output.status.success());
    let fields = String::from_utf8(output.stdout)
        .unwrap()
        .split_whitespace()
        .map(|word| word.parse::<usize>().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(fields.len(), 13);
    NativeRecordLayout::new(
        11,
        fields[0],
        fields[1],
        fields[2..]
            .iter()
            .enumerate()
            .map(|(kind, offset)| NativeRecordField {
                kind: ScalarKind::from_tag(kind as u32).unwrap(),
                offset: *offset,
            })
            .collect(),
    )
    .unwrap()
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn run(layout: &NativeRecordLayout, domain: u64) {
    let mut heap = DomainHeap::new(domain);
    let mut data = ManagedData::new(domain);
    data.configure_values(&heap, schema()).unwrap();
    layout.validate_schema(&schema()).unwrap();
    let expected = values();
    let mut copy = NativeRecordBuffer::new(layout, &expected).unwrap();
    assert_eq!(copy.as_ptr() as usize % layout.alignment(), 0);
    // SAFETY: the buffer owns this initialized target-sized allocation.
    let bytes = unsafe { std::slice::from_raw_parts(copy.as_ptr(), layout.size()) };
    for (index, byte) in bytes.iter().enumerate() {
        if !layout.fields().iter().any(|field| {
            (field.offset..field.offset + field.kind.width() as usize / 8).contains(&index)
        }) {
            assert_eq!(*byte, 0, "padding {index}");
        }
    }
    // SAFETY: this aligned native copy remains readable/stable during reception.
    let original = unsafe { data.receive_native_record(&mut heap, layout, copy.as_ptr()) }.unwrap();
    let mut independent = data
        .copy_native_record(&mut heap, layout, original)
        .unwrap();
    let address = independent.as_ptr();
    // SAFETY: mutating only native-owned copies cannot update the received record.
    unsafe {
        std::ptr::write_bytes(copy.as_mut_ptr(), 0xa5, layout.size());
    }
    let objects = heap.object_count();
    BUDGET.with(|budget| budget.set(0));
    // SAFETY: NULL has an explicit recoverable record-reception rule.
    let null = unsafe { data.receive_native_record(&mut heap, layout, std::ptr::null()) };
    BUDGET.with(|budget| budget.set(-1));
    assert_eq!(null, Err(NativeRecordError::Null));
    let bool_offset = layout.fields()[10].offset;
    // SAFETY: inject a readable invalid CBool byte without creating an invalid bool value.
    unsafe {
        independent.as_mut_ptr().add(bool_offset).write(2);
    }
    assert_eq!(
        // SAFETY: readable owned bytes, including an intentionally invalid Bool encoding.
        unsafe { data.receive_native_record(&mut heap, layout, independent.as_ptr()) },
        Err(NativeRecordError::Conversion)
    );
    assert_eq!(heap.object_count(), objects);
    // SAFETY: restore the valid byte, then vary only native padding.
    unsafe {
        independent.as_mut_ptr().add(bool_offset).write(1);
        for index in 0..layout.size() {
            if !layout.fields().iter().any(|field| {
                (field.offset..field.offset + field.kind.width() as usize / 8).contains(&index)
            }) {
                independent.as_mut_ptr().add(index).write(0x5a);
            }
        }
    }
    // SAFETY: all fields are valid initialized bytes; padding is not read.
    let other =
        unsafe { data.receive_native_record(&mut heap, layout, independent.as_ptr()) }.unwrap();
    assert!(
        !data
            .values_equal(
                &heap,
                layout.ty,
                ManagedSlot::Reference(original),
                ManagedSlot::Reference(other)
            )
            .unwrap()
    ); // NaN retains the language's unequal-to-itself rule.
    let mut finite = values();
    finite[9] = ScalarValue::from_f64(1.0);
    let mut finite_copy = NativeRecordBuffer::new(layout, &finite).unwrap();
    // SAFETY: owned readable struct; only padding changes between the two inputs.
    let finite_a =
        unsafe { data.receive_native_record(&mut heap, layout, finite_copy.as_ptr()) }.unwrap();
    let padding = (0..layout.size())
        .find(|index| {
            !layout.fields().iter().any(|field| {
                (field.offset..field.offset + field.kind.width() as usize / 8).contains(index)
            })
        })
        .unwrap();
    // SAFETY: this target-reported padding byte belongs to the owned buffer.
    unsafe {
        finite_copy.as_mut_ptr().add(padding).write(0x3c);
    }
    // SAFETY: initialized stable native copy, independent from both managed values.
    let finite_b =
        unsafe { data.receive_native_record(&mut heap, layout, finite_copy.as_ptr()) }.unwrap();
    assert!(
        data.values_equal(
            &heap,
            layout.ty,
            ManagedSlot::Reference(finite_a),
            ManagedSlot::Reference(finite_b)
        )
        .unwrap()
    );
    for _ in 0..32 {
        data.retain_roots(&mut heap, [original, other]).unwrap();
        heap.collect(CollectionKind::Full).unwrap();
        assert_eq!(independent.as_ptr(), address);
        for (index, value) in expected.iter().enumerate() {
            let ManagedSlot::Scalar(actual) = data
                .project(&mut heap, original, layout.ty, None, index)
                .unwrap()
            else {
                panic!("scalar field")
            };
            assert_eq!(actual.kind(), value.kind());
            assert_eq!(actual.bits(), value.bits());
        }
    }
    data.release_all(&mut heap).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), 0);
    assert_eq!(data.root_count(), 0);
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn target_clang_layout_matches_native_bits_padding_owned_copy_and_moving_gc() {
    let layout = Arc::new(layout());
    let copy = NativeRecordBuffer::new(&layout, &values()).unwrap();
    let golden = std::process::Command::new(binary(false))
        .arg("golden")
        .output()
        .unwrap();
    assert!(golden.status.success());
    // SAFETY: initialized allocation, used to compare with the actual Clang struct.
    let bytes = unsafe { std::slice::from_raw_parts(copy.as_ptr(), layout.size()) };
    let hex = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(hex, String::from_utf8(golden.stdout).unwrap().trim());
    run(&layout, 1);
    std::thread::scope(|scope| {
        for domain in 2..10 {
            let layout = &layout;
            scope.spawn(move || run(layout, domain));
        }
    });
    // IEEE encodings, including quiet NaN payload, infinity and signed zero, survive copy.
    for (f32, f64) in [
        (0x7fc0_0042, 0xfff0_0000_0000_0000),
        (0, 0x8000_0000_0000_0000),
    ] {
        let mut values = values();
        values[8] = ScalarValue::from_bits(ScalarKind::F32, f32).unwrap();
        values[9] = ScalarValue::from_bits(ScalarKind::F64, f64).unwrap();
        let copy = NativeRecordBuffer::new(&layout, &values).unwrap();
        // SAFETY: initialized target-aligned native buffer with matching fields.
        let restored = unsafe { layout.receive(copy.as_ptr()) }.unwrap();
        for (expected, actual) in values.iter().zip(restored) {
            assert_eq!(expected.bits(), actual.bits());
        }
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn c_record_signed_aot_sanitizers_and_allocation_native_faults() {
    use std::os::unix::process::ExitStatusExt;
    use std::process::Command;
    for sanitize in [false, true] {
        let path = binary(sanitize);
        let output = native_probe::run(&mut Command::new(&path), false);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8(output.stdout)
                .unwrap()
                .starts_with("PASS record")
        );
        for mode in [
            "copy-oom",
            "receive-oom-0",
            "receive-oom-1",
            "receive-oom-2",
            "wrong-type",
            "read",
        ] {
            let output =
                native_probe::run(Command::new(&path).arg(mode), sanitize && mode == "read");
            assert!(!output.status.success(), "{mode}");
            if mode == "read" {
                if sanitize {
                    assert!(String::from_utf8_lossy(&output.stderr).contains("AddressSanitizer"));
                } else {
                    assert!(matches!(output.status.signal(), Some(10 | 11)));
                }
            } else {
                assert_eq!(
                    output.status.code(),
                    Some(if mode == "wrong-type" { 86 } else { 71 }),
                    "{mode}: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            assert!(!String::from_utf8_lossy(&output.stdout).contains("UNEXPECTED_RETURN"));
        }
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn native_record_fatal_child() {
    let Ok(mode) = std::env::var("RINZU_RECORD_FAULT") else {
        return;
    };
    let layout = layout();
    let values = values();
    if mode == "copy-oom" {
        BUDGET.with(|budget| budget.set(0));
        let _ = NativeRecordBuffer::new(&layout, &values);
    } else if mode == "receive-oom" {
        let copy = NativeRecordBuffer::new(&layout, &values).unwrap();
        BUDGET.with(|budget| budget.set(0));
        // SAFETY: valid readable buffer; the allocator fault happens before reading it.
        let _ = unsafe { layout.receive(copy.as_ptr()) };
    } else {
        // SAFETY: this subprocess reads a private unreadable page to verify the
        // native fault boundary without touching system/unrelated memory.
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
        // SAFETY: intentional native fault from this subprocess's private page.
        let _ = unsafe { layout.receive(page.cast()) };
    }
    panic!("UNEXPECTED_RETURN");
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn rust_record_copy_oom_and_native_fault_do_not_publish_a_managed_error() {
    use std::os::unix::process::ExitStatusExt;
    use std::process::Command;
    for mode in ["copy-oom", "receive-oom", "read"] {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "native_record_fatal_child"])
            .env("RINZU_RECORD_FAULT", mode)
            .output()
            .unwrap();
        if mode == "read" {
            assert!(matches!(output.status.signal(), Some(10 | 11)));
        } else {
            assert_eq!(output.status.code(), Some(71), "{mode}");
        }
        assert!(!String::from_utf8_lossy(&output.stderr).contains("UNEXPECTED_RETURN"));
    }
}
