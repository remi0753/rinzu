use rinzu_runtime::data::{DataError, ManagedData, ManagedHandle};
use rinzu_runtime::failure::{
    Diagnostic, ErrorKind as E, Failure, FailureCause as C, SourcePosition,
};
use rinzu_runtime::heap::{CollectionKind, DomainHeap, FinalizerPolicy, HeapValue};

#[test]
fn strings_preserve_utf8_nul_and_padding_across_moving_collections() {
    let mut heap = DomainHeap::new(123);
    let mut data = ManagedData::new(123);
    heap.allocate(0, &[HeapValue::Integer(99)], FinalizerPolicy::None)
        .unwrap();
    let inputs = ["", "a", "12345678", "123456789", "日本語🙂\0tail"];
    let handles: Vec<_> = inputs
        .iter()
        .map(|value| data.create_string(&mut heap, value).unwrap())
        .collect();
    for _ in 0..3 {
        let report = heap.collect(CollectionKind::Full).unwrap();
        assert!(report.moved_objects > 0);
        for (handle, expected) in handles.iter().zip(inputs) {
            assert_eq!(data.string(&heap, *handle).unwrap(), expected);
            assert_eq!(data.string_length(&heap, *handle).unwrap(), expected.len());
            let duplicate = data.create_string(&mut heap, expected).unwrap();
            assert_ne!(*handle, duplicate);
            assert!(data.strings_equal(&heap, *handle, duplicate).unwrap());
            assert_eq!(
                data.string_hash(&heap, *handle).unwrap(),
                rinzu_runtime::key::string_hash(expected)
            );
            assert_eq!(
                data.string_hash(&heap, *handle).unwrap(),
                data.string_hash(&heap, duplicate).unwrap()
            );
            data.release(&mut heap, duplicate).unwrap();
            assert_eq!(ManagedHandle::from_bits(handle.bits()), Some(*handle));
        }
    }
    data.release_all(&mut heap).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), 0);
    assert_eq!(data.root_count(), 0);
    assert_eq!(
        data.string(&heap, handles[0]),
        Err(DataError::UnknownHandle)
    );
    let new = data.create_string(&mut heap, "new").unwrap();
    assert!(!handles.contains(&new));
}

#[test]
fn error_payload_is_immutable_and_message_can_outlive_its_error_root() {
    let mut heap = DomainHeap::new(456);
    let mut data = ManagedData::new(456);
    let original = Failure {
        managed: Default::default(),
        traces: Default::default(),
        primary: Diagnostic {
            cause: C::Error(E::Conversion),
            source: Some(SourcePosition {
                file: u64::MAX,
                start_byte: 12,
                end_byte: 23,
            }),
        },
        cleanup: vec![
            C::Error(E::InvalidUtf8).into(),
            C::Error(E::NativeRecordBusy).into(),
            C::Error(E::SupplantBusy).into(),
        ],
    };
    let mut input = original.clone();
    let error = data.create_error(&mut heap, &input).unwrap();
    input.cleanup.clear();
    input.primary.source = None;
    assert_eq!(data.root_count(), 1);
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(data.error(&heap, error).unwrap(), original);
    let message = data.error_message(&mut heap, error).unwrap();
    let text = data.string(&heap, message).unwrap();
    assert!(text.contains("numeric conversion"));
    assert!(text.contains("cleanup: native string"));
    assert!(text.contains("cleanup: native record"));
    assert!(text.contains("cleanup: another supplant call"));
    assert_eq!(data.string(&heap, error), Err(DataError::WrongKind));
    assert_eq!(data.error(&heap, message), Err(DataError::WrongKind));
    data.release(&mut heap, error).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), 1);
    assert_eq!(data.string(&heap, message).unwrap(), text);
    data.release_all(&mut heap).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), 0);
}

#[test]
fn wrong_domain_and_nonrecoverable_diagnostics_are_rejected_before_allocation() {
    let mut heap = DomainHeap::new(1);
    let mut other = DomainHeap::new(2);
    let mut data = ManagedData::new(1);
    let text = data.create_string(&mut heap, "owner").unwrap();
    assert_eq!(data.string(&other, text), Err(DataError::WrongDomain));
    assert_eq!(
        data.create_string(&mut other, "foreign"),
        Err(DataError::WrongDomain)
    );
    assert_eq!(data.release(&mut other, text), Err(DataError::WrongDomain));
    assert_eq!(other.object_count(), 0);
    assert_eq!(
        data.create_error(&mut heap, &Failure::new(C::Cancelled)),
        Err(DataError::NotRecoverable)
    );
    let mixed = Failure {
        managed: Default::default(),
        traces: Default::default(),
        primary: C::Error(E::Conversion).into(),
        cleanup: vec![C::ArithmeticTrap.into()],
    };
    assert_eq!(
        data.create_error(&mut heap, &mixed),
        Err(DataError::NotRecoverable)
    );
    assert_eq!(heap.object_count(), 1);
    assert_eq!(data.root_count(), 1);
    assert!(ManagedHandle::from_bits(0).is_none());
    assert!(ManagedHandle::from_bits(u64::MAX).is_none());
}

#[test]
fn caller_snapshots_and_independent_leases_bound_collection_to_live_values() {
    use rinzu_runtime::data::{ManagedRootError, ManagedRoots};
    let mut heap = DomainHeap::new(789);
    let mut data = ManagedData::new(789);
    let mut roots = ManagedRoots::default();
    let caller = data.create_string(&mut heap, "caller").unwrap();
    let lease_value = data.create_string(&mut heap, "native ledger").unwrap();
    roots.enter_frame();
    roots.publish([caller]).unwrap();
    let lease1 = roots.pin(lease_value).unwrap();
    let lease2 = roots.pin(lease_value).unwrap();
    roots.enter_frame();
    for _ in 0..20 {
        let temporary = data.create_string(&mut heap, "callee temporary").unwrap();
        roots.publish([temporary]).unwrap();
        data.retain_roots(&mut heap, roots.live()).unwrap();
        heap.collect(CollectionKind::Full).unwrap();
        assert_eq!(heap.object_count(), 3);
        assert_eq!(data.string(&heap, caller).unwrap(), "caller");
    }
    roots.leave_frame().unwrap();
    roots.unpin(lease1).unwrap();
    data.retain_roots(&mut heap, roots.live()).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), 2);
    roots.unpin(lease2).unwrap();
    assert_eq!(roots.unpin(lease2), Err(ManagedRootError::UnknownLease));
    roots.publish([]).unwrap();
    data.retain_roots(&mut heap, roots.live()).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), 0);
    roots.leave_frame().unwrap();
    assert_eq!(roots.publish([]), Err(ManagedRootError::MissingFrame));
    assert_eq!(roots.leave_frame(), Err(ManagedRootError::MissingFrame));
    assert_eq!(roots.frame_count(), 0);
    assert_eq!(roots.lease_count(), 0);
}

#[test]
fn invalid_live_root_publication_cannot_partially_release_existing_values() {
    let mut heap = DomainHeap::new(234);
    let mut data = ManagedData::new(234);
    let retained = data.create_string(&mut heap, "retained").unwrap();
    let released = data.create_string(&mut heap, "released").unwrap();
    data.release(&mut heap, released).unwrap();
    assert_eq!(
        data.retain_roots(&mut heap, [released]),
        Err(DataError::UnknownHandle)
    );
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(data.string(&heap, retained).unwrap(), "retained");
}

#[test]
fn root_publication_requires_the_heap_owner_even_when_no_root_would_be_removed() {
    use rinzu_runtime::heap::HeapError;
    let mut heap = DomainHeap::new(345);
    let mut data = ManagedData::new(345);
    let handle = data.create_string(&mut heap, "owner").unwrap();
    let heap_address = std::ptr::from_mut(&mut heap) as usize;
    let data_address = std::ptr::from_mut(&mut data) as usize;
    std::thread::scope(|scope| {
        scope
            .spawn(move || {
                // SAFETY: the scope joins before the stack values can move, and
                // the parent does not access them until then. No loan is active.
                // Bypass Send solely to verify the foreign Owner is rejected.
                let result = unsafe {
                    (&mut *(data_address as *mut ManagedData))
                        .retain_roots(&mut *(heap_address as *mut DomainHeap), [handle])
                };
                assert!(matches!(
                    result,
                    Err(DataError::Heap(HeapError::WrongThread { .. }))
                ));
            })
            .join()
            .unwrap();
    });
    assert_eq!(data.string(&heap, handle).unwrap(), "owner");
}

#[test]
fn identities_from_other_domains_cannot_alias_local_values() {
    let mut first_heap = DomainHeap::new(901);
    let mut second_heap = DomainHeap::new(902);
    let mut first = ManagedData::new(901);
    let mut second = ManagedData::new(902);
    let a = first.create_string(&mut first_heap, "first").unwrap();
    let b = second.create_string(&mut second_heap, "second").unwrap();
    assert_ne!(a, b);
    assert_eq!(
        second.string(&second_heap, a),
        Err(DataError::UnknownHandle)
    );
    assert_eq!(first.string(&first_heap, b), Err(DataError::UnknownHandle));
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn aot_managed_payloads_move_and_collect_from_caller_and_error_graph_roots() {
    let program = r#"
#include <assert.h>
#include "rinzu/managed_data.h"
int main(int argc, char **argv) {
  (void)argv;
  rinzu_managed_data_t data = {0}, other = {0};
  rinzu_managed_roots_t roots = {0};
  rinzu_managed_enter(&roots);
  const unsigned char bytes[] = {'a', 0, 0xe6, 0x97, 0xa5};
  uint64_t caller = rinzu_managed_string(&data, bytes, sizeof(bytes));
  uint64_t foreign = rinzu_managed_string(&other, bytes, sizeof(bytes));
  assert(caller != foreign);
  if (argc > 1) { rinzu_managed_publish(&roots, &foreign, 1); rinzu_managed_collect(&data, &roots); return 99; }
  rinzu_failure_state_t pending = {0};
  rinzu_failure_fail(&pending, 5, (rinzu_source_position_t){UINT64_MAX, 12, 34, 1});
  rinzu_failure_begin(&pending);
  rinzu_failure_fail(&pending, 9, (rinzu_source_position_t){42, 56, 78, 1});
  rinzu_failure_end(&pending);
  uint64_t error = rinzu_managed_error(&data, &pending.current);
  rinzu_managed_value_t *error_value = rinzu_managed_find(&data, error);
  assert(error_value->error_source.present && error_value->error_source.file == UINT64_MAX);
  assert(error_value->error_source.start_byte == 12 && error_value->error_source.end_byte == 34);
  assert(error_value->error_head->source.present && error_value->error_head->source.file == 42);
  assert(error_value->error_head->source.start_byte == 56 && error_value->error_head->source.end_byte == 78);
  uint64_t live[] = {caller, error}; rinzu_managed_publish(&roots, live, 2);
  uint64_t caller_hash = rinzu_managed_string_hash(&data, caller);
  uintptr_t old = (uintptr_t)rinzu_managed_string_value(&data, caller)->bytes;
  rinzu_managed_collect(&data, &roots);
  assert((uintptr_t)rinzu_managed_string_value(&data, caller)->bytes != old);
  assert(data.objects == 3);
  assert(rinzu_managed_string_hash(&data, caller) == caller_hash);
  assert(rinzu_managed_string_value(&data, caller)->length == sizeof(bytes));
  assert(memcmp(rinzu_managed_string_value(&data, caller)->bytes, bytes, sizeof(bytes)) == 0);
  rinzu_managed_enter(&roots);
  for (unsigned i = 0; i < 20; ++i) {
    uint64_t temporary = rinzu_managed_string(&data, (const unsigned char *)"temporary", 9);
    rinzu_managed_publish(&roots, &temporary, 1);
    rinzu_managed_collect(&data, &roots);
    assert(data.objects == 4);
  }
  rinzu_managed_leave(&roots);
  uint64_t message = rinzu_managed_message(&data, error);
  rinzu_managed_publish(&roots, &message, 1);
  rinzu_managed_collect(&data, &roots);
  assert(data.objects == 1);
  assert(strstr((const char *)rinzu_managed_string_value(&data, message)->bytes, "; cleanup: native string is not valid UTF-8"));
  uint64_t copied = rinzu_managed_string(&data, rinzu_managed_string_value(&data, message)->bytes, rinzu_managed_string_value(&data, message)->length);
  assert(rinzu_managed_string_equal(&data, message, copied));
  assert(rinzu_managed_string_hash(&data, message) == rinzu_managed_string_hash(&data, copied));
  rinzu_managed_publish(&roots, NULL, 0); rinzu_managed_collect(&data, &roots);
  assert(data.objects == 0);
  rinzu_managed_leave(&roots); rinzu_managed_dispose(&roots);
  rinzu_managed_data_dispose(&data); rinzu_managed_data_dispose(&other);
  rinzu_failure_dispose(&pending);
  return 0;
}
"#;
    let root = std::env::temp_dir().join(format!("rinzu-managed-c-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("test.c"), program).unwrap();
    let include = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include");
    let output = std::process::Command::new("clang")
        .args(["-O2", "-Wall", "-Wextra", "-Werror", "-I"])
        .arg(include)
        .arg(root.join("test.c"))
        .arg("-o")
        .arg(root.join("test"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = std::process::Command::new(root.join("test"))
        .env("RINZU_GC_STRESS", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rejected = std::process::Command::new(root.join("test"))
        .env("RINZU_GC_STRESS", "1")
        .arg("foreign")
        .output()
        .unwrap();
    assert_eq!(rejected.status.code(), Some(70));
    assert!(rejected.stderr.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn aot_shared_main_heap_collects_all_parked_task_roots_and_detaches_at_scope_end() {
    let program = r#"
#include <assert.h>
#include "rinzu/managed_data.h"
static void *foreign_collect(void *raw) {
  rinzu_managed_roots_t empty = {0};
  rinzu_managed_collect(raw, &empty); return NULL;
}
int main(int argc, char **argv) {
  (void)argv;
  rinzu_managed_data_t main_heap = {0}, worker_heap = {0};
  rinzu_managed_roots_t roots[4] = {0};
  rinzu_managed_root_set_t entries[4] = {0};
  for (unsigned i = 0; i < 4; ++i) {
    rinzu_managed_attach_roots(&main_heap, &entries[i], &roots[i]);
    rinzu_managed_enter(&roots[i]);
  }
  if (argc > 1) {
    switch (argv[1][0]) {
      case 'a': rinzu_managed_attach_roots(&main_heap, &entries[0], &roots[0]); break;
      case 'd': rinzu_managed_detach_roots(&main_heap, &entries[1]); break;
      case 'w': rinzu_managed_detach_roots(&worker_heap, &entries[2]); break;
      case 'f': {
        uint64_t foreign = rinzu_managed_string(&worker_heap, (const unsigned char *)"worker", 6);
        rinzu_managed_publish(&roots[2], &foreign, 1);
        rinzu_managed_collect(&main_heap, &roots[0]); break;
      }
      case 't': {
        pthread_t thread;
        assert(!pthread_create(&thread, NULL, foreign_collect, &main_heap));
        assert(!pthread_join(thread, NULL)); break;
      }
      case 'p': rinzu_managed_data_dispose(&main_heap); break;
      default: return 98;
    }
    return 99;
  }
  uint64_t caller = rinzu_managed_string(&main_heap, (const unsigned char *)"caller", 6);
  const unsigned char child_bytes[] = {'a', 0, 0xe6, 0x97, 0xa5};
  uint64_t child = rinzu_managed_string(&main_heap, child_bytes, sizeof(child_bytes));
  rinzu_failure_t failure = {0}; failure.primary = 5;
  uint64_t error = rinzu_managed_error(&main_heap, &failure);
  uint64_t prepared = rinzu_managed_string(&main_heap, (const unsigned char *)"prepared", 8);
  uint64_t live[] = {caller, child, error, prepared};
  for (unsigned i = 0; i < 4; ++i) rinzu_managed_publish(&roots[i], &live[i], 1);
  for (unsigned i = 0; i < 20; ++i) {
    uintptr_t old = (uintptr_t)rinzu_managed_string_value(&main_heap, child)->bytes;
    uint64_t temporary = rinzu_managed_string(&main_heap, (const unsigned char *)"temporary", 9);
    uint64_t active[] = {child, temporary};
    rinzu_managed_publish(&roots[1], active, 2);
    /* The current caller alone cannot account for the parked siblings. */
    rinzu_managed_collect(&main_heap, &roots[0]);
    assert(main_heap.objects == 6);
    assert((uintptr_t)rinzu_managed_string_value(&main_heap, child)->bytes != old);
    assert(!memcmp(rinzu_managed_string_value(&main_heap, child)->bytes, child_bytes, sizeof(child_bytes)));
    assert(rinzu_managed_string_value(&main_heap, caller)->length == 6);
    assert(rinzu_managed_string_value(&main_heap, prepared)->length == 8);
    assert(rinzu_managed_find(&main_heap, error)->error_primary == 5);
  }
  uint64_t message = rinzu_managed_message(&main_heap, error);
  uint64_t caller_live[] = {caller, message};
  rinzu_managed_publish(&roots[0], caller_live, 2);
  rinzu_managed_publish(&roots[1], &child, 1);
  rinzu_managed_leave(&roots[2]); rinzu_managed_detach_roots(&main_heap, &entries[2]);
  rinzu_managed_collect(&main_heap, &roots[1]);
  assert(main_heap.objects == 4);
  assert(strstr((const char *)rinzu_managed_string_value(&main_heap, message)->bytes, "numeric conversion"));
  for (unsigned i = 0; i < 4; ++i) {
    if (i != 2) { rinzu_managed_leave(&roots[i]); rinzu_managed_detach_roots(&main_heap, &entries[i]); }
    rinzu_managed_dispose(&roots[i]);
  }
  rinzu_managed_collect(&main_heap, &roots[0]);
  assert(main_heap.objects == 0 && !main_heap.root_sets);
  rinzu_managed_data_dispose(&main_heap); rinzu_managed_data_dispose(&worker_heap);
}
"#;
    let root = std::env::temp_dir().join(format!("rinzu-shared-main-c-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("test.c"), program).unwrap();
    let include = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include");
    let output = std::process::Command::new("clang")
        .args(["-O2", "-Wall", "-Wextra", "-Werror", "-I"])
        .arg(include)
        .arg(root.join("test.c"))
        .arg("-o")
        .arg(root.join("test"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = std::process::Command::new(root.join("test"))
        .env("RINZU_GC_STRESS", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for mode in [
        "attach",
        "detach",
        "wrong-heap",
        "foreign-root",
        "thread",
        "process-dispose",
    ] {
        let rejected = std::process::Command::new(root.join("test"))
            .env("RINZU_GC_STRESS", "1")
            .arg(mode)
            .output()
            .unwrap();
        assert_eq!(rejected.status.code(), Some(70), "{mode}: {rejected:?}");
    }
    std::fs::remove_dir_all(root).unwrap();
}
