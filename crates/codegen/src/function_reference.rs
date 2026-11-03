//! Module-local identities are values; executable addresses are resolved only
//! when entering a known typed target on the current Domain.
use anyhow::{Result, anyhow};
use rinzu_compiler::{CoreLengthKind, IrModule, OperationKind};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn core_length(target: &str) -> Option<(CoreLengthKind, Option<u64>)> {
    if target == "rinzu.core::utf8ByteLength" {
        return Some((CoreLengthKind::Utf8, None));
    }
    let spelling = target.strip_prefix("rinzu.core::byteLength:")?;
    if spelling == "dynamic" {
        return Some((CoreLengthKind::Bytes, Some(u64::MAX)));
    }
    let length = spelling.parse::<u64>().ok()?;
    (spelling == length.to_string()).then_some((CoreLengthKind::Bytes, Some(length)))
}

#[derive(Clone)]
pub(crate) struct ReferenceTargets(BTreeMap<String, i64>);
impl ReferenceTargets {
    pub fn new(module: &IrModule) -> Result<Self> {
        let mut targets = module
            .functions
            .iter()
            .map(|function| function.name.clone())
            .collect::<BTreeSet<_>>();
        for operation in module
            .functions
            .iter()
            .flat_map(|function| &function.blocks)
            .flat_map(|block| &block.operations)
        {
            match &operation.kind {
                OperationKind::FunctionReference { target } => {
                    targets.insert(target.clone());
                }
                OperationKind::IndirectCall {
                    targets: possible, ..
                }
                | OperationKind::ReserveSupplant {
                    targets: possible, ..
                } => targets.extend(possible.iter().cloned()),
                OperationKind::PrepareNativeCall { callbacks, .. } => {
                    for callback in callbacks {
                        targets.extend(callback.targets.iter().cloned());
                    }
                }
                _ => {}
            }
        }
        Ok(Self(
            targets
                .into_iter()
                .enumerate()
                .map(|(index, target)| {
                    Ok((
                        target,
                        i64::try_from(index)?
                            .checked_add(1)
                            .ok_or_else(|| anyhow!("function reference identity overflow"))?,
                    ))
                })
                .collect::<Result<_>>()?,
        ))
    }
    pub fn identity(&self, target: &str) -> Result<i64> {
        self.0
            .get(target)
            .copied()
            .ok_or_else(|| anyhow!("unknown function reference target {target}"))
    }
    pub fn metadata(&self) -> Vec<String> {
        self.0.keys().cloned().collect()
    }
}
