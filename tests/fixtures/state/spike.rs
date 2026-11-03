//! Comparison only: test-local mutation of the actual DomainHeap, not a source feature.
use super::*;

fn update(heap: &mut DomainHeap, object: HeapRef, index: usize, value: HeapValue) {
    heap.require_owner().unwrap();
    let header = heap.header(object).unwrap();
    assert!(index < header.field_count as usize);
    if let HeapValue::Reference(reference) = value {
        heap.validate_reference(reference).unwrap();
    }
    let offset = object.offset_words + HEADER_WORDS + index * FIELD_WORDS;
    let (tag, payload) = DomainHeap::encode_value(value);
    heap.words[offset] = tag;
    heap.words[offset + 1] = payload;
}
fn location(heap: &DomainHeap, root: RootSlot, field: usize, cells: bool) -> (HeapRef, usize) {
    let object = heap.root_target(root).unwrap().unwrap();
    if cells {
        let HeapValue::Reference(cell) = heap.fields(object).unwrap()[field] else {
            panic!("cell")
        };
        (cell, 0)
    } else {
        (object, field)
    }
}
fn read(heap: &DomainHeap, root: RootSlot, field: usize, cells: bool) -> i64 {
    let (object, index) = location(heap, root, field, cells);
    let HeapValue::Integer(value) = heap.fields(object).unwrap()[index] else {
        panic!("integer")
    };
    value
}
fn write(heap: &mut DomainHeap, root: RootSlot, field: usize, cells: bool, value: i64) {
    let (object, index) = location(heap, root, field, cells);
    update(heap, object, index, HeapValue::Integer(value));
}
fn edit_callback(heap: &mut DomainHeap, root: RootSlot, cells: bool) {
    write(heap, root, 0, cells, read(heap, root, 0, cells) + 1);
    write(heap, root, 2, cells, read(heap, root, 2, cells) + 1);
}
fn select_callback(heap: &mut DomainHeap, root: RootSlot, cells: bool) {
    assert_eq!(read(heap, root, 0, cells), 43);
    write(heap, root, 1, cells, read(heap, root, 1, cells) + 3);
}
#[test]
fn compare_editor_state() {
    for (name, cells) in [
        ("cells", true),
        ("state_object", false),
        ("class_composition", false),
    ] {
        let mut heap = DomainHeap::new(701);
        let initial = [42, 8, 2, 1];
        let fields = if cells {
            initial
                .iter()
                .map(|v| {
                    HeapValue::Reference(
                        heap.allocate(1, &[HeapValue::Integer(*v)], FinalizerPolicy::None)
                            .unwrap(),
                    )
                })
                .collect::<Vec<_>>()
        } else {
            initial.iter().map(|v| HeapValue::Integer(*v)).collect()
        };
        let object = heap.allocate(2, &fields, FinalizerPolicy::None).unwrap();
        let id = heap.header(object).unwrap().object_id;
        let root = heap.create_root(Some(object)).unwrap();
        let alias = heap.create_root(Some(object)).unwrap();
        let objects = heap.object_count();
        let bytes = heap.allocated_bytes();
        edit_callback(&mut heap, root, cells);
        let collection = heap.collect(CollectionKind::Full).unwrap();
        select_callback(&mut heap, alias, cells);
        assert_eq!(
            heap.header(heap.root_target(alias).unwrap().unwrap())
                .unwrap()
                .object_id,
            id
        );
        let values = (0..4)
            .map(|field| read(&heap, root, field, cells))
            .collect::<Vec<_>>();
        assert_eq!(values, vec![43, 11, 3, 1]);
        heap.release_root(root).unwrap();
        heap.release_root(alias).unwrap();
        let reclaimed = heap.collect(CollectionKind::Full).unwrap();
        assert_eq!(heap.object_count(), 0);
        assert!(reclaimed.native_finalizers.is_empty());
        println!(
            "M2_STATE {}",
            serde_json::json!({"candidate":name,"objects":objects,"bytes":bytes,"moved":collection.moved_objects,"values":values,"reclaimed":reclaimed.reclaimed_objects,"identity_preserved":true})
        );
    }
}
