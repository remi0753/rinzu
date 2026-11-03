use rinzu_runtime::data::{
    ManagedData, ManagedHandle, ManagedRoots, ManagedSlot, NativeUserDataError,
    NativeUserDataRegistry, userdata_process_stats,
};
use rinzu_runtime::heap::{CollectionKind, DomainHeap};
use rinzu_runtime::scalar::{ScalarKind, ScalarValue};
use rinzu_runtime::value::{ValueSchema, ValueType};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;

thread_local! {static BUDGET:Cell<isize>=const {Cell::new(-1)};}
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
// SAFETY: the System allocation contract is unchanged; only a private thread-local
// budget produces a failed allocation, without exhausting host memory.
unsafe impl GlobalAlloc for ControlledAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !allowed() {
            return std::ptr::null_mut();
        }
        // SAFETY: delegate the exact allocator layout to System.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: System allocated this pointer using the same layout.
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

fn configured(domain: u64) -> (DomainHeap, ManagedData, ManagedRoots) {
    let heap = DomainHeap::new(domain);
    let mut data = ManagedData::new(domain);
    let schema = ValueSchema::new(vec![
        ValueType::String,
        ValueType::Scalar(ScalarKind::I64),
        ValueType::Record {
            identity: "State".into(),
            fields: vec![0, 1],
        },
        ValueType::Enum {
            identity: "Event".into(),
            variants: vec![vec![], vec![2]],
        },
        ValueType::Record {
            identity: "Different".into(),
            fields: vec![0, 1],
        },
        ValueType::Other,
        ValueType::Record {
            identity: "Context".into(),
            fields: vec![5],
        },
    ])
    .unwrap();
    assert!(!schema.has_equality(6).unwrap());
    data.configure_values(&heap, Arc::new(schema)).unwrap();
    let mut roots = ManagedRoots::default();
    roots.enter_frame();
    (heap, data, roots)
}
fn record(
    heap: &mut DomainHeap,
    data: &mut ManagedData,
    name: &str,
    value: i128,
    ty: usize,
) -> ManagedHandle {
    let text = data.create_string(heap, name).unwrap();
    data.create_composite(
        heap,
        ty,
        None,
        &[
            ManagedSlot::Reference(text),
            ManagedSlot::Scalar(ScalarValue::from_integer(ScalarKind::I64, value).unwrap()),
        ],
    )
    .unwrap()
}
fn verify_record(
    heap: &mut DomainHeap,
    data: &mut ManagedData,
    value: ManagedHandle,
    name: &str,
    number: i128,
) {
    let ManagedSlot::Reference(text) = data.project(heap, value, 2, None, 0).unwrap() else {
        panic!("not a String");
    };
    assert_eq!(data.string(heap, text).unwrap(), name);
    assert_eq!(
        data.project(heap, value, 2, None, 1).unwrap(),
        ManagedSlot::Scalar(ScalarValue::from_integer(ScalarKind::I64, number).unwrap())
    );
}
fn collect(heap: &mut DomainHeap, data: &mut ManagedData, roots: &ManagedRoots) {
    data.retain_roots(heap, roots.live()).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
}
fn owner_run(domain: u64) {
    let (mut heap, mut data, mut roots) = configured(domain);
    let mut registry = NativeUserDataRegistry::new(&heap);
    let first = record(&mut heap, &mut data, "first🙂", 41, 2);
    let second = record(&mut heap, &mut data, "second\0value", 42, 2);
    let other = record(&mut heap, &mut data, "typed", 43, 4);
    let event = data
        .create_composite(&mut heap, 3, Some(1), &[ManagedSlot::Reference(first)])
        .unwrap();
    let scope = registry.enter_scope(&heap, &data);
    let one = registry.prepare(&heap, &data, &mut roots, scope, 2, first);
    let duplicate = registry.prepare(&heap, &data, &mut roots, scope, 2, first);
    let two = registry.prepare(&heap, &data, &mut roots, scope, 2, second);
    let typed = registry.prepare(&heap, &data, &mut roots, scope, 4, other);
    let enumeration = registry.prepare(&heap, &data, &mut roots, scope, 3, event);
    let context = data
        .create_composite(&mut heap, 6, None, &[ManagedSlot::Opaque(42)])
        .unwrap();
    let held = registry.prepare(&heap, &data, &mut roots, scope, 6, context);
    let active = userdata_process_stats();
    assert!(active.tokens >= 6 && active.token_bytes > 0);
    assert_eq!((active.closed, active.registry_capacity), (0, 0));
    assert_ne!(one, duplicate);
    assert_ne!(one, two);
    assert_ne!(two, typed);
    assert_ne!(one.bits(), first.bits());
    registry.open(&heap, scope);
    let admitted = registry.admit(&heap, scope);
    let nested = registry.admit(&heap, scope);
    for _ in 0..32 {
        collect(&mut heap, &mut data, &roots);
        assert_eq!(
            registry
                .resolve(&heap, &data, admitted, 6, held.pointer())
                .unwrap(),
            context
        );
        assert_eq!(
            data.project(&mut heap, context, 6, None, 0).unwrap(),
            ManagedSlot::Opaque(42)
        );
        // Native may exchange same-type tokens: resolve the actual returned
        // pointer, independent of parameter position or callback identity.
        let swapped = registry
            .resolve(&heap, &data, admitted, 2, two.pointer())
            .unwrap();
        let restored = registry
            .resolve(&heap, &data, nested, 2, one.pointer())
            .unwrap();
        verify_record(&mut heap, &mut data, swapped, "second\0value", 42);
        verify_record(&mut heap, &mut data, restored, "first🙂", 41);
        assert_eq!(
            registry
                .resolve(&heap, &data, admitted, 4, typed.pointer())
                .unwrap(),
            other
        );
        assert_eq!(
            registry
                .resolve(&heap, &data, admitted, 3, enumeration.pointer())
                .unwrap(),
            event
        );
    }
    BUDGET.with(|budget| budget.set(0));
    let null = registry.resolve(&heap, &data, admitted, 2, std::ptr::null());
    let resolved = registry.resolve(&heap, &data, admitted, 2, two.pointer());
    BUDGET.with(|budget| budget.set(-1));
    assert_eq!(null, Err(NativeUserDataError::Null));
    assert_eq!(resolved, Ok(second));
    registry.returned(&heap, &mut roots, scope);
    assert_eq!(registry.evidence().in_flight, 2);
    assert_eq!(registry.evidence().roots, 6);
    collect(&mut heap, &mut data, &roots);
    assert_eq!(
        registry
            .resolve(&heap, &data, admitted, 2, one.pointer())
            .unwrap(),
        first
    );
    registry.settle(&heap, &mut roots, nested);
    assert_eq!(registry.evidence().roots, 6);
    BUDGET.with(|budget| budget.set(0));
    registry.settle(&heap, &mut roots, admitted);
    BUDGET.with(|budget| budget.set(-1));
    assert!(registry.finish(&heap).is_baseline());
    assert_eq!(roots.lease_count(), 0);
    // Each call releases its lookup entries while token numbers are never reused.
    let keep = record(&mut heap, &mut data, "growing", 99, 2);
    roots.publish([keep]).unwrap();
    let mut addresses = std::collections::HashSet::new();
    for _ in 0..2049 {
        let next = registry.enter_scope(&heap, &data);
        let token = registry.prepare(&heap, &data, &mut roots, next, 2, keep);
        assert!(addresses.insert(token.bits()));
        assert_ne!(token, one);
        registry.open(&heap, next);
        let invocation = registry.admit(&heap, next);
        assert_eq!(
            registry
                .resolve(&heap, &data, invocation, 2, token.pointer())
                .unwrap(),
            keep
        );
        registry.settle(&heap, &mut roots, invocation);
        registry.returned(&heap, &mut roots, next);
    }
    let prepared = registry.enter_scope(&heap, &data);
    let discarded = registry.prepare(&heap, &data, &mut roots, prepared, 2, keep);
    assert_ne!(discarded, one);
    registry.discard_prepared(&heap, &mut roots, prepared);
    let baseline = registry.finish(&heap);
    assert_eq!(baseline.created, 2056);
    assert_eq!(baseline.created, baseline.released);
    for mode in 0..3 {
        let scope = registry.enter_scope(&heap, &data);
        registry.expect_result(&heap, &data, scope, 2);
        let _one = registry.prepare(&heap, &data, &mut roots, scope, 2, first);
        let two = registry.prepare(&heap, &data, &mut roots, scope, 2, second);
        roots.publish([]).unwrap();
        registry.open(&heap, scope);
        let invocation = registry.admit(&heap, scope);
        registry.returned(&heap, &mut roots, scope);
        registry.settle(&heap, &mut roots, invocation);
        assert_eq!(registry.evidence().roots, 2);
        collect(&mut heap, &mut data, &roots);
        if mode == 0 {
            let result = registry
                .receive_result(&heap, &data, scope, 2, two.pointer())
                .unwrap();
            assert_eq!(result, second);
            verify_record(&mut heap, &mut data, result, "second\0value", 42);
        } else if mode == 1 {
            assert_eq!(
                registry.receive_result(&heap, &data, scope, 2, std::ptr::null()),
                Err(NativeUserDataError::Null)
            );
        }
        // Transfer the received value to ordinary Rinzu ownership before
        // finishing; cancellation may instead discard the unread raw result.
        roots.publish([first, second]).unwrap();
        registry.finish_result(&heap, &mut roots, scope);
        assert!(registry.finish(&heap).is_baseline());
        assert_eq!(roots.lease_count(), 0);
        collect(&mut heap, &mut data, &roots);
        verify_record(&mut heap, &mut data, second, "second\0value", 42);
    }
    roots.leave_frame().unwrap();
    collect(&mut heap, &mut data, &roots);
    assert_eq!(heap.object_count(), 0);
    assert_eq!(data.root_count(), 0);
}

#[test]
fn userdata_actual_tokens_keep_moving_values_until_return_and_settlement_on_owners() {
    let before = userdata_process_stats();
    owner_run(600);
    let workers = (0..8)
        .map(|index| std::thread::spawn(move || owner_run(601 + index)))
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().unwrap();
    }
    let after = userdata_process_stats();
    assert_eq!(after, before);
    assert_eq!(after.tokens, 0);
    assert_eq!(after.registry_capacity, 0);
}

#[test]
fn userdata_fatal_probe() {
    let Ok(mode) = std::env::var("RINZU_USERDATA_PROBE") else {
        return;
    };
    let (mut heap, mut data, mut roots) = configured(701);
    let mut registry = NativeUserDataRegistry::new(&heap);
    if mode == "scope-oom" {
        BUDGET.with(|budget| budget.set(0));
        registry.enter_scope(&heap, &data);
        panic!("scope OOM returned");
    }
    let value = record(&mut heap, &mut data, "probe", 7, 2);
    let scope = registry.enter_scope(&heap, &data);
    if mode.starts_with("result-") {
        registry.expect_result(&heap, &data, scope, 2);
    }
    if let Some(budget) = mode.strip_prefix("prepare-oom-") {
        let budget = budget.parse().unwrap();
        BUDGET.with(|state| state.set(budget));
        registry.prepare(&heap, &data, &mut roots, scope, 2, value);
        panic!("prepare OOM returned");
    }
    if mode == "not-composite" {
        registry.prepare(&heap, &data, &mut roots, scope, 1, value);
        panic!("scalar mapping accepted");
    }
    let token = registry.prepare(&heap, &data, &mut roots, scope, 2, value);
    if mode == "not-open" {
        registry.admit(&heap, scope);
        panic!("pre-entry admitted");
    }
    registry.open(&heap, scope);
    if mode == "admit-oom" {
        BUDGET.with(|budget| budget.set(0));
        registry.admit(&heap, scope);
        panic!("admission OOM returned");
    }
    let invocation = registry.admit(&heap, scope);
    if mode.starts_with("result-") {
        if mode == "result-before-return" {
            let _ = registry.receive_result(&heap, &data, scope, 2, token.pointer());
        }
        if mode == "result-release-before-return" {
            registry.finish_result(&heap, &mut roots, scope);
        }
        registry.returned(&heap, &mut roots, scope);
        if mode == "result-in-flight" {
            let _ = registry.receive_result(&heap, &data, scope, 2, token.pointer());
        }
        if mode == "result-early-release" {
            registry.finish_result(&heap, &mut roots, scope);
        }
        registry.settle(&heap, &mut roots, invocation);
        match mode.as_str() {
            "result-wrong-type" => {
                let _ = registry.receive_result(&heap, &data, scope, 4, token.pointer());
            }
            "result-wrong-domain" => {
                let (other, other_data, _) = configured(702);
                let _ = registry.receive_result(&other, &other_data, scope, 2, token.pointer());
            }
            "result-wrong-roots" => {
                let mut other = ManagedRoots::default();
                registry.finish_result(&heap, &mut other, scope);
            }
            "result-wrong-call" => {
                let other = registry.enter_scope(&heap, &data);
                registry.expect_result(&heap, &data, other, 2);
                registry.prepare(&heap, &data, &mut roots, other, 2, value);
                registry.open(&heap, other);
                registry.returned(&heap, &mut roots, other);
                let _ = registry.receive_result(&heap, &data, other, 2, token.pointer());
            }
            "result-duplicate-read" => {
                registry
                    .receive_result(&heap, &data, scope, 2, token.pointer())
                    .unwrap();
                let _ = registry.receive_result(&heap, &data, scope, 2, token.pointer());
            }
            "result-missing-finish" => {
                drop(registry);
            }
            _ => panic!("invalid result mode returned"),
        }
        panic!("result violation returned");
    }
    match mode.as_str() {
        "unknown" => {
            let _ = registry.resolve(&heap, &data, invocation, 2, 0x1000usize as *const u8);
        }
        "wrong-type" => {
            let _ = registry.resolve(&heap, &data, invocation, 4, token.pointer());
        }
        "wrong-domain" => {
            let (foreign, foreign_data, _) = configured(702);
            let _ = registry.resolve(&foreign, &foreign_data, invocation, 2, token.pointer());
        }
        "null-wrong-owner" => {
            let (foreign, foreign_data, _) = configured(701);
            let _ = registry.resolve(&foreign, &foreign_data, invocation, 2, std::ptr::null());
        }
        "wrong-data" => {
            let (_, foreign_data, _) = configured(701);
            let _ = registry.resolve(&heap, &foreign_data, invocation, 2, token.pointer());
        }
        "wrong-roots" => {
            let mut foreign_roots = ManagedRoots::default();
            foreign_roots.pin(value).unwrap();
            registry.settle(&heap, &mut foreign_roots, invocation);
        }
        "null-bad-type" => {
            let _ = registry.resolve(&heap, &data, invocation, 5, std::ptr::null());
        }
        "wrong-scope" => {
            let nested = registry.enter_scope(&heap, &data);
            registry.open(&heap, nested);
            let other = registry.admit(&heap, nested);
            let _ = registry.resolve(&heap, &data, other, 2, token.pointer());
        }
        "closed" => {
            registry.settle(&heap, &mut roots, invocation);
            registry.returned(&heap, &mut roots, scope);
            let next = registry.enter_scope(&heap, &data);
            let fresh = registry.prepare(&heap, &data, &mut roots, next, 2, value);
            assert_ne!(fresh, token);
            registry.open(&heap, next);
            let other = registry.admit(&heap, next);
            let _ = registry.resolve(&heap, &data, other, 2, token.pointer());
        }
        "settled" => {
            registry.settle(&heap, &mut roots, invocation);
            let _ = registry.resolve(&heap, &data, invocation, 2, token.pointer());
        }
        "null-without-admission" => {
            registry.settle(&heap, &mut roots, invocation);
            let _ = registry.resolve(&heap, &data, invocation, 2, std::ptr::null());
        }
        "admit-after-return" => {
            registry.returned(&heap, &mut roots, scope);
            registry.admit(&heap, scope);
        }
        "double-settle" => {
            registry.settle(&heap, &mut roots, invocation);
            registry.settle(&heap, &mut roots, invocation);
        }
        "discard-open" => registry.discard_prepared(&heap, &mut roots, scope),
        "unfinished-owner" => {
            registry.finish(&heap);
        }
        _ => panic!("unknown private mode"),
    }
    panic!("interop failure returned");
}

#[test]
fn userdata_rejects_invalid_identity_type_domain_scope_and_exhaustion_before_body() {
    for mode in [
        "result-before-return",
        "result-release-before-return",
        "result-in-flight",
        "result-early-release",
        "result-wrong-type",
        "result-wrong-domain",
        "result-wrong-roots",
        "result-wrong-call",
        "result-duplicate-read",
        "result-missing-finish",
        "unknown",
        "wrong-type",
        "wrong-domain",
        "null-wrong-owner",
        "wrong-data",
        "wrong-roots",
        "null-bad-type",
        "wrong-scope",
        "closed",
        "settled",
        "null-without-admission",
        "admit-after-return",
        "double-settle",
        "discard-open",
        "unfinished-owner",
        "not-open",
        "not-composite",
        "scope-oom",
        "prepare-oom-0",
        "prepare-oom-1",
        "admit-oom",
    ] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "userdata_fatal_probe", "--nocapture"])
            .env("RINZU_USERDATA_PROBE", mode)
            .output()
            .unwrap();
        let code = if mode.contains("oom") { 71 } else { 86 };
        assert_eq!(
            output.status.code(),
            Some(code),
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("test result: ok"));
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn c_binary(sanitized: bool) -> std::path::PathBuf {
    use std::path::Path;
    use std::process::Command;
    use std::sync::OnceLock;
    static NORMAL: OnceLock<std::path::PathBuf> = OnceLock::new();
    static SANITIZED: OnceLock<std::path::PathBuf> = OnceLock::new();
    let slot = if sanitized { &SANITIZED } else { &NORMAL };
    slot.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let directory =
            std::env::temp_dir().join(format!("rinzu-c-userdata-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let binary = directory.join(if sanitized { "sanitized" } else { "signed" });
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
            .arg(root.join("tests/fixtures/native_userdata.c"));
        if sanitized {
            command.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
        }
        let output = command.arg("-o").arg(&binary).output().unwrap();
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
            let output = Command::new("codesign")
                .args(["-d", "--entitlements", "-"])
                .arg(&binary)
                .output()
                .unwrap();
            assert!(output.status.success());
            let entitlements = String::from_utf8_lossy(&output.stdout);
            assert!(
                !entitlements.contains("allow-jit")
                    && !entitlements.contains("allow-unsigned-executable-memory")
            );
        }
        binary
    })
    .clone()
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn c_userdata_signed_sanitized_growth_preserves_owner_roots_and_closed_identity() {
    for sanitized in [false, true] {
        let output = std::process::Command::new(c_binary(sanitized))
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "PASS userdata actual tokens GC Owner gate baseline process=0/0/0\n"
        );
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn c_userdata_rejects_unknown_closed_type_scope_owner_thread_and_allocation_failure() {
    for sanitized in [false, true] {
        let binary = c_binary(sanitized);
        for mode in [
            "result-before-return",
            "result-release-before-return",
            "result-in-flight",
            "result-early-release",
            "result-wrong-type",
            "result-wrong-domain",
            "result-wrong-call",
            "result-duplicate-read",
            "result-missing-finish",
            "unknown",
            "wrong-type",
            "null-bad-type",
            "wrong-domain",
            "foreign-thread",
            "wrong-scope",
            "closed",
            "settled",
            "null-without-admission",
            "admit-after-return",
            "double-settle",
            "discard-open",
            "unfinished-owner",
            "not-open",
            "not-composite",
            "scope-oom-0",
            "scope-oom-1",
            "prepare-oom-0",
            "prepare-oom-1",
            "admit-oom",
        ] {
            let output = std::process::Command::new(&binary)
                .arg(mode)
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(if mode.contains("oom") { 71 } else { 86 }),
                "{mode}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(!String::from_utf8_lossy(&output.stdout).contains("UNEXPECTED_RETURN"));
            assert!(!String::from_utf8_lossy(&output.stderr).contains("AddressSanitizer"));
        }
    }
}
