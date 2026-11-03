//! Checked per-call-site plans shared by compiled JIT and AOT lowering.
use std::collections::BTreeMap;

use anyhow::{Result, anyhow, ensure};
use rinzu_compiler::{
    CallbackArgumentPlan, IrModule, NativeCallId, OperationKind, SourceAbiType,
    SourceNativeSignature, Type,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeCallPlan {
    pub function: usize,
    pub call: NativeCallId,
    pub signature: SourceNativeSignature,
    pub callbacks: Vec<CallbackArgumentPlan>,
}

/// Logical managed type paired with one validated native parameter/result.
pub(crate) fn callback_type(ty: &SourceAbiType) -> Result<Type> {
    Ok(match ty {
        SourceAbiType::Unit => Type::Unit,
        SourceAbiType::Scalar(scalar) => Type::from_scalar(*scalar),
        SourceAbiType::String => Type::String,
        SourceAbiType::Bytes(length) => Type::FixedBytes(*length),
        SourceAbiType::Opaque(name) => Type::Native {
            name: name.clone(),
            call_only: false,
        },
        SourceAbiType::Record(name) | SourceAbiType::UserData(name) => Type::Named(name.clone()),
        _ => anyhow::bail!("invalid managed callback ABI type"),
    })
}

pub(crate) fn scalar_fallback(
    fallback: Option<&rinzu_compiler::FallbackConstant>,
) -> Result<Option<u64>> {
    use rinzu_compiler::FallbackConstant as F;
    Ok(match fallback {
        None => None,
        Some(F::Unit) => Some(0),
        Some(F::Integer { scalar, value }) => Some(
            rinzu_runtime::scalar::ScalarValue::from_integer(
                crate::scalar_backend::kind(*scalar),
                *value,
            )
            .map_err(|error| anyhow!("invalid callback fallback: {error:?}"))?
            .bits(),
        ),
        Some(F::Float { scalar, bits }) => Some(
            rinzu_runtime::scalar::ScalarValue::from_bits(
                crate::scalar_backend::kind(*scalar),
                *bits,
            )
            .map_err(|error| anyhow!("invalid callback fallback: {error:?}"))?
            .bits(),
        ),
        Some(F::Bool(value)) => Some(u64::from(*value)),
        _ => anyhow::bail!("callback buffer fallback requires the R3 process arena"),
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) fn callback_fallback(
    fallback: Option<&rinzu_compiler::FallbackConstant>,
    owner: Option<&std::sync::Arc<rinzu_runtime::callback_return::ReturnScope>>,
) -> Result<(
    Option<u64>,
    Option<rinzu_runtime::callback_return::ProducedBuffer>,
)> {
    use rinzu_compiler::FallbackConstant as F;
    use rinzu_runtime::callback_return::ProducedBuffer;
    let buffer = match fallback {
        Some(F::String(text)) => Some(ProducedBuffer::string(text, true, owner)),
        Some(F::Bytes { prefix, length }) => {
            Some(ProducedBuffer::bytes(prefix, *length, true, owner))
        }
        _ => None,
    };
    Ok((
        if let Some(buffer) = &buffer {
            Some(buffer.bits())
        } else {
            scalar_fallback(fallback)?
        },
        buffer,
    ))
}

#[derive(Clone, Debug, Default)]
pub(crate) struct NativeCallPlans {
    pub records: Vec<NativeCallPlan>,
    indices: BTreeMap<(usize, u32), usize>,
    supplants: BTreeMap<(usize, u32), Vec<usize>>,
}
impl NativeCallPlans {
    pub fn new(module: &IrModule) -> Result<Self> {
        let mut result = Self::default();
        for (function, body) in module.functions.iter().enumerate() {
            for operation in body.blocks.iter().flat_map(|block| &block.operations) {
                if let OperationKind::PrepareNativeCall {
                    call,
                    signature,
                    callbacks,
                    ..
                } = &operation.kind
                {
                    ensure!(
                        !signature.is_async && !signature.is_supplant,
                        "ordinary native plan required"
                    );
                    ensure!(
                        module.source_native_signatures.contains(signature),
                        "native call plan lost its module signature"
                    );
                    ensure!(
                        result
                            .indices
                            .insert((function, call.0), result.records.len())
                            .is_none(),
                        "duplicate native call plan"
                    );
                    result.records.push(NativeCallPlan {
                        function,
                        call: *call,
                        signature: signature.clone(),
                        callbacks: callbacks.clone(),
                    });
                }
                if let OperationKind::ReserveSupplant {
                    reservation,
                    targets,
                    callbacks,
                    ..
                } = &operation.kind
                {
                    let mut plans = Vec::new();
                    for target in targets {
                        let signature = module
                            .source_native_signatures
                            .iter()
                            .find(|signature| signature.name == *target)
                            .ok_or_else(|| anyhow!("supplant target lost its native signature"))?;
                        ensure!(
                            signature.is_supplant && !signature.is_async,
                            "supplant plan requires its closed effect"
                        );
                        let callbacks = callbacks.clone();
                        plans.push(result.records.len());
                        result.records.push(NativeCallPlan {
                            function,
                            call: NativeCallId(reservation.0),
                            signature: signature.clone(),
                            callbacks,
                        });
                    }
                    ensure!(
                        result
                            .supplants
                            .insert((function, reservation.0), plans)
                            .is_none(),
                        "duplicate supplant reservation plan"
                    );
                }
            }
        }
        Ok(result)
    }
    pub fn index(&self, function: usize, call: NativeCallId) -> Result<usize> {
        self.indices
            .get(&(function, call.0))
            .copied()
            .ok_or_else(|| anyhow!("missing native call plan"))
    }
    pub fn supplant(
        &self,
        function: usize,
        reservation: rinzu_compiler::ReservationId,
    ) -> Result<&[usize]> {
        self.supplants
            .get(&(function, reservation.0))
            .map(Vec::as_slice)
            .ok_or_else(|| anyhow!("missing supplant reservation plan"))
    }
}
