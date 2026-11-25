//! Representation plan for Options consumed by a verified immediate switch.

use super::{IrFunction, OperationKind, Terminator, ValueId, operation_inputs, payload_is_guarded};
use crate::{AbiScalar, DataOperation, Type};
use std::collections::HashSet;

#[derive(Clone, Default)]
pub struct VirtualOptions {
    pub values: HashSet<ValueId>,
    /// These Option-typed IR roots contain scalar bits in the C backend.
    pub scalar_values: HashSet<ValueId>,
    pub scalar_locals: HashSet<String>,
}

#[must_use]
pub fn virtual_options(function: &IrFunction) -> VirtualOptions {
    plan(function, true)
}

#[must_use]
pub fn virtual_bytes_options(function: &IrFunction) -> HashSet<ValueId> {
    plan(function, false).values
}

fn plan(function: &IrFunction, indexed: bool) -> VirtualOptions {
    // Start with closed leaf bodies: no task/native boundary can retain a
    // compiler temporary using the original Option schema during suspension.
    if function.is_async
        || function
            .blocks
            .iter()
            .flat_map(|b| &b.operations)
            .any(|op| {
                matches!(
                    op.kind,
                    OperationKind::ManagedCall { .. }
                        | OperationKind::IndirectCall { .. }
                        | OperationKind::NativeCall { .. }
                        | OperationKind::SourceNativeCall { .. }
                        | OperationKind::PrepareNativeCall { .. }
                        | OperationKind::ReserveSupplant { .. }
                        | OperationKind::Await { .. }
                        | OperationKind::Spawn { .. }
                        | OperationKind::TaskControl { .. }
                        | OperationKind::Stdout { .. }
                )
            })
    {
        return VirtualOptions::default();
    }
    let mut plan = VirtualOptions::default();
    for producer in function.blocks.iter().flat_map(|b| &b.operations) {
        let OperationKind::DataOperation {
            kind,
            argument_types,
            ..
        } = &producer.kind
        else {
            continue;
        };
        let payload = match (kind, argument_types.as_slice()) {
            (DataOperation::BytesSlice, [Type::Bytes, Type::Int, Type::Int]) => Type::Bytes,
            (DataOperation::BytesGet, [Type::Bytes, Type::Int]) if indexed => {
                Type::Scalar(AbiScalar::U8)
            }
            (DataOperation::ListGet, [Type::List(element), Type::Int])
                if indexed && !matches!(element.as_ref(), Type::Future(_)) =>
            {
                element.as_ref().clone()
            }
            _ => continue,
        };
        let Some(result) = &producer.result else {
            continue;
        };
        let Type::Named(identity) = &result.ty else {
            continue;
        };
        if *identity != crate::core::option_name(&payload) {
            continue;
        }
        let saved = format!("$switch{}", result.id.0);
        let writes: Vec<_> = function
            .blocks
            .iter()
            .flat_map(|b| &b.operations)
            .filter_map(|op| match &op.kind {
                OperationKind::StoreLocal { name, value } if name == &saved => Some(*value),
                _ => None,
            })
            .collect();
        if writes != [result.id] {
            continue;
        }
        let mut values = HashSet::from([result.id]);
        let mut valid = true;
        for op in function.blocks.iter().flat_map(|b| &b.operations) {
            if let OperationKind::LoadLocal { name } = &op.kind
                && name == &saved
            {
                if let Some(loaded) = &op.result
                    && loaded.ty == result.ty
                {
                    values.insert(loaded.id);
                } else {
                    valid = false;
                }
            }
        }
        let mut switches = 0;
        for block in &function.blocks {
            for op in &block.operations {
                if !operation_inputs(&op.kind)
                    .iter()
                    .any(|id| values.contains(id))
                {
                    continue;
                }
                let allowed = match &op.kind {
                    OperationKind::StoreLocal { name, value } => {
                        name == &saved && *value == result.id
                    }
                    OperationKind::EnumPayload {
                        operand,
                        identity: owner,
                        variant: 1,
                        field: 0,
                    } => {
                        *operand != result.id
                            && owner == identity
                            && op.result.as_ref().is_some_and(|v| v.ty == payload)
                            && payload_is_guarded(function, block, *operand, owner, 1)
                    }
                    _ => false,
                };
                valid &= allowed;
            }
            match &block.terminator {
                Terminator::Switch {
                    value,
                    identity: owner,
                    cases,
                } if values.contains(value) => {
                    valid &= *value == result.id
                        && owner == identity
                        && cases.len() == 2
                        && cases.iter().any(|(tag, _)| *tag == 0)
                        && cases.iter().any(|(tag, _)| *tag == 1);
                    switches += 1;
                }
                Terminator::Return {
                    value: Some(value), ..
                } if values.contains(value) => valid = false,
                Terminator::Branch { condition, .. } if values.contains(condition) => valid = false,
                _ => {}
            }
        }
        if valid && switches == 1 {
            if !payload.is_managed_reference() {
                plan.scalar_values.extend(values.iter().copied());
                plan.scalar_locals.insert(saved);
            }
            plan.values.extend(values);
        }
    }
    plan
}
