//! Whole-function C lowering of the existing word-ABI SSA.
//!
//! Unsupported types/instructions reject the whole function. Source, roots,
//! failure dispatch and polls have already been lowered and remain explicit.
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write;

use anyhow::{Result, anyhow, bail, ensure};
use cranelift_codegen::ir::{
    self, BlockArg, BlockCall, Function, GlobalValue, InstructionData as D, Opcode as O, SigRef,
    Type, Value, condcodes::IntCC, types,
};

#[path = "c_lowering_abi.rs"]
mod abi;

pub(crate) const MAX_INSTRUCTIONS: usize = 4096;

pub(crate) struct Emission {
    pub source: String,
    pub instructions: usize,
}

fn c_type(ty: Type) -> Result<&'static str> {
    Ok(match ty {
        types::I8 => "uint8_t",
        types::I16 => "uint16_t",
        types::I32 => "uint32_t",
        types::I64 => "uint64_t",
        _ => bail!("unsupported C value type {ty}"),
    })
}
fn signed_type(ty: Type) -> Result<&'static str> {
    Ok(match ty {
        types::I8 => "int8_t",
        types::I16 => "int16_t",
        types::I32 => "int32_t",
        types::I64 => "int64_t",
        _ => bail!("unsupported signed C value type {ty}"),
    })
}
fn word(value: i64) -> String {
    format!("UINT64_C(0x{:016x})", value as u64)
}
fn value(f: &Function, v: Value) -> String {
    f.dfg.resolve_aliases(v).to_string()
}
fn address(f: &Function, v: Value, offset: i32) -> String {
    format!(
        "(void *)(uintptr_t)({}+{})",
        value(f, v),
        word(i64::from(offset))
    )
}

fn word_stack_slot(data: &ir::StackSlotData) -> bool {
    data.align_shift >= 3 && data.size > 0 && data.size.is_multiple_of(8)
}

fn stack_word_address(f: &Function, address: Value, offset: i32) -> Option<String> {
    let ir::ValueDef::Result(inst, _) = f.dfg.value_def(f.dfg.resolve_aliases(address)) else {
        return None;
    };
    let D::StackAddr {
        stack_slot,
        offset: base,
        ..
    } = f.dfg.insts[inst]
    else {
        return None;
    };
    let data = &f.sized_stack_slots[stack_slot];
    let offset = u32::try_from(i64::from(i32::from(base)) + i64::from(offset)).ok()?;
    if word_stack_slot(data) && offset < data.size && offset.is_multiple_of(8) {
        Some(format!("{stack_slot}[{}]", offset / 8))
    } else {
        None
    }
}

fn edge(f: &Function, destination: BlockCall, suffix: &str) -> Result<String> {
    let block = destination.block(&f.dfg.value_lists);
    let arguments = destination.args(&f.dfg.value_lists).collect::<Vec<_>>();
    let parameters = f.dfg.block_params(block);
    ensure!(arguments.len() == parameters.len(), "C edge arity mismatch");
    let mut out = String::from("{\n");
    // Capture every input before writing any block parameter (loop swaps).
    for (index, (&parameter, argument)) in parameters.iter().zip(&arguments).enumerate() {
        let BlockArg::Value(argument) = argument else {
            bail!("exceptional C block argument")
        };
        writeln!(
            out,
            "{} edge_{suffix}_{index}={};",
            c_type(f.dfg.value_type(parameter))?,
            value(f, *argument)
        )?;
    }
    for (index, parameter) in parameters.iter().enumerate() {
        writeln!(out, "{}=edge_{suffix}_{index};", value(f, *parameter))?;
    }
    writeln!(out, "goto {block};\n}}")?;
    Ok(out)
}
fn signature(f: &Function, signature: SigRef) -> Result<usize> {
    let sig = &f.dfg.signatures[signature];
    ensure!(
        sig.params.iter().all(|p| p.value_type == types::I64)
            && sig.returns.len() == 1
            && sig.returns[0].value_type == types::I64,
        "non-word C call signature"
    );
    Ok(sig.params.len())
}

pub(crate) fn emit(
    f: &Function,
    symbol: &str,
    imports: &HashMap<ir::FuncRef, String>,
    literals: &HashMap<GlobalValue, (Vec<u8>, i64)>,
    managed_signatures: &HashSet<SigRef>,
    isa: &dyn cranelift_codegen::isa::TargetIsa,
) -> Result<Emission> {
    cranelift_codegen::verify_function(f, isa)
        .map_err(|error| anyhow!("invalid lowered SSA: {error}"))?;
    ensure!(
        !f.signature.params.is_empty()
            && f.signature
                .params
                .iter()
                .all(|p| p.value_type == types::I64)
            && f.signature.returns.len() == 1
            && f.signature.returns[0].value_type == types::I64,
        "non-word managed C signature"
    );
    let instructions = f
        .layout
        .blocks()
        .map(|b| f.layout.block_insts(b).count())
        .sum();
    ensure!(instructions <= MAX_INSTRUCTIONS, "C instruction budget");
    let entry = f
        .layout
        .entry_block()
        .ok_or_else(|| anyhow!("no C entry block"))?;
    let mut body = String::new();
    write!(body, "int64_t {symbol}(void *c_context")?;
    for i in 1..f.signature.params.len() {
        write!(body, ", int64_t c_arg_{i}")?;
    }
    writeln!(body, ") {{")?;
    for v in f.dfg.values() {
        if f.dfg.resolve_aliases(v) == v {
            writeln!(
                body,
                "{} {v} __attribute__((unused));",
                c_type(f.dfg.value_type(v))?
            )?;
        }
    }
    for (slot, data) in f.sized_stack_slots.iter() {
        ensure!(data.align_shift <= 16, "C stack alignment budget");
        // C runtime helpers read managed argument arrays as uint64_t words.
        let (element, count) = if word_stack_slot(data) {
            ("uint64_t", data.size / 8)
        } else {
            ("unsigned char", data.size.max(1))
        };
        writeln!(
            body,
            "_Alignas({}) {element} {slot}[{count}] __attribute__((unused));",
            1u32 << data.align_shift
        )?;
    }
    let parameters = f.dfg.block_params(entry);
    ensure!(
        parameters.len() == f.signature.params.len(),
        "C entry arity"
    );
    writeln!(
        body,
        "{}=(uint64_t)(uintptr_t)c_context;",
        value(f, parameters[0])
    )?;
    for (i, p) in parameters.iter().enumerate().skip(1) {
        writeln!(body, "{}=(uint64_t)c_arg_{i};", value(f, *p))?;
    }
    writeln!(body, "goto {entry};")?;
    let cfg = cranelift_codegen::flowgraph::ControlFlowGraph::with_function(f);
    let mut helpers = BTreeSet::new();
    let mut native_thunks = BTreeSet::new();
    for block in f.layout.blocks() {
        // Verified SSA may retain unreachable cleanup blocks. Their
        // instructions still receive the same validation, but a C label is
        // needed only for the entry jump or an actual incoming edge.
        if block == entry || cfg.pred_iter(block).next().is_some() {
            writeln!(body, "{block}:;")?;
        }
        for inst in f.layout.block_insts(block) {
            let data = &f.dfg.insts[inst];
            if let Some(flags) = data.memflags_data(&f.dfg) {
                ensure!(
                    flags.endianness(ir::Endianness::Little) == ir::Endianness::Little,
                    "big-endian C memory access"
                );
            }
            let results = f.dfg.inst_results(inst);
            let result = || -> Result<String> {
                ensure!(results.len() == 1, "C result arity");
                Ok(value(f, results[0]))
            };
            match *data {
                D::UnaryImm {
                    opcode: O::Iconst,
                    imm,
                } => {
                    writeln!(
                        body,
                        "{}=({}){};",
                        result()?,
                        c_type(f.dfg.value_type(results[0]))?,
                        word(imm.bits())
                    )?;
                }
                D::Binary { opcode, args }
                    if matches!(
                        opcode,
                        O::Iadd | O::Isub | O::Imul | O::Band | O::Bor | O::Bxor
                    ) =>
                {
                    let op = match opcode {
                        O::Iadd => "+",
                        O::Isub => "-",
                        O::Imul => "*",
                        O::Band => "&",
                        O::Bor => "|",
                        _ => "^",
                    };
                    writeln!(
                        body,
                        "{}=({})((uint64_t){} {op} (uint64_t){});",
                        result()?,
                        c_type(f.dfg.value_type(results[0]))?,
                        value(f, args[0]),
                        value(f, args[1])
                    )?;
                }
                D::Unary {
                    opcode: O::Uextend | O::Ireduce,
                    arg,
                } => {
                    writeln!(
                        body,
                        "{}=({}){};",
                        result()?,
                        c_type(f.dfg.value_type(results[0]))?,
                        value(f, arg)
                    )?;
                }
                D::Unary {
                    opcode: O::Sextend,
                    arg,
                } => {
                    writeln!(
                        body,
                        "{}=({})(({}){});",
                        result()?,
                        c_type(f.dfg.value_type(results[0]))?,
                        signed_type(f.dfg.value_type(arg))?,
                        value(f, arg)
                    )?;
                }
                D::IntCompare { args, cond, .. } => {
                    let (operator, signed) = match cond {
                        IntCC::Equal => ("==", false),
                        IntCC::NotEqual => ("!=", false),
                        IntCC::SignedLessThan => ("<", true),
                        IntCC::SignedLessThanOrEqual => ("<=", true),
                        IntCC::SignedGreaterThan => (">", true),
                        IntCC::SignedGreaterThanOrEqual => (">=", true),
                        IntCC::UnsignedLessThan => ("<", false),
                        IntCC::UnsignedLessThanOrEqual => ("<=", false),
                        IntCC::UnsignedGreaterThan => (">", false),
                        IntCC::UnsignedGreaterThanOrEqual => (">=", false),
                    };
                    let ty = if signed {
                        signed_type(f.dfg.value_type(args[0]))?
                    } else {
                        c_type(f.dfg.value_type(args[0]))?
                    };
                    writeln!(
                        body,
                        "{}=(({}){} {operator} ({}){});",
                        result()?,
                        ty,
                        value(f, args[0]),
                        ty,
                        value(f, args[1])
                    )?;
                }
                D::Load {
                    opcode: O::Load,
                    arg,
                    offset,
                    ..
                } => {
                    let r = result()?;
                    writeln!(
                        body,
                        "memcpy(&{r},{},sizeof {r});",
                        address(f, arg, i32::from(offset))
                    )?;
                }
                D::Store {
                    opcode: O::Store,
                    args,
                    offset,
                    ..
                } => {
                    let v = value(f, args[0]);
                    if f.dfg.value_type(args[0]) == types::I64
                        && let Some(word) = stack_word_address(f, args[1], i32::from(offset))
                    {
                        writeln!(body, "{word}={v};")?;
                    } else {
                        writeln!(
                            body,
                            "memcpy({},&{v},sizeof {v});",
                            address(f, args[1], i32::from(offset))
                        )?;
                    }
                }
                D::StackAddr {
                    opcode: O::StackAddr,
                    stack_slot,
                    offset,
                } => {
                    ensure!(
                        i32::from(offset) >= 0
                            && (i32::from(offset) as u32) <= f.sized_stack_slots[stack_slot].size,
                        "C stack address bounds"
                    );
                    writeln!(
                        body,
                        "{}=(uint64_t)(uintptr_t)((unsigned char *){stack_slot}+{});",
                        result()?,
                        i32::from(offset)
                    )?;
                }
                D::UnaryGlobalValue {
                    opcode: O::SymbolValue,
                    global_value,
                } => {
                    let (_, offset) = literals
                        .get(&global_value)
                        .ok_or_else(|| anyhow!("nonliteral C global"))?;
                    writeln!(
                        body,
                        "{}=(uint64_t)(uintptr_t)c_literal_{symbol}_{global_value}+{};",
                        result()?,
                        word(*offset)
                    )?;
                }
                D::Call { func_ref, args, .. } => {
                    let name = imports
                        .get(&func_ref)
                        .ok_or_else(|| anyhow!("unknown C import"))?;
                    let args = args.as_slice(&f.dfg.value_lists);
                    let sig = &f.dfg.ext_funcs[func_ref];
                    ensure!(
                        f.dfg.signatures[sig.signature]
                            .params
                            .iter()
                            .all(|p| p.value_type == types::I64),
                        "non-word direct C call"
                    );
                    let (ret, types): (&str, Vec<&str>) = if let Some((_, ret, params)) =
                        abi::C_HELPERS.iter().find(|(n, _, _)| *n == name)
                    {
                        helpers.insert(name.clone());
                        (ret, params.to_vec())
                    } else if name
                        .strip_prefix("rinzu_source_native_")
                        .is_some_and(|s| s.parse::<u32>().is_ok())
                    {
                        ensure!(signature(f, sig.signature)? == 2, "native word thunk arity");
                        native_thunks.insert(name.clone());
                        ("int64_t", vec!["uint64_t", "const uint64_t *"])
                    } else {
                        bail!("unsupported C import {name}")
                    };
                    ensure!(
                        args.len() == types.len() && results.len() == usize::from(ret != "void"),
                        "typed C helper arity {name}"
                    );
                    let actual = args
                        .iter()
                        .zip(types)
                        .map(|(v, ty)| {
                            let v = value(f, *v);
                            if ty.contains('*') {
                                format!("({ty})(uintptr_t){v}")
                            } else {
                                format!("({ty}){v}")
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(",");
                    if ret == "void" {
                        writeln!(body, "{name}({actual});")?;
                    } else if ret.contains('*') {
                        writeln!(body, "{}=(uint64_t)(uintptr_t){name}({actual});", result()?)?;
                    } else {
                        writeln!(body, "{}=(uint64_t){name}({actual});", result()?)?;
                    }
                }
                D::CallIndirect { sig_ref, args, .. } => {
                    ensure!(
                        managed_signatures.contains(&sig_ref),
                        "non-managed C indirect call"
                    );
                    let count = signature(f, sig_ref)?;
                    let args = args.as_slice(&f.dfg.value_lists);
                    ensure!(
                        count > 0 && args.len() == count + 1,
                        "managed C indirect arity"
                    );
                    let mut types = vec!["void *"];
                    types.extend((1..count).map(|_| "int64_t"));
                    let mut actual = vec![format!("(void *)(uintptr_t){}", value(f, args[1]))];
                    actual.extend(
                        args[2..]
                            .iter()
                            .map(|v| format!("(int64_t){}", value(f, *v))),
                    );
                    writeln!(
                        body,
                        "{}=(uint64_t)((int64_t (*)({}))(uintptr_t){})({});",
                        result()?,
                        types.join(","),
                        value(f, args[0]),
                        actual.join(",")
                    )?;
                }
                D::Jump { destination, .. } => {
                    body.push_str(&edge(f, destination, &inst.to_string())?);
                }
                D::Brif { arg, blocks, .. } => {
                    writeln!(
                        body,
                        "if({}) {} else {}",
                        value(f, arg),
                        edge(f, blocks[0], &format!("{inst}_a"))?,
                        edge(f, blocks[1], &format!("{inst}_b"))?
                    )?;
                }
                D::BranchTable { arg, table, .. } => {
                    writeln!(body, "switch((uint64_t){}) {{", value(f, arg))?;
                    let table = &f.dfg.jump_tables[table];
                    for (i, branch) in table.as_slice().iter().enumerate() {
                        writeln!(
                            body,
                            "case {i}: {}",
                            edge(f, *branch, &format!("{inst}_{i}"))?
                        )?;
                    }
                    writeln!(
                        body,
                        "default: {}\n}}",
                        edge(f, table.default_block(), &format!("{inst}_default"))?
                    )?;
                }
                D::MultiAry {
                    opcode: O::Return,
                    args,
                } => {
                    let args = args.as_slice(&f.dfg.value_lists);
                    ensure!(args.len() == 1, "C return arity");
                    writeln!(body, "return (int64_t){};", value(f, args[0]))?;
                }
                _ => bail!("unsupported C instruction {}", data.opcode()),
            }
        }
    }
    body.push_str("}\n");
    let mut source = String::from(
        "_Static_assert(sizeof(uintptr_t)==8 && sizeof(uint64_t)==8,\"private C word ABI\");\n",
    );
    for name in helpers {
        let (_, ret, params) = abi::C_HELPERS.iter().find(|(n, _, _)| *n == name).unwrap();
        writeln!(
            source,
            "_Static_assert(__builtin_types_compatible_p(__typeof__(&{name}), {ret} (*)({})),\"private C helper ABI: {name}\");",
            params.join(",")
        )?;
    }
    for name in native_thunks {
        writeln!(source, "extern int64_t {name}(uint64_t,const uint64_t *);")?;
    }
    let mut ordered = literals.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|(v, _)| v.as_u32());
    for (global, (bytes, _)) in ordered {
        ensure!(!bytes.is_empty(), "empty C literal global");
        writeln!(
            source,
            "static const unsigned char c_literal_{symbol}_{global}[]={{ {} }};",
            bytes
                .iter()
                .map(|b| format!("0x{b:02x}"))
                .collect::<Vec<_>>()
                .join(",")
        )?;
    }
    source.push_str(&body);
    Ok(Emission {
        source,
        instructions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cranelift_codegen::ir::{
        AbiParam, InstBuilder, JumpTableData, MemFlagsData, StackSlotData, StackSlotKind,
    };
    use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
    use cranelift_module::{Linkage, Module, default_libcall_names};
    use cranelift_object::{ObjectBuilder, ObjectModule};
    use std::process::Command;

    fn module() -> ObjectModule {
        ObjectModule::new(
            ObjectBuilder::new(
                crate::native_isa(true).unwrap(),
                "c-lowering-test",
                default_libcall_names(),
            )
            .unwrap(),
        )
    }
    fn function(module: &ObjectModule) -> Function {
        let mut f = Function::new();
        f.signature = module.make_signature();
        f.signature.params.extend([AbiParam::new(types::I64); 2]);
        f.signature.returns.push(AbiParam::new(types::I64));
        f
    }
    fn generated() -> (String, String) {
        let mut module = module();
        let mut f = function(&module);
        let mut context = FunctionBuilderContext::new();
        {
            let mut b = FunctionBuilder::new(&mut f, &mut context);
            let entry = b.create_block();
            let head = b.create_block();
            let again = b.create_block();
            let done = b.create_block();
            b.append_block_params_for_function_params(entry);
            for _ in 0..3 {
                b.append_block_param(head, types::I64);
            }
            b.switch_to_block(entry);
            let count = b.block_params(entry)[1];
            let a = b.ins().iconst(types::I64, 1);
            let c = b.ins().iconst(types::I64, 2);
            b.ins().jump(head, &[a.into(), c.into(), count.into()]);
            b.switch_to_block(head);
            let p = b.block_params(head).to_vec();
            let zero = b.ins().iconst(types::I64, 0);
            let empty = b.ins().icmp(IntCC::Equal, p[2], zero);
            b.ins().brif(empty, done, &[], again, &[]);
            b.switch_to_block(again);
            let one = b.ins().iconst(types::I64, 1);
            let left = b.ins().isub(p[2], one);
            b.ins().jump(head, &[p[1].into(), p[0].into(), left.into()]);
            b.switch_to_block(done);
            let hundred = b.ins().iconst(types::I64, 100);
            let first = b.ins().imul(p[0], hundred);
            let answer = b.ins().iadd(first, p[1]);
            b.ins().return_(&[answer]);
            b.seal_all_blocks();
            b.finalize(module.target_config());
        }
        let mut source = emit(
            &f,
            "c_swap",
            &HashMap::new(),
            &HashMap::new(),
            &HashSet::new(),
            module.isa(),
        )
        .unwrap()
        .source;

        let mut f = function(&module);
        let mut context = FunctionBuilderContext::new();
        {
            let mut b = FunctionBuilder::new(&mut f, &mut context);
            let entry = b.create_block();
            let zero = b.create_block();
            let one = b.create_block();
            let other = b.create_block();
            b.append_block_params_for_function_params(entry);
            b.switch_to_block(entry);
            let index = b.block_params(entry)[1];
            let index = b.ins().ireduce(types::I32, index);
            let default = BlockCall::new(other, [], &mut b.func.dfg.value_lists);
            let a = BlockCall::new(zero, [], &mut b.func.dfg.value_lists);
            let c = BlockCall::new(one, [], &mut b.func.dfg.value_lists);
            let table = b.create_jump_table(JumpTableData::new(default, &[a, c]));
            b.ins().br_table(index, table);
            for (block, answer) in [(zero, 10), (one, 20), (other, 999)] {
                b.switch_to_block(block);
                let answer = b.ins().iconst(types::I64, answer);
                b.ins().return_(&[answer]);
            }
            b.seal_all_blocks();
            b.finalize(module.target_config());
        }
        source.push_str(
            &emit(
                &f,
                "c_table",
                &HashMap::new(),
                &HashMap::new(),
                &HashSet::new(),
                module.isa(),
            )
            .unwrap()
            .source,
        );

        let mut f = function(&module);
        let mut context = FunctionBuilderContext::new();
        {
            let mut b = FunctionBuilder::new(&mut f, &mut context);
            let entry = b.create_block();
            b.append_block_params_for_function_params(entry);
            b.switch_to_block(entry);
            let slot =
                b.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 16, 3));
            let address = b.ins().stack_addr(types::I64, slot, 0);
            let input = b.block_params(entry)[1];
            b.ins().store(MemFlagsData::new(), input, address, 1);
            let output = b.ins().load(types::I64, MemFlagsData::new(), address, 1);
            b.ins().return_(&[output]);
            b.seal_all_blocks();
            b.finalize(module.target_config());
        }
        source.push_str(
            &emit(
                &f,
                "c_unaligned",
                &HashMap::new(),
                &HashMap::new(),
                &HashSet::new(),
                module.isa(),
            )
            .unwrap()
            .source,
        );

        let mut f = function(&module);
        let mut sig = module.make_signature();
        sig.params.extend([AbiParam::new(types::I64); 2]);
        sig.returns.push(AbiParam::new(types::I64));
        let id = module
            .declare_function("rinzu_baseline_prepare_roots", Linkage::Import, &sig)
            .unwrap();
        let helper = module.declare_func_in_func(id, &mut f);
        let mut context = FunctionBuilderContext::new();
        {
            let mut b = FunctionBuilder::new(&mut f, &mut context);
            let entry = b.create_block();
            b.append_block_params_for_function_params(entry);
            b.switch_to_block(entry);
            let ctx = b.block_params(entry)[0];
            let input = b.block_params(entry)[1];
            let count = b.ins().iconst(types::I64, 1);
            let call = b.ins().call(helper, &[ctx, count]);
            let pointer = b.inst_results(call)[0];
            b.ins().store(MemFlagsData::new(), input, pointer, 0);
            let output = b.ins().load(types::I64, MemFlagsData::new(), pointer, 0);
            b.ins().return_(&[output]);
            let unreachable = b.create_block();
            b.switch_to_block(unreachable);
            let unused = b.ins().iconst(types::I64, 0);
            b.ins().return_(&[unused]);
            b.seal_all_blocks();
            b.finalize(module.target_config());
        }
        let pointer = emit(
            &f,
            "c_pointer",
            &HashMap::from([(helper, "rinzu_baseline_prepare_roots".into())]),
            &HashMap::new(),
            &HashSet::new(),
            module.isa(),
        )
        .unwrap()
        .source;
        source.push_str(&pointer);
        (source, pointer)
    }

    #[test]
    fn generated_c_preserves_parallel_edges_table_and_unaligned_pointer_words() {
        let (source, _) = generated();
        let directory =
            std::env::temp_dir().join(format!("rinzu-c-lowering-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let c = directory.join("test.c");
        let prelude = "#include <stdint.h>\n#include <string.h>\n#include <assert.h>\nstatic uint64_t root_word;\nuint64_t *rinzu_baseline_prepare_roots(void *ctx,int64_t count) {assert((uintptr_t)ctx==41 && count==1);return &root_word;}\n";
        let main = "int main(void) {void *ctx=(void *)(uintptr_t)41;assert(c_swap(ctx,0)==102);assert(c_swap(ctx,1)==201);assert(c_swap(ctx,16)==102);assert(c_swap(ctx,17)==201);assert(c_table(ctx,0)==10);assert(c_table(ctx,1)==20);assert(c_table(ctx,-1)==999);assert(c_table(ctx,UINT32_MAX)==999);for(int i=0;i<2;++i){int64_t x=i?INT64_MIN:INT64_MAX;assert(c_unaligned(ctx,x)==x);assert(c_pointer(ctx,x)==x);}return 0;}\n";
        std::fs::write(&c, format!("{prelude}{source}{main}")).unwrap();
        for sanitizer in [false, true] {
            let binary = directory.join(if sanitizer { "sanitized" } else { "plain" });
            let mut build = Command::new("clang");
            build.args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror"]);
            if sanitizer {
                build.args(["-fsanitize=address,undefined", "-fno-sanitize-recover=all"]);
            }
            let output = build.arg(&c).arg("-o").arg(&binary).output().unwrap();
            assert!(output.status.success(), "{output:?}");
            let output = Command::new(&binary)
                .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn generated_c_rejects_an_erased_pointer_helper_abi() {
        let (_, pointer) = generated();
        let directory = std::env::temp_dir().join(format!("rinzu-c-abi-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let c = directory.join("wrong.c");
        std::fs::write(&c,format!("#include <stdint.h>\n#include <string.h>\nuint64_t rinzu_baseline_prepare_roots(void *ctx,int64_t n){{(void)ctx;(void)n;return 0;}}\n{pointer}")).unwrap();
        let output = Command::new("clang")
            .args(["-std=c11", "-fsyntax-only"])
            .arg(c)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("private C helper ABI"));
        std::fs::remove_dir_all(directory).unwrap();
    }
}
