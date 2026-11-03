//! Signature-specific physical ABI calls shared by JIT and native AOT objects.
//! The internal word array is private to the runtime. The external call retains
//! exactly the source declaration's C parameters and return convention.
use std::collections::BTreeMap;

use anyhow::{Context, Result, anyhow, ensure};
use cranelift_codegen::ir::{AbiParam, InstBuilder, MemFlagsData, Signature, Value, types};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module, default_libcall_names};
use cranelift_object::{ObjectBuilder, ObjectModule};
use rinzu_compiler::{AbiScalar, SourceAbiType, SourceNativeSignature};

fn physical_type(ty: &SourceAbiType) -> Result<AbiParam> {
    use AbiScalar::*;
    Ok(match ty {
        SourceAbiType::Unit => return Err(anyhow!("Unit has no physical parameter")),
        SourceAbiType::Scalar(scalar) => match scalar {
            I8 => AbiParam::new(types::I8).sext(),
            I16 => AbiParam::new(types::I16).sext(),
            I32 => AbiParam::new(types::I32).sext(),
            I64 => AbiParam::new(types::I64),
            U8 | Bool => AbiParam::new(types::I8).uext(),
            U16 => AbiParam::new(types::I16).uext(),
            U32 => AbiParam::new(types::I32).uext(),
            U64 => AbiParam::new(types::I64),
            F32 => AbiParam::new(types::F32),
            F64 => AbiParam::new(types::F64),
        },
        _ => AbiParam::new(types::I64),
    })
}

fn decode(builder: &mut FunctionBuilder, ty: &SourceAbiType, bits: Value) -> Result<Value> {
    let physical = physical_type(ty)?.value_type;
    Ok(if physical == types::F64 {
        builder.ins().bitcast(types::F64, MemFlagsData::new(), bits)
    } else if physical == types::F32 {
        let bits = builder.ins().ireduce(types::I32, bits);
        builder.ins().bitcast(types::F32, MemFlagsData::new(), bits)
    } else if physical == types::I64 {
        bits
    } else {
        builder.ins().ireduce(physical, bits)
    })
}

fn encode(builder: &mut FunctionBuilder, ty: &SourceAbiType, value: Value) -> Result<Value> {
    let physical = physical_type(ty)?.value_type;
    Ok(if physical == types::F64 {
        builder
            .ins()
            .bitcast(types::I64, MemFlagsData::new(), value)
    } else if physical == types::F32 {
        let bits = builder
            .ins()
            .bitcast(types::I32, MemFlagsData::new(), value);
        builder.ins().uextend(types::I64, bits)
    } else if physical == types::I64 {
        value
    } else if matches!(
        ty,
        SourceAbiType::Scalar(AbiScalar::I8 | AbiScalar::I16 | AbiScalar::I32)
    ) {
        builder.ins().sextend(types::I64, value)
    } else {
        builder.ins().uextend(types::I64, value)
    })
}

pub(crate) fn declare<M: Module>(
    module: &mut M,
    signatures: &[SourceNativeSignature],
) -> Result<BTreeMap<String, (FuncId, String)>> {
    let mut functions = BTreeMap::new();
    let mut signatures = signatures
        .iter()
        .filter(|signature| !signature.is_async)
        .collect::<Vec<_>>();
    signatures.sort_by(|a, b| a.name.cmp(&b.name));
    for (index, source) in signatures.into_iter().enumerate() {
        ensure!(
            !functions.contains_key(&source.name),
            "duplicate source native signature"
        );
        let symbol = format!("rinzu_source_native_{index}");
        let mut signature = module.make_signature();
        signature.params.extend([AbiParam::new(types::I64); 2]);
        signature.returns.push(AbiParam::new(types::I64));
        let function = module.declare_function(&symbol, Linkage::Export, &signature)?;
        let mut context = module.make_context();
        context.func.signature = signature;
        let mut builder_context = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let address = builder.block_params(entry)[0];
        let words = builder.block_params(entry)[1];
        let mut native = Signature::new(module.target_config().default_call_conv);
        let mut arguments = Vec::new();
        for (parameter, ty) in source.parameters.iter().enumerate() {
            native.params.push(physical_type(ty)?);
            let offset = i32::try_from(
                parameter
                    .checked_mul(8)
                    .ok_or_else(|| anyhow!("native parameter count overflow"))?,
            )?;
            let bits = builder
                .ins()
                .load(types::I64, MemFlagsData::trusted(), words, offset);
            arguments.push(decode(&mut builder, ty, bits)?);
        }
        if source.result != SourceAbiType::Unit {
            native.returns.push(physical_type(&source.result)?);
        }
        let native = builder.import_signature(native);
        let call = builder.ins().call_indirect(native, address, &arguments);
        let result = if source.result == SourceAbiType::Unit {
            builder.ins().iconst(types::I64, 0)
        } else {
            let value = builder.inst_results(call)[0];
            encode(&mut builder, &source.result, value)?
        };
        builder.ins().return_(&[result]);
        builder.finalize(module.target_config());
        module.define_function(function, &mut context)?;
        functions.insert(source.name.clone(), (function, symbol));
    }
    Ok(functions)
}

pub struct NativeAbiObject {
    pub object: Vec<u8>,
    pub symbols: BTreeMap<String, String>,
}

pub fn emit_object(signatures: &[SourceNativeSignature]) -> Result<NativeAbiObject> {
    let isa = crate::native_isa(true)?;
    let builder = ObjectBuilder::new(isa, "rinzu-source-native", default_libcall_names())?;
    let mut module = ObjectModule::new(builder);
    let functions = declare(&mut module, signatures)?;
    Ok(NativeAbiObject {
        object: module.finish().emit()?,
        symbols: functions
            .into_iter()
            .map(|(name, (_, symbol))| (name, symbol))
            .collect(),
    })
}

/// Owned by the compiling thread. Each physical Worker can compile its own
/// immutable stubs without sharing compiler state or touching another heap.
pub struct NativeAbiStubs {
    module: JITModule,
    functions: BTreeMap<String, (FuncId, usize)>,
}

impl NativeAbiStubs {
    pub fn new(signatures: &[SourceNativeSignature]) -> Result<Self> {
        let mut builder = JITBuilder::with_isa(crate::native_isa(false)?, default_libcall_names());
        let (memory, audit) = crate::macos_jit_memory::MacOsArm64JitMemory::new_with_audit()?;
        builder.memory_provider(Box::new(memory));
        let mut module = JITModule::new(builder);
        let functions = declare(&mut module, signatures)?;
        module
            .finalize_definitions()
            .context("finalize source native ABI stubs")?;
        let audit = audit.snapshot();
        ensure!(
            audit.rwx_mappings == 0
                && audit.executable_mappings == audit.rx_transitions
                && audit.executable_mappings == audit.icache_flushes,
            "source native stubs violated W^X publication"
        );
        let functions = functions
            .into_iter()
            .map(|(name, (id, _))| {
                let count = signatures
                    .iter()
                    .find(|signature| signature.name == name)
                    .unwrap()
                    .parameters
                    .len();
                (name, (id, count))
            })
            .collect();
        Ok(Self { module, functions })
    }

    /// # Safety
    /// `address` is a live entry with exactly this checked C signature. Every word
    /// already represents its physical scalar/pointer argument and pins the
    /// pointed storage/admitted callback throughout the synchronous native call.
    pub unsafe fn invoke(&self, name: &str, address: usize, words: &[u64]) -> Result<u64> {
        let (function, count) = self
            .functions
            .get(name)
            .ok_or_else(|| anyhow!("missing source native stub {name}"))?;
        ensure!(
            address != 0 && words.len() == *count,
            "invalid source native entry/argument count"
        );
        // SAFETY: the generated private entry is exactly (address, word array)->u64.
        let call: unsafe extern "C" fn(usize, *const u64) -> u64 =
            unsafe { std::mem::transmute(self.module.get_finalized_function(*function)) };
        // SAFETY: caller guarantees the physical signature and live argument storage.
        Ok(unsafe { call(address, words.as_ptr()) })
    }
}
