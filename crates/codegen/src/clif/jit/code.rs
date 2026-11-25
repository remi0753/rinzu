//! Capture and replay Cranelift's unrelocated module definitions.
use super::*;
use cranelift_codegen::{
    FinalizedRelocTarget,
    binemit::Reloc,
    control::ControlPlane,
    ir::{ExternalName, LibCall, Signature},
    isa::TargetIsa,
};
use cranelift_module::{
    DataId, Init, ModuleDeclarations, ModuleReloc, ModuleRelocTarget, ModuleResult,
};
use serde::{Deserialize, Serialize};

pub struct AsyncJitImage {
    pub metadata: AsyncJitCode,
    pub bytes: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AsyncJitCode {
    declarations: Vec<Declaration>,
    definitions: Vec<Definition>,
    compile: CompileEvidence,
}

#[derive(Serialize, Deserialize)]
struct Declaration {
    name: String,
    linkage: Linkage,
    signature: Signature,
}

#[derive(Serialize, Deserialize)]
enum Definition {
    Function {
        id: u32,
        length: usize,
        alignment: u64,
        relocs: Vec<Relocation>,
    },
    Constant {
        id: u32,
        length: usize,
        alignment: Option<u64>,
    },
}

impl Definition {
    fn length(&self) -> usize {
        match self {
            Self::Function { length, .. } | Self::Constant { length, .. } => *length,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Relocation {
    kind: Reloc,
    offset: u32,
    target: ModuleRelocTarget,
    addend: i64,
}

impl From<ModuleReloc> for Relocation {
    fn from(reloc: ModuleReloc) -> Self {
        Self {
            kind: reloc.kind,
            offset: reloc.offset,
            target: reloc.name,
            addend: reloc.addend,
        }
    }
}

impl From<&Relocation> for ModuleReloc {
    fn from(reloc: &Relocation) -> Self {
        Self {
            kind: reloc.kind,
            offset: reloc.offset,
            name: reloc.target.clone(),
            addend: reloc.addend,
        }
    }
}

impl AsyncJitCode {
    // Also bounds each function's possible veneer region to the AArch64
    // branch range, after accounting for the nonoverlapping relocation slots.
    pub const MAX_BYTES: usize = 16 * 1024 * 1024;

    pub(super) fn replay(
        self,
        module: &mut JITModule,
        aot: &crate::BaselineAotObject,
        runtime_symbols: &[String],
        bytes: &[u8],
    ) -> Result<CompileEvidence> {
        let mut timing = rinzu_compiler::timing::span("jit.managed.cache.validate");
        let mut exports = aot
            .functions
            .iter()
            .map(|function| {
                (
                    function.symbol.clone(),
                    managed_signature(module, function.parameter_count),
                )
            })
            .collect::<HashMap<_, _>>();
        let stubs = aot
            .native_signatures
            .iter()
            .filter(|stub| !stub.is_async)
            .count();
        for index in 0..stubs {
            exports.insert(
                format!("rinzu_source_native_{index}"),
                native_abi::stub_signature(module),
            );
        }
        for (index, decl) in self.declarations.iter().enumerate() {
            ensure!(
                match decl.linkage {
                    Linkage::Export => exports.remove(&decl.name).as_ref() == Some(&decl.signature),
                    Linkage::Import => true,
                    _ => false,
                },
                "cached JIT declaration differs from the checked program"
            );
            let id = module.declare_function(&decl.name, decl.linkage, &decl.signature)?;
            ensure!(
                id.as_u32() as usize == index,
                "duplicate cached JIT declaration"
            );
        }
        ensure!(exports.is_empty(), "missing cached JIT declarations");

        let lengths = Lengths::checked(&self, bytes.len())?;
        ensure!(
            self.compile.code_size > 0 && self.compile.code_size <= bytes.len(),
            "invalid cached JIT compile evidence"
        );
        let imports = runtime_symbols
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let mut libcalls = HashMap::new();
        for definition in &self.definitions {
            if let Definition::Function { length, relocs, .. } = definition {
                let mut end = 0;
                for reloc in relocs {
                    end = reloc.check(
                        *length,
                        end,
                        aot.target,
                        &lengths,
                        &self.declarations,
                        &imports,
                        &mut libcalls,
                    )?;
                }
            }
        }
        timing.set_counter("functions", (aot.functions.len() + stubs) as u64);
        timing.set_counter("constants", lengths.constants.len() as u64);
        drop(timing);

        let _timing = rinzu_compiler::timing::span("jit.managed.cache.replay");
        let mut remaining = bytes;
        for definition in &self.definitions {
            let (body, next) = remaining.split_at(definition.length());
            remaining = next;
            match definition {
                Definition::Function {
                    id,
                    alignment,
                    relocs,
                    ..
                } => {
                    let relocs = relocs.iter().map(ModuleReloc::from).collect::<Vec<_>>();
                    module.define_function_bytes(
                        FuncId::from_u32(*id),
                        *alignment,
                        body,
                        &relocs,
                    )?;
                }
                Definition::Constant { id, alignment, .. } => {
                    let data = module.declare_anonymous_data(false, false)?;
                    ensure!(data.as_u32() == *id, "cached JIT constant order differs");
                    let mut description = DataDescription::new();
                    description.align = *alignment;
                    description.define(body.into());
                    module.define_data(data, &description)?;
                }
            }
        }
        self.check_local_distances(module)?;
        Ok(self.compile)
    }

    fn check_local_distances(&self, module: &JITModule) -> Result<()> {
        for definition in &self.definitions {
            if let Definition::Function { id, relocs, .. } = definition {
                let address = module.get_address(&FuncId::from_u32(*id).into()).addr() as i128;
                for reloc in relocs.iter().filter(|reloc| reloc.needs_local_range()) {
                    // These targets were checked to be definitions in this module.
                    let from = address + i128::from(reloc.offset);
                    let to =
                        module.get_address(&reloc.target).addr() as i128 + i128::from(reloc.addend);
                    let distance = if reloc.kind == Reloc::Aarch64AdrPrelPgHi21 {
                        (to & !0xfff) - (from & !0xfff)
                    } else {
                        to - from
                    };
                    ensure!(
                        i32::try_from(distance).is_ok(),
                        "cached JIT relocation is out of range"
                    );
                }
            }
        }
        Ok(())
    }
}

struct Lengths {
    functions: Vec<Option<usize>>,
    constants: Vec<usize>,
}

impl Lengths {
    fn checked(code: &AsyncJitCode, bytes: usize) -> Result<Self> {
        ensure!(
            bytes <= AsyncJitCode::MAX_BYTES,
            "oversized cached JIT code"
        );
        let mut functions = vec![None; code.declarations.len()];
        let mut constants = Vec::new();
        let mut remaining = bytes;
        for definition in &code.definitions {
            let length = definition.length();
            ensure!(
                length > 0 && length <= remaining,
                "invalid cached JIT payload length"
            );
            remaining -= length;
            match definition {
                Definition::Function { id, alignment, .. } => {
                    let body = functions
                        .get_mut(*id as usize)
                        .context("unknown cached JIT function")?;
                    ensure!(
                        body.replace(length).is_none() && alignment.is_power_of_two(),
                        "invalid cached JIT function"
                    );
                }
                Definition::Constant { id, alignment, .. } => {
                    ensure!(
                        *id as usize == constants.len()
                            && alignment.is_none_or(u64::is_power_of_two),
                        "invalid cached JIT constant"
                    );
                    constants.push(length);
                }
            }
        }
        ensure!(
            remaining == 0
                && code
                    .declarations
                    .iter()
                    .zip(&functions)
                    .all(|(decl, length)| {
                        (decl.linkage == Linkage::Export) == length.is_some()
                    }),
            "cached JIT definitions do not match the declarations"
        );
        Ok(Self {
            functions,
            constants,
        })
    }
}

impl Relocation {
    fn needs_local_range(&self) -> bool {
        matches!(self.kind, Reloc::X86PCRel4 | Reloc::Aarch64AdrPrelPgHi21)
    }

    #[allow(clippy::too_many_arguments)]
    fn check(
        &self,
        length: usize,
        prior_end: usize,
        target: Target,
        lengths: &Lengths,
        declarations: &[Declaration],
        imports: &HashSet<&str>,
        libcalls: &mut HashMap<LibCall, bool>,
    ) -> Result<usize> {
        let (end, correction, call) =
            relocation_field(target, self.kind, self.offset as usize, length, prior_end)?;
        let addend = self
            .addend
            .checked_add(correction)
            .and_then(|n| usize::try_from(n).ok());
        let valid = match self.target {
            ModuleRelocTarget::User {
                namespace: 0,
                index,
            } => declarations.get(index as usize).is_some_and(|decl| {
                addend == Some(0)
                    && (decl.linkage != Linkage::Import
                        || (!self.needs_local_range() && imports.contains(decl.name.as_str())))
            }),
            ModuleRelocTarget::User {
                namespace: 1,
                index,
            } => {
                !call
                    && lengths
                        .constants
                        .get(index as usize)
                        .is_some_and(|&size| addend.is_some_and(|addend| addend <= size))
            }
            ModuleRelocTarget::FunctionOffset(id, at) => {
                addend == Some(0)
                    && lengths
                        .functions
                        .get(id.as_u32() as usize)
                        .and_then(|length| *length)
                        .is_some_and(|length| (at as usize) < length)
            }
            ModuleRelocTarget::LibCall(libcall) => {
                addend == Some(0)
                    && !self.needs_local_range()
                    && *libcalls
                        .entry(libcall)
                        .or_insert_with(|| libcall_is_available(libcall))
            }
            _ => false,
        };
        ensure!(valid, "invalid cached JIT relocation target or addend");
        Ok(end)
    }
}

fn relocation_field(
    target: Target,
    kind: Reloc,
    offset: usize,
    length: usize,
    prior_end: usize,
) -> Result<(usize, i64, bool)> {
    let (width, correction, call) = match (target, kind) {
        (_, Reloc::Abs8) => (8, 0, false),
        (Target::Aarch64AppleDarwin, Reloc::Arm64Call) => (4, 0, true),
        (Target::Aarch64AppleDarwin, Reloc::Aarch64AdrPrelPgHi21 | Reloc::Aarch64AddAbsLo12Nc) => {
            (4, 0, false)
        }
        (Target::X86_64UnknownLinuxGnu, Reloc::X86CallPCRel4 | Reloc::X86CallPLTRel4) => {
            (4, 4, true)
        }
        (Target::X86_64UnknownLinuxGnu, Reloc::X86PCRel4 | Reloc::X86GOTPCRel4) => (4, 4, false),
        _ => bail!("unsupported cached JIT relocation"),
    };
    ensure!(
        offset >= prior_end
            && offset.checked_add(width).is_some_and(|end| end <= length)
            && (target != Target::Aarch64AppleDarwin || offset.is_multiple_of(4)),
        "cached JIT relocation is outside its function"
    );
    Ok((offset + width, correction, call))
}

fn libcall_is_available(libcall: LibCall) -> bool {
    let name = default_libcall_names()(libcall);
    let Ok(name) = std::ffi::CString::new(name) else {
        return false;
    };
    // SAFETY: the name is a NUL-terminated Cranelift libcall symbol. This
    // checks the same lookup scope as JITBuilder and never calls the symbol.
    !unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) }.is_null()
}

#[derive(Default)]
struct Recorded {
    definitions: Vec<Definition>,
    bytes: Vec<u8>,
}

/// Defines current functions and constants, optionally from cached Cranelift stencils.
pub(super) struct Capture<'module, 'cache> {
    module: &'module mut JITModule,
    recorded: Option<Recorded>,
    cache: Option<&'cache mut dyn AsyncJitFunctionCache>,
    reused: usize,
}

impl<'module, 'cache> Capture<'module, 'cache> {
    pub(super) fn new(
        module: &'module mut JITModule,
        enabled: bool,
        cache: Option<&'cache mut dyn AsyncJitFunctionCache>,
    ) -> Self {
        Self {
            module,
            recorded: enabled.then(Recorded::default),
            cache,
            reused: 0,
        }
    }

    pub(super) fn reused(&self) -> usize {
        self.reused
    }

    pub(super) fn finish(self, compile: &CompileEvidence) -> Option<AsyncJitImage> {
        let recorded = self.recorded?;
        let declarations = self
            .module
            .declarations()
            .get_functions()
            .map(|(_, decl)| {
                Some(Declaration {
                    name: decl.name.clone()?,
                    linkage: decl.linkage,
                    signature: decl.signature.clone(),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(AsyncJitImage {
            metadata: AsyncJitCode {
                declarations,
                definitions: recorded.definitions,
                compile: compile.clone(),
            },
            bytes: recorded.bytes,
        })
    }

    fn push(&mut self, bytes: &[u8], definition: impl FnOnce() -> Definition) {
        if let Some(recorded) = &mut self.recorded {
            if bytes.len() > AsyncJitCode::MAX_BYTES - recorded.bytes.len() {
                self.recorded = None;
                return;
            }
            recorded.bytes.extend_from_slice(bytes);
            recorded.definitions.push(definition());
        }
    }
}

impl Module for Capture<'_, '_> {
    fn isa(&self) -> &dyn TargetIsa {
        self.module.isa()
    }

    fn declarations(&self) -> &ModuleDeclarations {
        self.module.declarations()
    }

    fn declare_function(
        &mut self,
        name: &str,
        linkage: Linkage,
        signature: &Signature,
    ) -> ModuleResult<FuncId> {
        self.module.declare_function(name, linkage, signature)
    }

    fn declare_anonymous_function(&mut self, signature: &Signature) -> ModuleResult<FuncId> {
        self.recorded = None;
        self.module.declare_anonymous_function(signature)
    }

    fn declare_data(
        &mut self,
        name: &str,
        linkage: Linkage,
        writable: bool,
        tls: bool,
    ) -> ModuleResult<DataId> {
        self.recorded = None;
        self.module.declare_data(name, linkage, writable, tls)
    }

    fn declare_anonymous_data(&mut self, writable: bool, tls: bool) -> ModuleResult<DataId> {
        if writable || tls {
            self.recorded = None;
        }
        self.module.declare_anonymous_data(writable, tls)
    }

    fn define_function_with_control_plane(
        &mut self,
        id: FuncId,
        context: &mut cranelift_codegen::Context,
        control: &mut ControlPlane,
    ) -> ModuleResult<()> {
        let mut relocs = None;
        if let Some(cache) = self.cache.as_deref_mut() {
            let (_, reused) = context.compile_with_cache(self.module.isa(), cache, control)?;
            let compiled = context
                .compiled_code()
                .expect("Cranelift retained the stencil");
            if !reused || valid_cached_relocations(context) {
                // The current FunctionParameters bind the stencil's external names
                // to this module's function and data IDs.
                let current = module_relocations(context, id);
                self.module.define_function_bytes(
                    id,
                    u64::from(compiled.buffer.alignment),
                    compiled.code_buffer(),
                    &current,
                )?;
                relocs = Some(current);
                self.reused += usize::from(reused);
            }
        }
        if relocs.is_none() {
            self.module
                .define_function_with_control_plane(id, context, control)?;
        }
        if self.recorded.is_some() {
            let compiled = context
                .compiled_code()
                .expect("Cranelift just defined the function");
            self.push(compiled.code_buffer(), || Definition::Function {
                id: id.as_u32(),
                length: compiled.code_buffer().len(),
                alignment: u64::from(compiled.buffer.alignment),
                relocs: relocs
                    .unwrap_or_else(|| module_relocations(context, id))
                    .into_iter()
                    .map(Relocation::from)
                    .collect(),
            });
        }
        Ok(())
    }

    fn define_function_bytes(
        &mut self,
        id: FuncId,
        alignment: u64,
        bytes: &[u8],
        relocs: &[ModuleReloc],
    ) -> ModuleResult<()> {
        self.recorded = None;
        self.module
            .define_function_bytes(id, alignment, bytes, relocs)
    }

    fn define_data(&mut self, id: DataId, data: &DataDescription) -> ModuleResult<()> {
        self.module.define_data(id, data)?;
        if let Init::Bytes { contents } = &data.init
            && data.function_relocs.is_empty()
            && data.data_relocs.is_empty()
        {
            self.push(contents, || Definition::Constant {
                id: id.as_u32(),
                length: contents.len(),
                alignment: data.align,
            });
        } else {
            self.recorded = None;
        }
        Ok(())
    }
}

fn module_relocations(context: &cranelift_codegen::Context, id: FuncId) -> Vec<ModuleReloc> {
    context
        .compiled_code()
        .unwrap()
        .buffer
        .relocs()
        .iter()
        .map(|reloc| ModuleReloc::from_mach_reloc(reloc, &context.func, id))
        .collect()
}

fn valid_cached_relocations(context: &cranelift_codegen::Context) -> bool {
    let compiled = context.compiled_code().unwrap();
    let length = compiled.code_buffer().len();
    if length == 0
        || length > AsyncJitCode::MAX_BYTES
        || !compiled.buffer.alignment.is_power_of_two()
    {
        return false;
    }
    let Some(target) = Target::host() else {
        return false;
    };
    let mut prior_end = 0;
    compiled.buffer.relocs().iter().all(|reloc| {
        let Ok((end, correction, call)) =
            relocation_field(target, reloc.kind, reloc.offset as usize, length, prior_end)
        else {
            return false;
        };
        prior_end = end;
        let addend = reloc.addend.checked_add(correction);
        match reloc.target {
            FinalizedRelocTarget::ExternalName(ExternalName::User(reference)) => context
                .func
                .params
                .user_named_funcs()
                .get(reference)
                .is_some_and(|name| match name.namespace {
                    0 => addend == Some(0),
                    1 => !call && addend.is_some_and(|offset| offset >= 0),
                    _ => false,
                }),
            FinalizedRelocTarget::ExternalName(ExternalName::LibCall(name)) => {
                addend == Some(0) && libcall_is_available(name)
            }
            FinalizedRelocTarget::Func(at) => (at as usize) < length && addend == Some(0),
            _ => false,
        }
    })
}
