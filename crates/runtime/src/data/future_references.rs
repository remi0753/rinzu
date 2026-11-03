//! Trace Future identities through the typed managed graph and its outcomes.
//! Future records live in the runtime; their scalar identities are not heap refs.
use super::*;
use std::collections::{BTreeMap, BTreeSet, HashSet};

impl ManagedData {
    /// `future_types` comes from the compiler's closed schema. Integer bit
    /// patterns alone never identify a Future. Outcome edges are followed only
    /// after the corresponding Future is reached from a live program root.
    pub fn reachable_futures(
        &self,
        heap: &DomainHeap,
        roots: impl IntoIterator<Item = ManagedHandle>,
        futures: impl IntoIterator<Item = u64>,
        future_types: &BTreeSet<usize>,
        outcomes: &BTreeMap<u64, ManagedHandle>,
    ) -> Result<BTreeSet<u64>, DataError> {
        self.require_domain(heap)?;
        let schema = self.value_schema()?;
        for ty in future_types {
            if !matches!(schema.ty(*ty)?, crate::value::ValueType::Other) {
                return Err(DataError::WrongKind);
            }
        }
        let mut pending = roots
            .into_iter()
            .map(|handle| {
                self.any_reference(heap, handle)
                    .map(|(reference, _)| reference)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut future_work = futures.into_iter().collect::<Vec<_>>();
        let mut visited = HashSet::new();
        let mut reached = BTreeSet::new();
        while !pending.is_empty() || !future_work.is_empty() {
            while let Some(future) = future_work.pop() {
                if reached.insert(future)
                    && let Some(handle) = outcomes.get(&future)
                {
                    pending.push(self.any_reference(heap, *handle)?.0);
                }
            }
            let Some(reference) = pending.pop() else {
                continue;
            };
            if !visited.insert(reference) {
                continue;
            }
            if heap.header(reference)?.type_id == super::composite::COMPOSITE_TYPE {
                let mut encoded = heap.fields_iter(reference)?;
                let Some(HeapValue::Integer(ty)) = encoded.next() else {
                    return Err(DataError::InvalidEncoding);
                };
                let ty = usize::try_from(ty).map_err(|_| DataError::InvalidEncoding)?;
                let (variant, fields) = self.composite_fields_iter(heap, reference, ty)?;
                let types = super::composite::child_types(schema.ty(ty)?, variant, fields.len())?;
                for (ty, value) in types.iter().zip(fields) {
                    match value {
                        HeapValue::Reference(child) => pending.push(child),
                        HeapValue::Integer(bits) if future_types.contains(&ty) => {
                            future_work.push(bits as u64)
                        }
                        _ => {}
                    }
                }
            } else {
                for value in heap.fields_iter(reference)? {
                    if let HeapValue::Reference(child) = value {
                        pending.push(child);
                    }
                }
            }
        }
        Ok(reached)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heap::CollectionKind;
    use crate::scalar::{ScalarKind, ScalarValue};
    use crate::value::{ValueSchema, ValueType};
    use std::sync::Arc;

    #[test]
    fn traces_typed_future_fields_outcomes_and_cycles_without_retaining_unreachable_results() {
        let mut heap = DomainHeap::new(391);
        let mut data = ManagedData::new(391);
        let schema = Arc::new(
            ValueSchema::new(vec![
                ValueType::Scalar(ScalarKind::U64),
                ValueType::Other,
                ValueType::State {
                    identity: "Outcome".into(),
                    fields: vec![1, 0, 3],
                },
                ValueType::List(1),
                ValueType::Record {
                    identity: "Holder".into(),
                    fields: vec![1],
                },
                ValueType::State {
                    identity: "Cycle".into(),
                    fields: vec![6, 1],
                },
                ValueType::List(5),
            ])
            .unwrap(),
        );
        data.configure_values(&heap, schema).unwrap();
        let empty = data.create_composite(&mut heap, 3, None, &[]).unwrap();
        let children = data
            .create_composite(&mut heap, 3, None, &[ManagedSlot::Opaque(43)])
            .unwrap();
        let number = ManagedSlot::Scalar(ScalarValue::from_integer(ScalarKind::U64, 999).unwrap());
        let first = data
            .create_composite(
                &mut heap,
                2,
                None,
                &[
                    ManagedSlot::Opaque(42),
                    number,
                    ManagedSlot::Reference(children),
                ],
            )
            .unwrap();
        let second = data
            .create_composite(
                &mut heap,
                2,
                None,
                &[
                    ManagedSlot::Opaque(41),
                    number,
                    ManagedSlot::Reference(empty),
                ],
            )
            .unwrap();
        let dead = data
            .create_composite(
                &mut heap,
                2,
                None,
                &[
                    ManagedSlot::Opaque(45),
                    number,
                    ManagedSlot::Reference(empty),
                ],
            )
            .unwrap();
        let holder = data
            .create_composite(&mut heap, 4, None, &[ManagedSlot::Opaque(41)])
            .unwrap();
        let outcomes = BTreeMap::from([(41, first), (42, second), (44, dead)]);
        let future_types = BTreeSet::from([1]);
        for _ in 0..2 {
            assert_eq!(
                data.reachable_futures(&heap, [holder], [], &future_types, &outcomes)
                    .unwrap(),
                BTreeSet::from([41, 42, 43])
            );
            assert!(
                data.reachable_futures(&heap, [], [], &future_types, &outcomes)
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                data.reachable_futures(&heap, [], [44], &future_types, &outcomes)
                    .unwrap(),
                BTreeSet::from([44, 45])
            );
            heap.collect(CollectionKind::Full).unwrap();
        }
        let empty_cycle = data.create_composite(&mut heap, 6, None, &[]).unwrap();
        let cycle = data
            .create_composite(
                &mut heap,
                5,
                None,
                &[ManagedSlot::Reference(empty_cycle), ManagedSlot::Opaque(43)],
            )
            .unwrap();
        let link = data
            .create_composite(&mut heap, 6, None, &[ManagedSlot::Reference(cycle)])
            .unwrap();
        data.set_state_field(&mut heap, cycle, 5, 0, ManagedSlot::Reference(link))
            .unwrap();
        assert_eq!(
            data.reachable_futures(&heap, [cycle], [], &future_types, &outcomes)
                .unwrap(),
            BTreeSet::from([43])
        );
        assert!(
            data.reachable_futures(&heap, [holder], [], &BTreeSet::new(), &outcomes)
                .unwrap()
                .is_empty()
        );
    }
}
