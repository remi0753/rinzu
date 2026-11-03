//! M5.05 experiment only: serialized, closed scalar images, no production reload API.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Result, anyhow, ensure};
use cranelift_codegen::ir::{AbiParam, InstBuilder, UserFuncName, types};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module, default_libcall_names};
use rinzu_compiler::ast::{Declaration, ExpressionKind, Literal, StatementKind};
use rinzu_compiler::{CompilerOptions, FileId, FrontendArtifact, SourceFile, Type};
use rinzu_runtime::code::{CodeFrame, CodeVersion, CodeVersionId, CodeVersionState};
use rinzu_runtime::dispatch::VersionedDispatchCell;

use crate::function_reference::ReferenceTargets;
use crate::macos_jit_memory::{JitMemoryAuditSnapshot, MacOsArm64JitMemory};

fn checked(text: &str) -> (SourceFile, FrontendArtifact) {
    let file = SourceFile::new(FileId(1), "entry.rinzu", text);
    let text = rinzu_compiler::format_source(&file).expect("format spike input");
    let file = SourceFile::new(FileId(1), "entry.rinzu", text);
    let artifact = rinzu_compiler::compile_source_with_options(
        &file,
        &CompilerOptions {
            is_library: true,
            ..CompilerOptions::default()
        },
    )
    .expect("check spike input through verified IR");
    (file, artifact)
}

// Equal ABI/layout/target contracts are necessary, but do not prove arbitrary
// bodies compatible. The executable experiment further accepts only one pure
// synchronous zero-argument Int function returning an integer literal.
fn body_compatible(old: &FrontendArtifact, new: &FrontendArtifact) -> Result<()> {
    let (old_ir, new_ir) = (&old.ir, &new.ir);
    ensure!(
        old_ir.source_types == new_ir.source_types,
        "data layout changed"
    );
    ensure!(
        old_ir.native_records == new_ir.native_records,
        "native layout changed"
    );
    ensure!(
        old_ir.source_native_signatures == new_ir.source_native_signatures,
        "native ABI changed"
    );
    ensure!(
        ReferenceTargets::new(old_ir)?.metadata() == ReferenceTargets::new(new_ir)?.metadata(),
        "function target universe changed"
    );
    ensure!(
        old_ir.functions.len() == new_ir.functions.len(),
        "function set changed"
    );
    for (old, new) in old_ir.functions.iter().zip(&new_ir.functions) {
        ensure!(
            old.name == new.name
                && old.is_async == new.is_async
                && old.parameter_types == new.parameter_types
                && old.result_type == new.result_type
                && old.callback_fallback == new.callback_fallback,
            "function signature changed"
        );
        ensure!(old.facts == new.facts, "function effect contract changed");
    }
    literal_body(old)?;
    literal_body(new)?;
    Ok(())
}

fn literal_body(artifact: &FrontendArtifact) -> Result<i64> {
    ensure!(
        artifact.syntax.imports.is_empty() && artifact.syntax.native_blocks.is_empty(),
        "open image"
    );
    ensure!(
        artifact.ir.functions.len() == 1 && artifact.syntax.declarations.len() == 1,
        "unsupported image"
    );
    let function = &artifact.ir.functions[0];
    ensure!(
        !function.is_async
            && function.parameter_types.is_empty()
            && function.result_type == Type::Int,
        "unsupported signature"
    );
    let Declaration::Function(declaration) = &artifact.syntax.declarations[0] else {
        return Err(anyhow!("unsupported declaration"));
    };
    ensure!(declaration.body.statements.len() == 1, "unsupported body");
    let StatementKind::Return(Some(expression)) = &declaration.body.statements[0].kind else {
        return Err(anyhow!("unsupported return"));
    };
    match &expression.kind {
        ExpressionKind::Literal(Literal::Int(value)) => Ok(*value),
        ExpressionKind::Literal(Literal::DecimalInteger(value)) => Ok(value.parse()?),
        _ => Err(anyhow!("only checked Int literal bodies are supported")),
    }
}

struct Image {
    module: Option<JITModule>,
    version: Arc<CodeVersion>,
    source: SourceFile,
    memory: JitMemoryAuditSnapshot,
    freed: Rc<Cell<usize>>,
}

impl Image {
    fn compile(
        generation: u64,
        source: SourceFile,
        artifact: &FrontendArtifact,
        freed: Rc<Cell<usize>>,
    ) -> Result<Rc<Self>> {
        let value = literal_body(artifact)?;
        let mut builder = JITBuilder::with_isa(crate::native_isa(false)?, default_libcall_names());
        let (memory, audit) = MacOsArm64JitMemory::new_with_audit()?;
        builder.memory_provider(Box::new(memory));
        let mut module = JITModule::new(builder);
        let mut signature = module.make_signature();
        signature.returns.push(AbiParam::new(types::I64));
        let function = module.declare_function("entry", Linkage::Export, &signature)?;
        let mut context = module.make_context();
        context.func.name = UserFuncName::user(0, function.as_u32());
        context.func.signature = signature;
        let mut builder_context = FunctionBuilderContext::new();
        {
            let target = module.target_config();
            let mut builder = FunctionBuilder::new(&mut context.func, &mut builder_context);
            let block = builder.create_block();
            builder.switch_to_block(block);
            builder.seal_block(block);
            let result = builder.ins().iconst(types::I64, value);
            builder.ins().return_(&[result]);
            builder.finalize(target);
        }
        module.define_function(function, &mut context)?;
        let size = context
            .compiled_code()
            .expect("compiled image")
            .code_buffer()
            .len();
        module.finalize_definitions()?;
        let address = module.get_finalized_function(function) as usize;
        let version = CodeVersion::published(CodeVersionId(generation), address, size)?;
        let memory = audit.snapshot();
        ensure!(
            memory.executable_mappings > 0
                && memory.icache_flushes == memory.executable_mappings
                && memory.rx_transitions == memory.executable_mappings
                && memory.rwx_mappings == 0,
            "publication violated W^X"
        );
        Ok(Rc::new(Self {
            module: Some(module),
            version,
            source,
            memory,
            freed,
        }))
    }

    fn pin(self: &Rc<Self>) -> Result<BoundEntry> {
        Ok(BoundEntry {
            frame: self.version.enter()?,
            image: Rc::clone(self),
        })
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        // A bound entry owns both a frame and this image. Its frame drops first,
        // so the last image owner may retire and unmap the executable safely.
        assert_eq!(self.version.live_frames(), 0);
        self.version.request_retire();
        assert_eq!(self.version.state(), CodeVersionState::Retired);
        // SAFETY: this is the last image owner; no callable/pin survives it.
        unsafe { self.module.take().expect("live image").free_memory() };
        self.freed.set(self.freed.get() + 1);
    }
}

struct BoundEntry {
    frame: CodeFrame,
    image: Rc<Image>,
}

impl BoundEntry {
    fn invoke(&self) -> i64 {
        assert_eq!(self.frame.version().id(), self.image.version.id());
        // SAFETY: the closed classifier/lowering emits this exact C signature.
        // This bound entry pins the owning image throughout the call.
        let callable: unsafe extern "C" fn() -> i64 =
            unsafe { std::mem::transmute(self.image.version.address()) };
        // SAFETY: this function takes no parameters and accesses no heap/native state.
        unsafe { callable() }
    }
}

#[test]
fn body_classifier_rejects_contract_and_unsupported_body_changes() {
    let (_, old) = checked("fn entry() -> Int { return 41; }");
    let (_, new) = checked("fn entry() -> Int { return 42; }");
    body_compatible(&old, &new).unwrap();
    for text in [
        "fn entry() -> Bool { return true; }",
        "fn entry(n: Int) -> Int { return n; }",
        "fn entry() -> Int { return 42; } fn extra() -> Int { return 0; }",
        "fn entry() -> Int { return 40 + 2; }",
    ] {
        assert!(body_compatible(&old, &checked(text).1).is_err(), "{text}");
    }
    // Boundary mutation vectors operate on checked IR, not invented valid source.
    let mut effect = new.clone();
    effect.ir.functions[0].facts.may_spawn = true;
    assert!(
        body_compatible(&old, &effect)
            .unwrap_err()
            .to_string()
            .contains("effect")
    );
    let mut layout = new.clone();
    layout.ir.source_types.insert(
        "Data".into(),
        rinzu_compiler::SourceTypeSchema {
            kind: "class".into(),
            fields: vec![("value".into(), Type::Int)],
            const_fields: vec![],
            variants: vec![],
        },
    );
    assert!(
        body_compatible(&old, &layout)
            .unwrap_err()
            .to_string()
            .contains("data layout")
    );
    let mut native = new.clone();
    native.ir.native_records.insert(
        "NativeData".into(),
        vec![("value".into(), rinzu_compiler::AbiScalar::I32)],
    );
    assert!(
        body_compatible(&old, &native)
            .unwrap_err()
            .to_string()
            .contains("native layout")
    );
}

#[test]
fn real_dispatch_preserves_bound_old_entries_and_reclaims_last_image_owner() -> Result<()> {
    let (old_source, old_artifact) = checked("fn entry() -> Int { return 41; }");
    let (new_source, new_artifact) = checked("fn entry() -> Int { return 42; }");
    body_compatible(&old_artifact, &new_artifact)?;
    let freed = Rc::new(Cell::new(0));
    let old = Image::compile(1, old_source, &old_artifact, Rc::clone(&freed))?;
    let old_version = Arc::clone(&old.version);
    let old_weak = Rc::downgrade(&old);
    let cell = VersionedDispatchCell::new();
    assert_eq!(cell.publish(old.version.address())?.version, 1);
    let function_value = old.pin()?;
    let prepared_callback = old.pin()?;
    let active_frame = old.pin()?;
    assert_eq!(active_frame.invoke(), 41);
    let new = Image::compile(2, new_source, &new_artifact, Rc::clone(&freed))?;
    let current = cell.publish(new.version.address())?;
    assert_eq!(current.version, 2);
    assert_eq!(cell.load()?, current);
    assert_eq!(current.address, new.version.address());
    drop(old); // dispatch ownership replaced; bound entries still own old code.
    let fresh_named_entry = new.pin()?;
    assert_eq!(fresh_named_entry.invoke(), 42);
    assert_eq!(function_value.invoke(), 41);
    assert_eq!(prepared_callback.invoke(), 41);
    assert_eq!(active_frame.invoke(), 41);
    assert!(function_value.image.source.text().contains("41"));
    assert!(fresh_named_entry.image.source.text().contains("42"));
    assert_eq!(old_version.live_frames(), 3);
    assert_eq!(freed.get(), 0);
    drop(active_frame);
    drop(function_value);
    assert!(old_weak.upgrade().is_some());
    assert_eq!(prepared_callback.invoke(), 41);
    assert_eq!(old_version.live_frames(), 1);
    let memory = prepared_callback.image.memory;
    drop(prepared_callback);
    assert!(old_weak.upgrade().is_none());
    assert_eq!(old_version.state(), CodeVersionState::Retired);
    assert_eq!(freed.get(), 1);
    drop(fresh_named_entry);
    drop(new);
    assert_eq!(freed.get(), 2);
    println!(
        "M5_RELOAD_SPIKE {}",
        serde_json::json!({"oldResults": [41,41,41,41], "newResult":42, "dispatchGenerations":[1,2], "oldPeakFrames":old_version.peak_live_frames(), "imagesFreed":freed.get(), "executableMappingsPerImage":memory.executable_mappings, "rwxMappings":memory.rwx_mappings, "scope":"serialized closed scalar machine-code experiment; callback is a bound-entry model, not a production C trampoline"})
    );
    Ok(())
}

// This is a host C-ABI bridge with a pinned context, not the production SDK
// trampoline or a cross-thread native admission.
unsafe extern "C" fn pinned_native_entry(context: *const BoundEntry) -> i64 {
    // SAFETY: the caller below retains the BoundEntry at this exact address.
    unsafe { context.as_ref().expect("pinned context") }.invoke()
}

struct RetainedResource {
    generation: CodeVersionId,
    entry: BoundEntry,
    live: Rc<Cell<usize>>,
}

impl RetainedResource {
    fn new(image: &Rc<Image>, live: Rc<Cell<usize>>) -> Result<Self> {
        let entry = image.pin()?;
        live.set(live.get() + 1);
        Ok(Self {
            generation: image.version.id(),
            entry,
            live,
        })
    }

    fn accepts(&self, entry: &BoundEntry) -> bool {
        self.generation == entry.image.version.id()
    }
}

impl Drop for RetainedResource {
    fn drop(&mut self) {
        assert_eq!(self.generation, self.entry.image.version.id());
        self.live.set(self.live.get() - 1);
    }
}

#[test]
fn retained_categories_keep_code_context_resource_and_source_in_one_generation() -> Result<()> {
    let (old_source, old_artifact) = checked("fn entry() -> Int { return 101; }");
    let (new_source, new_artifact) = checked("fn entry() -> Int { return 202; }");
    body_compatible(&old_artifact, &new_artifact)?;
    let freed = Rc::new(Cell::new(0));
    let old = Image::compile(1, old_source, &old_artifact, Rc::clone(&freed))?;
    let weak = Rc::downgrade(&old);
    let active = old.pin()?;
    let suspended = old.pin()?;
    let pending_child = old.pin()?;
    let closure_entry = old.pin()?;
    let captured_environment = Box::new([7_i64, 11]);
    let closure = move || closure_entry.invoke() + captured_environment.iter().sum::<i64>();
    let native_context = Box::new(old.pin()?);
    let native_pointer = std::ptr::from_ref(native_context.as_ref());
    let live = Rc::new(Cell::new(0));
    let resource = RetainedResource::new(&old, Rc::clone(&live))?;
    let old_version = Arc::clone(&old.version);
    assert_eq!(old_version.live_frames(), 6);
    let new = Image::compile(2, new_source, &new_artifact, Rc::clone(&freed))?;
    let cell = VersionedDispatchCell::new();
    cell.publish(old.version.address())?;
    cell.publish(new.version.address())?;
    let fresh = new.pin()?;
    assert_eq!(cell.load()?.address, fresh.image.version.address());
    drop(old);
    assert_eq!(active.invoke(), 101);
    assert_eq!(suspended.invoke(), 101); // explicit host suspension/resume model
    assert_eq!(pending_child.invoke(), 101); // host pending-entry model
    assert_eq!(closure(), 119);
    // SAFETY: native_context is pinned and not moved/dropped across this call.
    assert_eq!(unsafe { pinned_native_entry(native_pointer) }, 101);
    assert_eq!(fresh.invoke(), 202);
    assert!(resource.accepts(&active));
    assert!(!resource.accepts(&fresh));
    assert_eq!(active.image.source.path(), fresh.image.source.path());
    assert!(active.image.source.text().contains("101"));
    assert!(!active.image.source.text().contains("202"));
    assert!(fresh.image.source.text().contains("202"));
    drop(active);
    drop(suspended);
    drop(pending_child);
    drop(closure);
    drop(native_context);
    assert_eq!(old_version.live_frames(), 1);
    assert_eq!(live.get(), 1);
    assert_eq!(freed.get(), 0);
    assert!(weak.upgrade().is_some());
    drop(resource);
    assert_eq!(live.get(), 0);
    assert_eq!(old_version.state(), CodeVersionState::Retired);
    assert!(weak.upgrade().is_none());
    assert_eq!(freed.get(), 1);
    drop(fresh);
    drop(new);
    assert_eq!(freed.get(), 2);
    println!(
        "M5_RELOAD_RETENTION {}",
        serde_json::json!({"categories":["host active entry","host suspended entry","host pending child","Rust captured closure","host C-ABI pinned pointer","generation-bound resource"],"oldResult":101,"closureResult":119,"newResult":202,"oldPeakFrames":old_version.peak_live_frames(),"crossGenerationResourceAccepted":false,"liveResourcesAtEnd":live.get(),"imagesFreed":freed.get(),"productionAsyncNativeIntegration":false})
    );
    Ok(())
}
