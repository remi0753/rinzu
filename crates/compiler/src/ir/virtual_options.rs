//! A bounded representation plan for a verified, nonescaping bytesSlice Option.
//!
//! This leaves semantic IR, safepoints and logical allocation sites intact. A C
//! backend may hold the managed ByteArray handle (or zero for None) in the same
//! reference root slots. No scalar payload or observable Option can use it.

use super::{IrFunction, OperationKind, Terminator, ValueId, operation_inputs, payload_is_guarded};
use crate::Type;
use std::collections::HashSet;

#[must_use]
pub fn virtual_bytes_options(function: &IrFunction) -> HashSet<ValueId> {
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
        return HashSet::new();
    }
    let mut plan = HashSet::new();
    for producer in function.blocks.iter().flat_map(|b| &b.operations) {
        let OperationKind::DataOperation {
            kind: crate::DataOperation::BytesSlice,
            argument_types,
            ..
        } = &producer.kind
        else {
            continue;
        };
        if argument_types.as_slice() != [Type::Bytes, Type::Int, Type::Int] {
            continue;
        }
        let Some(result) = &producer.result else {
            continue;
        };
        let Type::Named(identity) = &result.ty else {
            continue;
        };
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
                            && op.result.as_ref().is_some_and(|v| v.ty == Type::Bytes)
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
            plan.extend(values);
        }
    }
    plan
}
