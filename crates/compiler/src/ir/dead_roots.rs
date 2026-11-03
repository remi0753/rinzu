//! Independent C-backend plans for dead temporary and ordinary local roots.
//! Semantic IR stays conservative; every safepoint and poll remains in place.
use super::{BlockId, IrFunction, OperationKind, StackRoot, Terminator, ValueId, operation_inputs};
use crate::Type;
use std::collections::{HashMap, HashSet};

/// `(block, operation index, value)` entries may be omitted at that safepoint.
/// Verified IR forbids SSA uses across blocks; cross-block and cleanup values
/// live in locals, which this first bounded plan never removes.
#[must_use]
pub fn dead_temporary_roots(function: &IrFunction) -> HashSet<(BlockId, usize, ValueId)> {
    if function.is_async
        || function
            .blocks
            .iter()
            .flat_map(|b| &b.operations)
            .any(|op| {
                matches!(
                    op.kind,
                    OperationKind::IndirectCall { .. }
                        | OperationKind::NativeCall { .. }
                        | OperationKind::SourceNativeCall { .. }
                        | OperationKind::PrepareNativeCall { .. }
                        | OperationKind::ReserveSupplant { .. }
                        | OperationKind::Await { .. }
                        | OperationKind::Spawn { .. }
                        | OperationKind::TaskControl { .. }
                ) || matches!(op.kind, OperationKind::ManagedCall { .. })
                    && op
                        .result
                        .as_ref()
                        .is_some_and(|v| matches!(v.ty, Type::Future(_)))
            })
    {
        return HashSet::new();
    }
    let mut plan = HashSet::new();
    for block in &function.blocks {
        let mut live = HashSet::new();
        match &block.terminator {
            Terminator::Return { value, .. } => live.extend(value),
            Terminator::Branch { condition, .. } => {
                live.insert(*condition);
            }
            Terminator::Switch { value, .. } => {
                live.insert(*value);
            }
            _ => {}
        }
        for (index, op) in block.operations.iter().enumerate().rev() {
            if let OperationKind::GcSafepoint { stack_map, .. } = &op.kind {
                for root in &stack_map.roots {
                    if let StackRoot::Value { value, ty } = root
                        && !live.contains(value)
                        // Keep logical Future/Error/native lifetime protocols
                        // outside this first ordinary-data temporary plan.
                        && matches!(ty, Type::String | Type::Bytes | Type::FixedBytes(_)
                            | Type::Named(_) | Type::List(_) | Type::Map(_, _))
                    {
                        plan.insert((block.id, index, *value));
                    }
                }
            }
            if let Some(result) = &op.result {
                live.remove(&result.id);
            }
            // Stack maps themselves are observations, not semantic SSA uses.
            // This shared input catalog also verifies block-local dominance.
            live.extend(operation_inputs(&op.kind));
        }
    }
    plan
}

/// A bounded, independent local-root plan. SSA operands remain governed by the
/// temporary plan above. Solve the verified CFG to a fixed point, including
/// operation Error/failure edges at their exact operation, so a later store
/// cannot kill a value read by an earlier exceptional successor.
///
/// Explicit cleanup routes and async/native/indirect/child bodies keep all
/// locals. Ordinary data only: Future/Error/native lifetimes are unchanged.
#[must_use]
pub fn dead_local_roots(function: &IrFunction) -> HashSet<(BlockId, usize, String)> {
    use super::{operation_error_edge, operation_failure_target};
    if function.is_async
        || function
            .scopes
            .iter()
            .any(|s| !s.deferred_finally.is_empty())
        || function.blocks.iter().any(|b| {
            matches!(&b.terminator,
                Terminator::Goto { cleanup_chain, .. }
                | Terminator::Return { cleanup_chain, .. } if !cleanup_chain.is_empty())
                || b.operations.iter().any(|op| {
                    op.unwind.as_ref().is_some_and(|u| !u.finally.is_empty())
                        || matches!(
                            op.kind,
                            OperationKind::EnterTry { .. }
                                | OperationKind::LeaveTry { .. }
                                | OperationKind::IndirectCall { .. }
                                | OperationKind::NativeCall { .. }
                                | OperationKind::SourceNativeCall { .. }
                                | OperationKind::PrepareNativeCall { .. }
                                | OperationKind::ReserveSupplant { .. }
                                | OperationKind::Await { .. }
                                | OperationKind::Spawn { .. }
                                | OperationKind::TaskControl { .. }
                        )
                        || op
                            .result
                            .as_ref()
                            .is_some_and(|v| matches!(v.ty, Type::Future(_)))
                })
        })
    {
        return HashSet::new();
    }
    let mut entries: HashMap<BlockId, HashSet<String>> = function
        .blocks
        .iter()
        .map(|b| (b.id, HashSet::new()))
        .collect();
    let successors = |term: &Terminator| -> Vec<BlockId> {
        match term {
            Terminator::Goto { target, .. } => vec![*target],
            Terminator::Branch {
                then_target,
                else_target,
                ..
            } => vec![*then_target, *else_target],
            Terminator::Switch { cases, .. } => cases.iter().map(|(_, target)| *target).collect(),
            _ => Vec::new(),
        }
    };
    let transfer = |op: &super::Operation,
                    live: &mut HashSet<String>,
                    entries: &HashMap<BlockId, HashSet<String>>| {
        match &op.kind {
            OperationKind::StoreLocal { name, .. } | OperationKind::DeclareLocal { name } => {
                live.remove(name);
            }
            OperationKind::LoadLocal { name } | OperationKind::CheckLocal { name, .. } => {
                live.insert(name.clone());
            }
            _ => {}
        }
        // These edges leave before any later store in this block. Their union
        // belongs here, not at the block terminator or after the kill set.
        for target in operation_failure_target(&op.kind)
            .into_iter()
            .chain(operation_error_edge(&op.kind).map(|e| e.target))
        {
            if let Some(values) = entries.get(&target) {
                live.extend(values.iter().cloned());
            }
        }
    };
    loop {
        let mut changed = false;
        for block in function.blocks.iter().rev() {
            let mut live = HashSet::new();
            for target in successors(&block.terminator) {
                if let Some(values) = entries.get(&target) {
                    live.extend(values.iter().cloned());
                }
            }
            for op in block.operations.iter().rev() {
                transfer(op, &mut live, &entries);
            }
            if entries[&block.id] != live {
                entries.insert(block.id, live);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut plan = HashSet::new();
    for block in &function.blocks {
        let mut live = HashSet::new();
        for target in successors(&block.terminator) {
            if let Some(values) = entries.get(&target) {
                live.extend(values.iter().cloned());
            }
        }
        for (index, op) in block.operations.iter().enumerate().rev() {
            if let OperationKind::GcSafepoint { stack_map, .. } = &op.kind {
                // The immediately preceding cancellation poll can take an
                // exceptional route before the point; explicit cleanup bodies
                // are excluded above. No runtime/native hidden local reads.
                for root in &stack_map.roots {
                    if let StackRoot::Local { name, ty } = root
                        && !live.contains(name)
                        && matches!(
                            ty,
                            Type::String
                                | Type::Bytes
                                | Type::FixedBytes(_)
                                | Type::Named(_)
                                | Type::List(_)
                                | Type::Map(_, _)
                        )
                    {
                        plan.insert((block.id, index, name.clone()));
                    }
                }
            }
            transfer(op, &mut live, &entries);
        }
    }
    plan
}
