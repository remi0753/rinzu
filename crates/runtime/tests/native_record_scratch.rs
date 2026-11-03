use rinzu_runtime::data::{
    ManagedData, ManagedHandle, ManagedSlot, NATIVE_RECORD_SCRATCH_LIMIT, NativeRecordField,
    NativeRecordLayout, NativeRecordLoan, NativeRecordLoanError,
};
use rinzu_runtime::heap::{CollectionKind, DomainHeap};
use rinzu_runtime::scalar::{ScalarKind, ScalarValue};
use rinzu_runtime::value::{ValueSchema, ValueType};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;
thread_local! { static TRACK: Cell<bool> = const { Cell::new(false) }; static COUNT: Cell<usize> = const { Cell::new(0) }; static LIVE: Cell<isize> = const { Cell::new(0) }; }
struct Counting;
fn account(bytes: isize, allocation: bool) {
    if TRACK.try_with(Cell::get).unwrap_or(false) {
        LIVE.with(|live| live.set(live.get() + bytes));
        if allocation {
            COUNT.with(|count| count.set(count.get() + 1));
        }
    }
}
// SAFETY: System receives exactly the original allocation contracts; only
// thread-local counters observe requested bytes and allocation/reallocation calls.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forward the caller's valid layout unchanged to System.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            account(layout.size() as isize, true);
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forward the caller's valid layout unchanged to System.
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            account(layout.size() as isize, true);
        }
        p
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: the caller supplies System's live allocation and matching
        // layout, and a valid new size; forward all three unchanged.
        let out = unsafe { System.realloc(p, layout, size) };
        if !out.is_null() {
            account(size as isize - layout.size() as isize, true);
        }
        out
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        account(-(layout.size() as isize), false);
        // SAFETY: forward the caller's live System allocation and its layout.
        unsafe { System.dealloc(p, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;
fn measured<T>(body: impl FnOnce() -> T) -> (T, usize, isize) {
    COUNT.with(|v| v.set(0));
    LIVE.with(|v| v.set(0));
    TRACK.with(|v| v.set(true));
    let value = body();
    TRACK.with(|v| v.set(false));
    (value, COUNT.with(Cell::get), LIVE.with(Cell::get))
}
fn schema_and_layout(
    fields: usize,
) -> (Arc<ValueSchema>, Arc<NativeRecordLayout>, Vec<ManagedSlot>) {
    let kinds = [
        ScalarKind::I64,
        ScalarKind::F64,
        ScalarKind::Bool,
        ScalarKind::I16,
    ];
    let mut offset = 0usize;
    let mut native = Vec::new();
    let mut values = Vec::new();
    let mut schema_fields = Vec::new();
    for index in 0..fields {
        let kind = kinds[index % 4];
        let width = kind.width() as usize / 8;
        offset = offset.next_multiple_of(width);
        native.push(NativeRecordField { kind, offset });
        offset += width;
        schema_fields.push(index % 4);
        values.push(ManagedSlot::Scalar(
            ScalarValue::from_bits(kind, if kind == ScalarKind::Bool { 1 } else { 0 }).unwrap(),
        ));
    }
    let schema = Arc::new(
        ValueSchema::new(vec![
            ValueType::Scalar(kinds[0]),
            ValueType::Scalar(kinds[1]),
            ValueType::Scalar(kinds[2]),
            ValueType::Scalar(kinds[3]),
            ValueType::State {
                identity: "Packet".into(),
                fields: schema_fields,
            },
        ])
        .unwrap(),
    );
    (
        schema,
        Arc::new(NativeRecordLayout::new(4, offset.next_multiple_of(8), 8, native).unwrap()),
        values,
    )
}
fn complete(data: &mut ManagedData, heap: &mut DomainHeap, loan: NativeRecordLoan) {
    data.enter_native_records(heap, loan).unwrap();
    data.return_native_records(heap, loan).unwrap();
    data.commit_native_records(heap, loan);
    data.release_native_records(heap, loan).unwrap();
}
#[test]
fn private_heap_roots_cover_many_targets_after_source_pruning_and_moving_gc() {
    let (schema, layout, values) = schema_and_layout(4);
    let mut heap = DomainHeap::new(1300);
    let mut data = ManagedData::new(1300);
    data.configure_values(&heap, schema).unwrap();
    let targets: Vec<_> = (0..8)
        .map(|_| data.create_composite(&mut heap, 4, None, &values).unwrap())
        .collect();
    let arguments: Vec<_> = (0..32)
        .map(|i| (Arc::clone(&layout), targets[i % 8]))
        .collect();
    let loan = data.prepare_native_records(&mut heap, &arguments).unwrap();
    assert_eq!(data.native_record_loan_stats().targets, 8);
    let pointer = data.native_record_argument_pointer(&heap, loan, 0).unwrap();
    assert_eq!(
        pointer,
        data.native_record_argument_pointer(&heap, loan, 8).unwrap()
    );
    data.retain_roots(&mut heap, []).unwrap();
    assert_eq!(data.root_count(), 8);
    heap.collect(CollectionKind::Full).unwrap();
    data.enter_native_records(&heap, loan).unwrap();
    // SAFETY: the active loan owns this initialized aligned I64 field.
    unsafe {
        pointer.cast::<i64>().write(9);
    }
    data.return_native_records(&heap, loan).unwrap();
    data.commit_native_records(&mut heap, loan);
    // SAFETY: reception uses the same live, typed loan pointer.
    let restored = unsafe { data.receive_native_record(&mut heap, &layout, pointer) }.unwrap();
    data.release_native_records(&mut heap, loan).unwrap();
    assert_eq!(
        data.project(&mut heap, restored, 4, None, 0).unwrap(),
        ManagedSlot::Scalar(ScalarValue::from_bits(ScalarKind::I64, 9).unwrap())
    );
    assert_eq!(data.root_count(), 1);
    data.release(&mut heap, restored).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), 0);
    assert_eq!(data.root_count(), 0);
    assert_eq!(data.native_record_scratch_stats().active_bytes, 0);
    assert!(data.native_record_scratch_stats().retained_bytes <= NATIVE_RECORD_SCRATCH_LIMIT);
}

#[test]
fn warm_storage_has_zero_temporary_and_total_allocations_for_small_calls_and_aliases() {
    for fields in [2, 4, 16, 64] {
        let (schema, layout, values) = schema_and_layout(fields);
        let mut heap = DomainHeap::new(710 + fields as u64);
        let mut data = ManagedData::new(heap.domain_id());
        data.configure_values(&heap, schema).unwrap();
        let handles: Vec<_> = (0..4)
            .map(|_| data.create_composite(&mut heap, 4, None, &values).unwrap())
            .collect();
        for targets in [1, 2, 4] {
            let arguments: Vec<_> = (0..8)
                .map(|index| (layout.clone(), handles[index % targets]))
                .collect();
            let loan = data.prepare_native_records(&mut heap, &arguments).unwrap();
            complete(&mut data, &mut heap, loan);
            let before = data.native_record_scratch_stats();
            let roots = data.root_count();
            let (_, allocations, _) = measured(|| {
                for _ in 0..128 {
                    let loan = data.prepare_native_records(&mut heap, &arguments).unwrap();
                    assert_eq!(data.native_record_loan_stats().targets, targets);
                    complete(&mut data, &mut heap, loan);
                }
            });
            assert_eq!(
                data.native_record_scratch_stats().temporary_allocations,
                before.temporary_allocations,
                "fields={fields}, targets={targets}"
            );
            // At most four source roots and four call roots fit the existing root node.
            assert_eq!(allocations, 0, "fields={fields}, targets={targets}");
            assert_eq!(data.root_count(), roots);
            assert_eq!(data.native_record_loan_stats().calls, 0);
            assert!(
                data.native_record_scratch_stats().retained_bytes <= NATIVE_RECORD_SCRATCH_LIMIT
            );
            eprintln!(
                "NATIVE_RECORD_SCRATCH={}",
                serde_json::json!({"case":"warm", "fields":fields,"targets":targets,"arguments":arguments.len(),"cBytes":layout.size()*targets,"temporaryAllocationsPerCall":0,"totalAllocationsPerCall":allocations/128,"iterations":128,"retainedBytes":data.native_record_scratch_stats().retained_bytes,"additionalRootsAfterCall":data.root_count()-roots})
            );
        }
    }
}
#[test]
fn nested_loans_keep_distinct_buffers_and_warm_each_depth_without_pinning_cached_values() {
    let (schema, layout, values) = schema_and_layout(4);
    let mut heap = DomainHeap::new(800);
    let mut data = ManagedData::new(800);
    data.configure_values(&heap, schema).unwrap();
    let a = data.create_composite(&mut heap, 4, None, &values).unwrap();
    let b = data.create_composite(&mut heap, 4, None, &values).unwrap();
    let first = [(layout.clone(), a)];
    let second = [(layout, b)];
    for warm in [false, true] {
        let before = data.native_record_scratch_stats().temporary_allocations;
        let outer = data.prepare_native_records(&mut heap, &first).unwrap();
        data.enter_native_records(&heap, outer).unwrap();
        let (busy, count, _) = measured(|| data.prepare_native_records(&mut heap, &first));
        assert_eq!(busy, Err(NativeRecordLoanError::Busy));
        assert_eq!(count, 0);
        let inner = data.prepare_native_records(&mut heap, &second).unwrap();
        assert_ne!(
            data.native_record_argument_pointer(&heap, outer, 0)
                .unwrap(),
            data.native_record_argument_pointer(&heap, inner, 0)
                .unwrap()
        );
        heap.collect(CollectionKind::Full).unwrap();
        complete(&mut data, &mut heap, inner);
        data.return_native_records(&heap, outer).unwrap();
        data.commit_native_records(&mut heap, outer);
        data.release_native_records(&mut heap, outer).unwrap();
        if warm {
            assert_eq!(
                before,
                data.native_record_scratch_stats().temporary_allocations
            );
        }
    }
    assert_eq!(data.native_record_scratch_stats().cached_batches, 2);
    eprintln!(
        "NATIVE_RECORD_SCRATCH={}",
        serde_json::json!({"case":"nested","depth":2,"warmTemporaryAllocations":0,"retainedBytes":data.native_record_scratch_stats().retained_bytes})
    );
    data.release(&mut heap, a).unwrap();
    data.release(&mut heap, b).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), 0);
    assert_eq!(data.root_count(), 0);
}
#[test]
fn oversized_scratch_is_released_and_domain_drop_releases_every_scratch_byte() {
    let (schema, layout, values) = schema_and_layout(16384);
    let (_, _, live) = measured(|| {
        let mut heap = DomainHeap::new(900);
        let mut data = ManagedData::new(900);
        data.configure_values(&heap, schema.clone()).unwrap();
        let handle = data.create_composite(&mut heap, 4, None, &values).unwrap();
        let args = [(layout.clone(), handle)];
        let loan = data.prepare_native_records(&mut heap, &args).unwrap();
        assert!(data.native_record_scratch_stats().active_bytes > NATIVE_RECORD_SCRATCH_LIMIT);
        complete(&mut data, &mut heap, loan);
        assert_eq!(data.native_record_scratch_stats().retained_bytes, 0);
        assert_eq!(data.native_record_scratch_stats().cached_batches, 0);
        data.release(&mut heap, handle).unwrap();
        heap.collect(CollectionKind::Full).unwrap();
    });
    assert_eq!(live, 0);
    eprintln!(
        "NATIVE_RECORD_SCRATCH={}",
        serde_json::json!({"case":"oversized-and-drop","fields":16384,"idleRetainedBytesAfterOversized":0,"liveBytesAfterDomainDrop":live})
    );
    let (schema, layout, values) = schema_and_layout(4);
    let (_, _, live) = measured(|| {
        let mut heap = DomainHeap::new(901);
        let mut data = ManagedData::new(901);
        data.configure_values(&heap, schema.clone()).unwrap();
        let h: ManagedHandle = data.create_composite(&mut heap, 4, None, &values).unwrap();
        let loan = data
            .prepare_native_records(&mut heap, &[(layout.clone(), h)])
            .unwrap();
        complete(&mut data, &mut heap, loan);
        assert!(data.native_record_scratch_stats().retained_bytes > 0);
    });
    assert_eq!(live, 0);
}

#[test]
fn deep_reentry_cache_obeys_one_domain_budget_and_never_revives_a_stale_loan() {
    let (schema, layout, values) = schema_and_layout(4);
    let mut heap = DomainHeap::new(902);
    let mut data = ManagedData::new(902);
    data.configure_values(&heap, schema).unwrap();
    let handles: Vec<_> = (0..512)
        .map(|_| data.create_composite(&mut heap, 4, None, &values).unwrap())
        .collect();
    let mut loans = Vec::new();
    for handle in &handles {
        let loan = data
            .prepare_native_records(&mut heap, &[(layout.clone(), *handle)])
            .unwrap();
        data.enter_native_records(&heap, loan).unwrap();
        loans.push(loan);
    }
    let stale = *loans.last().unwrap();
    while let Some(loan) = loans.pop() {
        data.return_native_records(&heap, loan).unwrap();
        data.commit_native_records(&mut heap, loan);
        data.release_native_records(&mut heap, loan).unwrap();
    }
    let stats = data.native_record_scratch_stats();
    assert!(stats.retained_bytes <= NATIVE_RECORD_SCRATCH_LIMIT);
    assert!(stats.cached_batches > 0 && stats.cached_batches < 512);
    eprintln!(
        "NATIVE_RECORD_SCRATCH={}",
        serde_json::json!({"case":"deep","depth":512,"retainedBytes":stats.retained_bytes,"cachedBatches":stats.cached_batches,"budgetBytes":NATIVE_RECORD_SCRATCH_LIMIT})
    );
    let fresh = data
        .prepare_native_records(&mut heap, &[(layout, handles[0])])
        .unwrap();
    assert_eq!(
        data.enter_native_records(&heap, stale),
        Err(NativeRecordLoanError::UnknownLoan)
    );
    data.discard_prepared_native_records(&mut heap, fresh)
        .unwrap();
    for handle in handles {
        data.release(&mut heap, handle).unwrap();
    }
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), 0);
    assert_eq!(data.root_count(), 0);
}
