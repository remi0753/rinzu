//! Actual heap layout comparison, before adopting closure production lowering.
use super::*;
#[test]
fn compare_closure_environment_layouts_and_unrooted_code_counterexample() {
    for (candidate, converted) in [("flat_descriptor", false), ("typed_conversion", true)] {
        let mut heap = DomainHeap::new(710);
        let object = if converted {
            let environment = heap
                .allocate(
                    1,
                    &[
                        HeapValue::Integer(1),
                        HeapValue::Integer(0),
                        HeapValue::Integer(42),
                    ],
                    FinalizerPolicy::None,
                )
                .unwrap();
            heap.allocate(
                2,
                &[
                    HeapValue::Integer(2),
                    HeapValue::Integer(0),
                    HeapValue::Reference(environment),
                ],
                FinalizerPolicy::None,
            )
            .unwrap()
        } else {
            heap.allocate(
                3,
                &[
                    HeapValue::Integer(3),
                    HeapValue::Integer(0),
                    HeapValue::Integer(1),
                    HeapValue::Integer(42),
                ],
                FinalizerPolicy::None,
            )
            .unwrap()
        };
        let root = heap.create_root(Some(object)).unwrap();
        let bytes = heap.allocated_bytes();
        let objects = heap.object_count();
        let moved = heap.collect(CollectionKind::Full).unwrap().moved_objects;
        assert_eq!(moved, objects);
        heap.release_root(root).unwrap();
        heap.collect(CollectionKind::Full).unwrap();
        assert_eq!(heap.object_count(), 0);
        println!(
            "M2_FUNCTION {}",
            serde_json::json!({"candidate":candidate,"objects":objects,"bytes":bytes,"moved":moved,"reclaimed":objects})
        );
    }
    let mut heap = DomainHeap::new(711);
    let environment = heap
        .allocate(1, &[HeapValue::Integer(42)], FinalizerPolicy::None)
        .unwrap();
    let collected = heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(collected.reclaimed_objects, 1);
    assert!(heap.fields(environment).is_err());
    println!(
        "M2_FUNCTION {}",
        serde_json::json!({"candidate":"code_without_root","code_identity":1,"environment_reclaimed":true,"usable_after_scope":false})
    );
}
