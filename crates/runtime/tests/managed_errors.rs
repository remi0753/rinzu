use rinzu_runtime::{
    child_scope::ChildScope,
    data::{
        DataError, ManagedData, ManagedHandle, ManagedSlot as S,
        display::{DisplaySchema, DisplayType as D},
    },
    failure::{Diagnostic, ErrorKind, Failure, FailureCause, PendingFailure, SourcePosition},
    heap::{CollectionKind, DomainHeap},
    scalar::{ScalarKind, ScalarValue},
    transport::typed::Descriptor,
    value::{Value, ValueError, ValueSchema, ValueType as T},
};
use std::collections::BTreeMap;
use std::sync::Arc;

fn schema() -> Arc<ValueSchema> {
    Arc::new(
        ValueSchema::new(vec![
            T::String, // 0
            T::Scalar(ScalarKind::I64),
            T::State {
                identity: "Document".into(),
                fields: vec![1],
            },
            T::ErrorRecord {
                identity: "SaveFailed".into(),
                fields: vec![0, 0, 5, 6, 2],
            },
            T::Error,
            T::Enum {
                identity: "Option<SourceLocation>".into(),
                variants: vec![vec![], vec![8]],
            },
            T::Enum {
                identity: "Option<Error>".into(),
                variants: vec![vec![], vec![4]],
            },
            T::List(4),
            T::Record {
                identity: "SourceLocation".into(),
                fields: vec![0, 1, 1],
            },
        ])
        .unwrap(),
    )
}

fn configure_sources(data: &mut ManagedData, heap: &DomainHeap) {
    data.configure_sources(
        heap,
        Arc::new(rinzu_runtime::data::SourceCatalog {
            files: BTreeMap::from([(1, "missing/日本語.rinzu".into())]),
            positions: BTreeMap::from([(1, BTreeMap::from([(17, (2, 9))]))]),
        }),
    )
    .unwrap();
}

#[test]
fn error_trace_is_shared_across_aliases_and_gc_without_retaining_payload_or_frames() {
    let mut heap = DomainHeap::new(94);
    let mut data = ManagedData::new(94);
    data.configure_values(&heap, schema()).unwrap();
    let (error, _document) = error(&mut data, &mut heap);
    let mut stack = rinzu_runtime::error_trace::TraceStack::new(true);
    let token = stack.enter("origin", None);
    let trace = stack
        .capture(Some(SourcePosition {
            file: 1,
            start_byte: 17,
            end_byte: 29,
        }))
        .unwrap();
    assert!(stack.leave(token));
    drop(stack);
    assert!(data.error_trace(&heap, error).unwrap().is_none());
    let saved = data
        .first_error_trace(&mut heap, error, Arc::clone(&trace))
        .unwrap();
    assert!(Arc::ptr_eq(&saved, &trace));
    let list = data
        .create_composite(&mut heap, 7, None, &[S::Reference(error)])
        .unwrap();
    for _ in 0..3 {
        data.retain_roots(&mut heap, [list]).unwrap();
        heap.collect(CollectionKind::Full).unwrap();
        let alias = reference(data.project(&mut heap, list, 7, None, 0).unwrap());
        let mut later = rinzu_runtime::error_trace::TraceStack::new(true);
        let _token = later.enter("rethrow", None);
        let alternative = later.capture(None).unwrap();
        let actual = data
            .first_error_trace(&mut heap, alias, alternative)
            .unwrap();
        assert!(Arc::ptr_eq(&trace, &actual));
        assert_eq!(&*actual.frames[0].function, "origin");
    }
    let weak = Arc::downgrade(&trace);
    drop(trace);
    data.retain_roots(&mut heap, []).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), 0);
    assert_eq!(&*saved.frames[0].function, "origin");
    drop(saved);
    assert!(weak.upgrade().is_none());
}

#[test]
fn first_source_is_lazy_preserves_aliases_and_survives_moving_gc_with_payload() {
    let mut heap = DomainHeap::new(92);
    let mut data = ManagedData::new(92);
    data.configure_values(&heap, schema()).unwrap();
    configure_sources(&mut data, &heap);
    let (error, document) = error(&mut data, &mut heap);
    let before = reference(data.error_record_field(&mut heap, error, 2).unwrap());
    assert_eq!(data.enum_variant(&heap, before, 5).unwrap(), 0);
    let first = SourcePosition {
        file: 1,
        start_byte: 17,
        end_byte: 29,
    };
    assert_eq!(
        data.first_error_source(&mut heap, error, Some(first))
            .unwrap(),
        Some(first)
    );
    assert_eq!(
        data.first_error_source(
            &mut heap,
            error,
            Some(SourcePosition {
                file: 9,
                start_byte: 91,
                end_byte: 99
            })
        )
        .unwrap(),
        Some(first)
    );
    assert_eq!(data.enum_variant(&heap, before, 5).unwrap(), 0);
    let source = reference(data.error_record_field(&mut heap, error, 2).unwrap());
    assert_eq!(data.enum_variant(&heap, source, 5).unwrap(), 1);
    let list = data
        .create_composite(&mut heap, 7, None, &[S::Reference(error)])
        .unwrap();
    for value in [31, 47] {
        data.set_state_field(&mut heap, document, 2, 0, int(value))
            .unwrap();
        data.retain_roots(&mut heap, [list, document]).unwrap();
        heap.collect(CollectionKind::Full).unwrap();
        let error = reference(data.project(&mut heap, list, 7, None, 0).unwrap());
        assert_eq!(data.error_source(&heap, error).unwrap(), Some(first));
        let option = reference(data.error_record_field(&mut heap, error, 2).unwrap());
        let location = reference(data.project(&mut heap, option, 5, Some(1), 0).unwrap());
        let file = reference(data.project(&mut heap, location, 8, None, 0).unwrap());
        assert_eq!(data.string(&heap, file).unwrap(), "missing/日本語.rinzu");
        assert_eq!(
            data.project(&mut heap, location, 8, None, 1).unwrap(),
            int(2)
        );
        assert_eq!(
            data.project(&mut heap, location, 8, None, 2).unwrap(),
            int(9)
        );
        let payload = reference(data.error_record_field(&mut heap, error, 4).unwrap());
        assert_eq!(
            data.project(&mut heap, payload, 2, None, 0).unwrap(),
            int(value)
        );
    }
    data.retain_roots(&mut heap, []).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!((heap.object_count(), data.root_count()), (0, 0));
}

#[test]
fn builtin_and_legacy_error_sources_keep_original_ranges_and_unknown_positions_are_none() {
    let mut heap = DomainHeap::new(93);
    let mut data = ManagedData::new(93);
    data.configure_values(&heap, schema()).unwrap();
    configure_sources(&mut data, &heap);
    let first = SourcePosition {
        file: 1,
        start_byte: 17,
        end_byte: 29,
    };
    let operation = data
        .create_error(
            &mut heap,
            &Failure::new(Diagnostic {
                cause: FailureCause::Error(ErrorKind::Conversion),
                source: Some(first),
            }),
        )
        .unwrap();
    let legacy = data
        .create_error_value(
            &mut heap,
            &rinzu_runtime::error_value::ErrorValue::new(7, "legacy"),
        )
        .unwrap();
    assert_eq!(data.error_source(&heap, legacy).unwrap(), None);
    for error in [operation, legacy] {
        data.first_error_source(&mut heap, error, Some(first))
            .unwrap();
        data.first_error_source(
            &mut heap,
            error,
            Some(SourcePosition {
                file: 8,
                start_byte: 80,
                end_byte: 90,
            }),
        )
        .unwrap();
        assert_eq!(
            data.error_value(&heap, error).unwrap().root().source,
            Some(first)
        );
        let option = reference(data.error_metadata(&mut heap, error, 2, 5).unwrap());
        assert_eq!(data.enum_variant(&heap, option, 5).unwrap(), 1);
    }
    let unknown = data
        .create_error(
            &mut heap,
            &Failure::new(Diagnostic {
                cause: FailureCause::Error(ErrorKind::Conversion),
                source: None,
            }),
        )
        .unwrap();
    let option = reference(data.error_metadata(&mut heap, unknown, 2, 5).unwrap());
    assert_eq!(data.enum_variant(&heap, option, 5).unwrap(), 0);
    data.retain_roots(&mut heap, [operation, legacy]).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(data.error_source(&heap, legacy).unwrap(), Some(first));
    data.retain_roots(&mut heap, []).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), 0);
}

fn int(n: i64) -> S {
    S::Scalar(ScalarValue::from_bits(ScalarKind::I64, n as u64).unwrap())
}
fn reference(slot: S) -> ManagedHandle {
    let S::Reference(handle) = slot else {
        panic!("expected reference")
    };
    handle
}
fn error(data: &mut ManagedData, heap: &mut DomainHeap) -> (ManagedHandle, ManagedHandle) {
    let document = data.create_composite(heap, 2, None, &[int(7)]).unwrap();
    let name = data.create_string(heap, "SaveFailed").unwrap();
    let message = data.create_string(heap, "日本語\0message").unwrap();
    let source = data.create_composite(heap, 5, Some(0), &[]).unwrap();
    let cause = data.create_composite(heap, 6, Some(0), &[]).unwrap();
    let error = data
        .create_composite(
            heap,
            3,
            None,
            &[
                S::Reference(name),
                S::Reference(message),
                S::Reference(source),
                S::Reference(cause),
                S::Reference(document),
            ],
        )
        .unwrap();
    (error, document)
}

fn display_schema(values: &ValueSchema) -> DisplaySchema {
    DisplaySchema(
        (0..values.type_count())
            .map(|index| match values.ty(index).unwrap() {
                T::String => D::String,
                T::Scalar(kind) => D::Scalar(*kind),
                T::State { .. } => D::Marker("<class Document>".into()),
                T::Error | T::ErrorRecord { .. } => D::Error,
                T::Record { identity, fields } => D::Record {
                    name: identity.clone(),
                    fields: fields
                        .iter()
                        .enumerate()
                        .map(|(index, ty)| (format!("field{index}"), *ty))
                        .collect(),
                },
                T::Enum { identity, variants } => D::Enum {
                    name: identity.clone(),
                    variants: variants
                        .iter()
                        .enumerate()
                        .map(|(index, fields)| {
                            (
                                format!("variant{index}"),
                                fields
                                    .iter()
                                    .enumerate()
                                    .map(|(index, ty)| (format!("field{index}"), *ty))
                                    .collect(),
                            )
                        })
                        .collect(),
                },
                T::List(element) => D::List(*element),
                _ => unreachable!("fixture's closed display schema"),
            })
            .collect(),
    )
}

#[test]
fn nominal_error_in_common_error_collection_keeps_payload_identity_across_moving_gc() {
    let mut heap = DomainHeap::new(17);
    let mut data = ManagedData::new(17);
    data.configure_values(&heap, schema()).unwrap();
    let (error, document) = error(&mut data, &mut heap);
    let original = data.snapshot(&heap, 2, S::Reference(document)).unwrap();
    let list = data
        .create_composite(&mut heap, 7, None, &[S::Reference(error)])
        .unwrap();
    let common = data.snapshot(&heap, 4, S::Reference(error)).unwrap();
    assert!(matches!(common, Value::Other { ty: 3, .. }));
    let display = display_schema(&schema());
    for ty in [3, 4] {
        assert_eq!(
            data.display(&heap, &display, ty, S::Reference(error))
                .unwrap(),
            r#"Error(message: "日本語\u{00}message")"#.as_bytes()
        );
    }
    for value in [11, 19, 31] {
        data.set_state_field(&mut heap, document, 2, 0, int(value))
            .unwrap();
        data.retain_roots(&mut heap, [list, document]).unwrap();
        let report = heap.collect(CollectionKind::Full).unwrap();
        assert!(report.moved_objects > 0);
        let error = reference(data.project(&mut heap, list, 7, None, 0).unwrap());
        assert_eq!(data.error_record_type(&heap, error).unwrap(), Some(3));
        let payload = reference(data.error_record_field(&mut heap, error, 4).unwrap());
        let after = data.snapshot(&heap, 2, S::Reference(payload)).unwrap();
        match (&original, after) {
            (Value::Other { slot: before, .. }, Value::Other { slot: after, .. }) => {
                assert_eq!(*before, after)
            }
            _ => panic!("payload must retain reference identity"),
        }
        assert_eq!(
            data.project(&mut heap, payload, 2, None, 0).unwrap(),
            int(value)
        );
        let message = data.error_message(&mut heap, error).unwrap();
        assert_eq!(data.string(&heap, message).unwrap(), "日本語\0message");
    }
    data.retain_roots(&mut heap, []).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), 0);
}

#[test]
fn cloned_failure_leases_keep_only_error_graph_alive_and_release_after_last_observer() {
    let mut heap = DomainHeap::new(23);
    let mut data = ManagedData::new(23);
    data.configure_values(&heap, schema()).unwrap();
    let (error, _) = error(&mut data, &mut heap);
    data.create_string(&mut heap, "unrelated local must be collected")
        .unwrap();
    let pending = data.lease_error(&heap, error).unwrap();
    let future = pending.clone();
    let independently_obtained = data.lease_error(&heap, error).unwrap();
    assert_eq!(data.release(&mut heap, error), Err(DataError::LeasedHandle));
    drop(pending);
    drop(independently_obtained);
    for _ in 0..3 {
        data.retain_roots(&mut heap, []).unwrap();
        heap.collect(CollectionKind::Full).unwrap();
        assert_eq!(data.root_count(), 1);
        assert_eq!(heap.object_count(), 6);
        let error = data.leased_error(&heap, &future).unwrap();
        let message = data.error_message(&mut heap, error).unwrap();
        assert_eq!(data.string(&heap, message).unwrap(), "日本語\0message");
    }
    drop(future);
    data.retain_roots(&mut heap, []).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(data.root_count(), 0);
    assert_eq!(heap.object_count(), 0);
}

#[test]
fn error_leases_cannot_be_resolved_in_a_different_domain() {
    let mut heap = DomainHeap::new(29);
    let mut data = ManagedData::new(29);
    data.configure_values(&heap, schema()).unwrap();
    let (error, _) = error(&mut data, &mut heap);
    let lease = data.lease_error(&heap, error).unwrap();
    let other_heap = DomainHeap::new(30);
    let mut other = ManagedData::new(30);
    other.configure_values(&other_heap, schema()).unwrap();
    assert_eq!(
        other.leased_error(&other_heap, &lease),
        Err(DataError::WrongDomain)
    );
    assert_eq!(
        data.leased_error(&other_heap, &lease),
        Err(DataError::WrongDomain)
    );
    let text = data.create_string(&mut heap, "not an Error").unwrap();
    assert_eq!(data.lease_error(&heap, text), Err(DataError::WrongKind));
}

#[test]
fn errors_are_readonly_and_have_no_value_equality_key_or_transport_permission() {
    let values = schema();
    assert!(!values.has_equality(3).unwrap());
    assert!(!values.is_key(3).unwrap());
    assert!(!values.has_transferable(3).unwrap());
    assert!(Descriptor::new(values.clone(), vec![3]).is_err());
    assert!(
        values
            .validate(3, &Value::Other { ty: 2, slot: 99 })
            .is_err()
    );
    assert!(matches!(
        ValueSchema::new(vec![T::ErrorRecord {
            identity: "Bad".into(),
            fields: vec![]
        }]),
        Err(ValueError::InvalidSchema)
    ));
    let mut heap = DomainHeap::new(37);
    let mut data = ManagedData::new(37);
    data.configure_values(&heap, values).unwrap();
    let (error, _) = error(&mut data, &mut heap);
    let replacement = data.create_string(&mut heap, "replacement").unwrap();
    assert_eq!(
        data.set_state_field(&mut heap, error, 3, 1, S::Reference(replacement)),
        Err(DataError::WrongKind)
    );
    let value = data.snapshot(&heap, 3, S::Reference(error)).unwrap();
    assert!(data.materialize_values(&mut heap, &[3], &[value]).is_err());
}

#[test]
fn pending_cleanup_and_child_control_preserve_error_indices_and_leases() {
    let mut heap = DomainHeap::new(41);
    let mut data = ManagedData::new(41);
    data.configure_values(&heap, schema()).unwrap();
    let (error, _) = error(&mut data, &mut heap);
    let failure = Failure::with_managed(
        Diagnostic {
            cause: FailureCause::Error(ErrorKind::User),
            source: None,
        },
        data.lease_error(&heap, error).unwrap(),
    );
    let mut pending = PendingFailure::default();
    pending.propagate(failure.clone());
    pending.begin_cleanup();
    pending.propagate(failure.clone());
    pending.end_cleanup().unwrap();
    let combined = pending.finish().unwrap().unwrap();
    assert_eq!(combined.managed.keys().copied().collect::<Vec<_>>(), [0, 1]);
    assert!(combined.diagnostics_only().managed.is_empty());
    let cleanup_only = combined.without_primary().unwrap();
    assert_eq!(
        cleanup_only.managed.keys().copied().collect::<Vec<_>>(),
        [0]
    );
    let mut control = Failure::new(FailureCause::Cancelled);
    control.append_cleanup(cleanup_only);
    let mut children = ChildScope::new();
    children.admit(1).unwrap();
    children
        .begin_close(Some(Failure::new(FailureCause::Cancelled)))
        .unwrap();
    children.complete(1, Some(control)).unwrap();
    let outcome = children.finish_close().unwrap().unwrap();
    assert_eq!(outcome.managed.keys().copied().collect::<Vec<_>>(), [1]);
    drop(failure);
    data.retain_roots(&mut heap, []).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(
        data.leased_error(&heap, &outcome.managed[&1]).unwrap(),
        error
    );
    drop(children);
    drop(outcome);
    data.retain_roots(&mut heap, []).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!((data.root_count(), heap.object_count()), (0, 0));
}

#[test]
fn lexical_error_groups_keep_payloads_until_dispatch_or_group_drop() {
    use rinzu_runtime::error_groups::{ErrorGroups, aggregate};
    let mut heap = DomainHeap::new(43);
    let mut data = ManagedData::new(43);
    data.configure_values(&heap, schema()).unwrap();
    let (error, _) = error(&mut data, &mut heap);
    let failure = Failure::with_managed(
        Diagnostic {
            cause: FailureCause::Error(ErrorKind::User),
            source: None,
        },
        data.lease_error(&heap, error).unwrap(),
    );
    let mut groups = ErrorGroups::default();
    groups.enter(1);
    groups.record(1, Some(7), failure.clone()).unwrap();
    groups.record(1, Some(7), failure).unwrap();
    for _ in 0..3 {
        data.retain_roots(&mut heap, []).unwrap();
        heap.collect(CollectionKind::Full).unwrap();
        let payload = reference(data.project(&mut heap, error, 3, None, 4).unwrap());
        assert_eq!(
            data.project(&mut heap, payload, 2, None, 0).unwrap(),
            int(7)
        );
    }
    let combined = aggregate(groups.finish(1).unwrap()).unwrap();
    assert_eq!(combined.managed.len(), 1);
    assert_eq!(
        data.leased_error(&heap, &combined.managed[&0]).unwrap(),
        error
    );
    groups.enter(2);
    groups.record(2, None, combined).unwrap();
    drop(groups);
    data.retain_roots(&mut heap, []).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!((heap.object_count(), data.root_count()), (0, 0));
}
