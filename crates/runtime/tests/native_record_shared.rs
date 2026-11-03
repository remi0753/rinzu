use rinzu_runtime::data::{
    DataError, ManagedData, ManagedHandle, ManagedSlot, NativeRecordError, NativeRecordField,
    NativeRecordLayout, NativeRecordLoanError, NativeRecordLoanStats,
};
use rinzu_runtime::heap::{CollectionKind, DomainHeap};
use rinzu_runtime::scalar::{ScalarKind, ScalarValue};
use rinzu_runtime::value::{ValueSchema, ValueType};
use std::sync::Arc;

const PAIR: usize = 2;
const BOXED: usize = 3;
const OTHER: usize = 4;
const IMMUTABLE: usize = 5;

fn schema() -> Arc<ValueSchema> {
    Arc::new(
        ValueSchema::new(vec![
            ValueType::Scalar(ScalarKind::I64),
            ValueType::Scalar(ScalarKind::Bool),
            ValueType::State {
                identity: "Pair".into(),
                fields: vec![0, 1],
            },
            ValueType::Record {
                identity: "Box".into(),
                fields: vec![PAIR],
            },
            ValueType::State {
                identity: "Other".into(),
                fields: vec![0, 1],
            },
            ValueType::Record {
                identity: "Immutable".into(),
                fields: vec![0, 1],
            },
        ])
        .unwrap(),
    )
}

fn layout(ty: usize) -> NativeRecordLayout {
    NativeRecordLayout::new(
        ty,
        16,
        8,
        vec![
            NativeRecordField {
                kind: ScalarKind::I64,
                offset: 0,
            },
            NativeRecordField {
                kind: ScalarKind::Bool,
                offset: 8,
            },
        ],
    )
    .unwrap()
}

fn values(number: i128, flag: bool) -> [ScalarValue; 2] {
    [
        ScalarValue::from_integer(ScalarKind::I64, number).unwrap(),
        ScalarValue::from_bits(ScalarKind::Bool, u64::from(flag)).unwrap(),
    ]
}

fn configured() -> (DomainHeap, ManagedData) {
    let heap = DomainHeap::new(901);
    let mut data = ManagedData::new(901);
    data.configure_values(&heap, schema()).unwrap();
    (heap, data)
}

fn create(data: &mut ManagedData, heap: &mut DomainHeap, ty: usize) -> ManagedHandle {
    data.create_composite(heap, ty, None, &values(1, false).map(ManagedSlot::Scalar))
        .unwrap()
}

fn read(data: &mut ManagedData, heap: &mut DomainHeap, handle: ManagedHandle) -> [ManagedSlot; 2] {
    [
        data.project(heap, handle, PAIR, None, 0).unwrap(),
        data.project(heap, handle, PAIR, None, 1).unwrap(),
    ]
}

#[test]
fn projected_roots_share_a_target_across_gc_without_merging_equal_targets() {
    let (mut heap, mut data) = configured();
    let layout = layout(PAIR);
    let original = create(&mut data, &mut heap, PAIR);
    let separate = create(&mut data, &mut heap, PAIR);
    let parent = data
        .create_composite(&mut heap, BOXED, None, &[ManagedSlot::Reference(original)])
        .unwrap();
    let ManagedSlot::Reference(projected) =
        data.project(&mut heap, parent, BOXED, None, 0).unwrap()
    else {
        panic!("expected a rooted reference");
    };
    assert_ne!(projected, original);
    let identity = data
        .native_record_identity(&heap, &layout, original)
        .unwrap();
    assert_eq!(
        identity,
        data.native_record_identity(&heap, &layout, projected)
            .unwrap()
    );
    assert_ne!(
        identity,
        data.native_record_identity(&heap, &layout, separate)
            .unwrap()
    );
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(
        identity,
        data.native_record_identity(&heap, &layout, original)
            .unwrap()
    );
    data.update_native_record_fields(&mut heap, &layout, projected, &values(77, true))
        .unwrap();
    assert_eq!(
        read(&mut data, &mut heap, original),
        values(77, true).map(ManagedSlot::Scalar)
    );
    assert_eq!(
        read(&mut data, &mut heap, separate),
        values(1, false).map(ManagedSlot::Scalar)
    );
    data.release(&mut heap, original).unwrap();
    data.release(&mut heap, parent).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(
        identity,
        data.native_record_identity(&heap, &layout, projected)
            .unwrap()
    );
    assert_eq!(
        read(&mut data, &mut heap, projected),
        values(77, true).map(ManagedSlot::Scalar)
    );
}

#[test]
fn writeback_checks_the_entire_output_type_and_domain_before_mutation() {
    let (mut heap, mut data) = configured();
    let layout = layout(PAIR);
    let original = create(&mut data, &mut heap, PAIR);
    let invalid = [values(9, true)[0], values(1, false)[0]];
    assert_eq!(
        data.update_native_record_fields(&mut heap, &layout, original, &invalid),
        Err(NativeRecordError::Conversion)
    );
    assert_eq!(
        data.update_native_record_fields(&mut heap, &layout, original, &invalid[..1]),
        Err(NativeRecordError::Conversion)
    );
    let other = create(&mut data, &mut heap, OTHER);
    assert_eq!(
        data.update_native_record_fields(&mut heap, &layout, other, &values(9, true)),
        Err(NativeRecordError::Data(DataError::WrongKind))
    );
    let foreign = DomainHeap::new(902);
    assert_eq!(
        data.native_record_identity(&foreign, &layout, original),
        Err(NativeRecordError::Data(DataError::WrongDomain))
    );
    let immutable = create(&mut data, &mut heap, IMMUTABLE);
    assert_eq!(
        data.update_native_record_fields(
            &mut heap,
            &self::layout(IMMUTABLE),
            immutable,
            &values(9, true)
        ),
        Err(NativeRecordError::InvalidLayout)
    );
    assert_eq!(
        read(&mut data, &mut heap, original),
        values(1, false).map(ManagedSlot::Scalar)
    );
}

#[test]
fn new_received_data_creates_a_separate_shared_target() {
    let (mut heap, mut data) = configured();
    let layout = layout(PAIR);
    let original = create(&mut data, &mut heap, PAIR);
    let copy = data
        .copy_native_record(&mut heap, &layout, original)
        .unwrap();
    // SAFETY: this initialized buffer owns all fields of the validated layout.
    let received =
        unsafe { data.receive_native_record(&mut heap, &layout, copy.as_ptr()) }.unwrap();
    assert_ne!(
        data.native_record_identity(&heap, &layout, original)
            .unwrap(),
        data.native_record_identity(&heap, &layout, received)
            .unwrap()
    );
    data.update_native_record_fields(&mut heap, &layout, received, &values(2, true))
        .unwrap();
    assert_eq!(
        read(&mut data, &mut heap, original),
        values(1, false).map(ManagedSlot::Scalar)
    );
}

#[test]
fn reference_schema_and_its_container_do_not_offer_value_markers() {
    let schema = schema();
    for ty in [PAIR, BOXED] {
        assert!(!schema.has_equality(ty).unwrap());
        assert!(!schema.is_key(ty).unwrap());
        assert!(!schema.has_transferable(ty).unwrap());
    }
    assert!(schema.has_equality(IMMUTABLE).unwrap());
    assert!(schema.is_key(IMMUTABLE).unwrap());
    assert!(schema.has_transferable(IMMUTABLE).unwrap());
}

fn assert_busy(data: &mut ManagedData, heap: &mut DomainHeap, handle: ManagedHandle) {
    assert!(matches!(
        data.project(heap, handle, PAIR, None, 0),
        Err(DataError::Thrown(error)) if error.root().code == 4
    ));
}

#[test]
fn loans_merge_aliases_but_keep_equal_targets_separate() {
    let (mut heap, mut data) = configured();
    let layout = Arc::new(layout(PAIR));
    let original = create(&mut data, &mut heap, PAIR);
    let separate = create(&mut data, &mut heap, PAIR);
    let parent = data
        .create_composite(&mut heap, BOXED, None, &[ManagedSlot::Reference(original)])
        .unwrap();
    let ManagedSlot::Reference(alias) = data.project(&mut heap, parent, BOXED, None, 0).unwrap()
    else {
        panic!("expected reference")
    };
    let loan = data
        .prepare_native_records(
            &mut heap,
            &[
                (Arc::clone(&layout), original),
                (Arc::clone(&layout), alias),
                (Arc::clone(&layout), separate),
            ],
        )
        .unwrap();
    use rinzu_runtime::data::display::{DisplaySchema, DisplayType as D};
    let display = DisplaySchema(vec![
        D::Scalar(ScalarKind::I64),
        D::Scalar(ScalarKind::Bool),
        D::Record {
            name: "Pair".into(),
            fields: vec![("x".into(), 0), ("flag".into(), 1)],
        },
        D::Record {
            name: "Box".into(),
            fields: vec![("pair".into(), PAIR)],
        },
        D::Marker("Other".into()),
        D::Record {
            name: "Immutable".into(),
            fields: vec![("x".into(), 0), ("flag".into(), 1)],
        },
    ]);
    assert!(
        matches!(data.display(&heap, &display, BOXED, ManagedSlot::Reference(parent)),
        Err(DataError::Thrown(error)) if error.root().code == 4)
    );
    let pointer = data.native_record_argument_pointer(&heap, loan, 0).unwrap();
    assert_eq!(
        pointer,
        data.native_record_argument_pointer(&heap, loan, 1).unwrap()
    );
    assert_ne!(
        pointer,
        data.native_record_argument_pointer(&heap, loan, 2).unwrap()
    );
    assert_eq!(
        data.native_record_loan_stats(),
        NativeRecordLoanStats {
            calls: 1,
            targets: 2,
            bytes: 32,
        }
    );
    assert_busy(&mut data, &mut heap, original);
    assert_busy(&mut data, &mut heap, alias);
    // Reading a container's reference field is allowed while the target is Busy.
    let ManagedSlot::Reference(held) = data.project(&mut heap, parent, BOXED, None, 0).unwrap()
    else {
        panic!("expected reference")
    };
    assert_busy(&mut data, &mut heap, held);
    assert_eq!(data.release_all(&mut heap), Err(DataError::LeasedHandle));
    data.discard_prepared_native_records(&mut heap, loan)
        .unwrap();
    assert_eq!(
        read(&mut data, &mut heap, held),
        values(1, false).map(ManagedSlot::Scalar)
    );
    assert_eq!(
        data.native_record_loan_stats(),
        NativeRecordLoanStats::default()
    );
    data.release_all(&mut heap).unwrap();
}

#[test]
fn failed_reloan_does_not_acquire_or_release_any_target() {
    let (mut heap, mut data) = configured();
    let layout = Arc::new(layout(PAIR));
    let first = create(&mut data, &mut heap, PAIR);
    let other = create(&mut data, &mut heap, PAIR);
    let outer = data
        .prepare_native_records(&mut heap, &[(Arc::clone(&layout), first)])
        .unwrap();
    let before = data.native_record_loan_stats();
    let roots_before = data.root_count();
    assert_eq!(
        data.prepare_native_records(
            &mut heap,
            &[(Arc::clone(&layout), other), (Arc::clone(&layout), first),]
        ),
        Err(NativeRecordLoanError::Busy)
    );
    assert_eq!(data.native_record_loan_stats(), before);
    assert_eq!(data.root_count(), roots_before);
    assert_eq!(
        read(&mut data, &mut heap, other),
        values(1, false).map(ManagedSlot::Scalar)
    );
    let inner = data
        .prepare_native_records(&mut heap, &[(layout, other)])
        .unwrap();
    assert_eq!(data.native_record_loan_stats().calls, 2);
    data.discard_prepared_native_records(&mut heap, inner)
        .unwrap();
    assert_busy(&mut data, &mut heap, first);
    data.discard_prepared_native_records(&mut heap, outer)
        .unwrap();
    assert_eq!(
        data.native_record_loan_stats(),
        NativeRecordLoanStats::default()
    );
}

#[test]
fn private_roots_and_c_buffers_survive_pruning_and_moving_gc() {
    let (mut heap, mut data) = configured();
    let layout = Arc::new(layout(PAIR));
    let original = create(&mut data, &mut heap, PAIR);
    let loan = data
        .prepare_native_records(&mut heap, &[(layout, original)])
        .unwrap();
    let pointer = data.native_record_argument_pointer(&heap, loan, 0).unwrap();
    data.retain_roots(&mut heap, []).unwrap();
    assert_eq!(data.root_count(), 1);
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(
        pointer,
        data.native_record_argument_pointer(&heap, loan, 0).unwrap()
    );
    // SAFETY: the active loan owns the initialized buffer despite heap movement.
    assert_eq!(unsafe { pointer.cast::<i64>().read() }, 1);
    data.discard_prepared_native_records(&mut heap, loan)
        .unwrap();
    assert_eq!(data.root_count(), 0);
    assert_eq!(
        data.native_record_loan_stats(),
        NativeRecordLoanStats::default()
    );
    data.release_all(&mut heap).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
}

#[test]
fn preparation_validates_all_arguments_before_marking_any_target_busy() {
    let (mut heap, mut data) = configured();
    let layout = Arc::new(layout(PAIR));
    let first = create(&mut data, &mut heap, PAIR);
    let wrong = create(&mut data, &mut heap, OTHER);
    assert!(
        data.prepare_native_records(
            &mut heap,
            &[(Arc::clone(&layout), first), (Arc::clone(&layout), wrong),]
        )
        .is_err()
    );
    assert_eq!(
        data.native_record_loan_stats(),
        NativeRecordLoanStats::default()
    );
    assert_eq!(
        read(&mut data, &mut heap, first),
        values(1, false).map(ManagedSlot::Scalar)
    );
    let loan = data
        .prepare_native_records(&mut heap, &[(layout, first)])
        .unwrap();
    let mut foreign = DomainHeap::new(902);
    assert!(
        data.discard_prepared_native_records(&mut foreign, loan)
            .is_err()
    );
    assert_busy(&mut data, &mut heap, first);
    data.discard_prepared_native_records(&mut heap, loan)
        .unwrap();
}

#[test]
fn writeback_publishes_all_targets_and_echo_restores_a_durable_alias() {
    let (mut heap, mut data) = configured();
    let layout = Arc::new(layout(PAIR));
    let original = create(&mut data, &mut heap, PAIR);
    let other = create(&mut data, &mut heap, PAIR);
    let loan = data
        .prepare_native_records(
            &mut heap,
            &[
                (Arc::clone(&layout), original),
                (Arc::clone(&layout), other),
            ],
        )
        .unwrap();
    let first = data.native_record_argument_pointer(&heap, loan, 0).unwrap();
    let second = data.native_record_argument_pointer(&heap, loan, 1).unwrap();
    assert_eq!(
        data.release_native_records(&mut heap, loan),
        Err(NativeRecordLoanError::InvalidPhase)
    );
    data.enter_native_records(&heap, loan).unwrap();
    assert_eq!(
        data.discard_prepared_native_records(&mut heap, loan),
        Err(NativeRecordLoanError::InvalidPhase)
    );
    // SAFETY: simulate native writes to the two valid, exclusively loaned buffers.
    unsafe {
        first.cast::<i64>().write(42);
        first.add(8).write(1);
        second.cast::<i64>().write(99);
    }
    // SAFETY: receiving a known pointer restores the reference without reading Busy contents.
    let echoed = unsafe { data.receive_native_record(&mut heap, &layout, first) }.unwrap();
    assert_ne!(echoed, original);
    assert_eq!(
        data.native_record_identity(&heap, &layout, echoed).unwrap(),
        data.native_record_identity(&heap, &layout, original)
            .unwrap()
    );
    assert_busy(&mut data, &mut heap, echoed);
    data.return_native_records(&heap, loan).unwrap();
    assert_busy(&mut data, &mut heap, original);
    heap.collect(CollectionKind::Full).unwrap();
    data.commit_native_records(&mut heap, loan);
    assert_eq!(
        read(&mut data, &mut heap, original),
        values(42, true).map(ManagedSlot::Scalar)
    );
    assert_eq!(
        read(&mut data, &mut heap, other),
        values(99, false).map(ManagedSlot::Scalar)
    );
    // Result decoding may occur after publication while the correspondence is retained.
    let after = unsafe { data.receive_native_record(&mut heap, &layout, first) }.unwrap();
    assert_eq!(
        data.native_record_identity(&heap, &layout, after).unwrap(),
        data.native_record_identity(&heap, &layout, original)
            .unwrap()
    );
    data.release_native_records(&mut heap, loan).unwrap();
    data.retain_roots(&mut heap, [echoed, after]).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(
        read(&mut data, &mut heap, echoed),
        values(42, true).map(ManagedSlot::Scalar)
    );
    assert_eq!(
        data.native_record_loan_stats(),
        NativeRecordLoanStats::default()
    );
    data.release_all(&mut heap).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(data.root_count(), 0);
    assert_eq!(heap.object_count(), 0);
}

#[test]
fn unknown_native_result_is_copied_while_known_pointer_is_not_interned_after_release() {
    let (mut heap, mut data) = configured();
    let layout = Arc::new(layout(PAIR));
    let original = create(&mut data, &mut heap, PAIR);
    let buffer = rinzu_runtime::data::NativeRecordBuffer::new(&layout, &values(5, true)).unwrap();
    let first = unsafe { data.receive_native_record(&mut heap, &layout, buffer.as_ptr()) }.unwrap();
    let second =
        unsafe { data.receive_native_record(&mut heap, &layout, buffer.as_ptr()) }.unwrap();
    assert_ne!(
        data.native_record_identity(&heap, &layout, first).unwrap(),
        data.native_record_identity(&heap, &layout, second).unwrap()
    );
    let loan = data
        .prepare_native_records(&mut heap, &[(Arc::clone(&layout), original)])
        .unwrap();
    data.enter_native_records(&heap, loan).unwrap();
    data.return_native_records(&heap, loan).unwrap();
    data.commit_native_records(&mut heap, loan);
    data.release_native_records(&mut heap, loan).unwrap();
    assert_eq!(
        data.native_record_loan_stats(),
        NativeRecordLoanStats::default()
    );
}

#[test]
fn native_record_writeback_fatal_child() {
    let Ok(mode) = std::env::var("RINZU_SHARED_RECORD_FAULT") else {
        return;
    };
    let (mut heap, mut data) = configured();
    let layout = Arc::new(layout(PAIR));
    let first = create(&mut data, &mut heap, PAIR);
    let second = create(&mut data, &mut heap, PAIR);
    let loan = data
        .prepare_native_records(&mut heap, &[(Arc::clone(&layout), first), (layout, second)])
        .unwrap();
    let pointer = data.native_record_argument_pointer(&heap, loan, 1).unwrap();
    data.enter_native_records(&heap, loan).unwrap();
    match mode.as_str() {
        "invalid-bool" => {
            unsafe { pointer.add(8).write(2) };
            data.return_native_records(&heap, loan).unwrap();
            data.commit_native_records(&mut heap, loan);
        }
        "early-release" => {
            let _ = data.release_native_records(&mut heap, loan);
        }
        "early-commit" => data.commit_native_records(&mut heap, loan),
        "drop-entered" => drop(data),
        "drop-returned" => {
            data.return_native_records(&heap, loan).unwrap();
            drop(data);
        }
        _ => panic!("unknown mode"),
    }
    panic!("UNEXPECTED_RETURN");
}

#[test]
fn invalid_writeback_and_abandoning_entered_loans_exit_with_86() {
    for mode in [
        "invalid-bool",
        "early-release",
        "early-commit",
        "drop-entered",
        "drop-returned",
    ] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "native_record_writeback_fatal_child"])
            .env("RINZU_SHARED_RECORD_FAULT", mode)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(86),
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stderr).contains("UNEXPECTED_RETURN"));
    }
}
