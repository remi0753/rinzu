//! Rinzu 0.5 frontend.

/// Grammar and meaning used by parse/check, independently of a legacy manifest version.
pub const LANGUAGE_SEMANTICS_REVISION: &str = "rinzu-language-0.5-r82";

pub mod ast;
mod ast_walk;
mod closures;
mod collections;
mod compilation;
mod core;
mod json;
mod lambdas;
mod native_inputs;
mod records;
mod runtime_library;
pub mod standard_library;
mod stdio;
pub use core::{CoreLengthKind, TaskControlKind};
pub use native_inputs::NativeCliInputs;
pub use rinzu_abi::data_operation::DataOperation;
pub use runtime_library::RuntimeFunction;
mod constants;
mod diagnostic;
mod documentation;
mod editor_index;
mod error_effects;
mod fallback;
mod formatter;
mod generics;
mod interfaces;
mod ir;
mod lexer;
mod manifest;
mod methods;
mod native_interface;
mod package;
mod package_metadata;
mod package_templates;
mod package_tests;
mod parser;
mod scalar;
mod sema;
mod source;
mod source_locations;
mod source_provider;
pub mod timing;
mod value_constants;

pub use ast::{BinaryOperator, UnaryOperator};
pub use ast::{Module, TypeRef};
pub use compilation::{
    CompilationContext, CompilationInput, CompilationMode, CompilationOptions, check_input,
    check_input_with_sources,
};
pub use constants::{IntegerConstant, evaluate_module_constants};
pub use diagnostic::{Applicability, Diagnostic, DiagnosticCode, Severity, Suggestion};
pub use documentation::{
    SourceDeclarationDocumentation, SourceMemberDocumentation, source_documentation,
};
pub use editor_index::{EditorIndex, EditorReference, EditorSymbol, build_editor_index};
pub use fallback::FallbackConstant;
pub use formatter::format_source;
pub use ir::{
    BasicBlock, BlockId, CallbackArgumentPlan, ConstantValue, ErrorEdge, IrFunction, IrModule,
    NativeCallId, NativeEntryDomain, Operation, OperationKind, PollReason, ReservationId,
    ScopeInfo, StackMap, StackRoot, Terminator, TypedValue, UnwindPlan, ValueId, dead_local_roots,
    dead_temporary_roots, fresh_return_consuming_awaits, proven_data_owner_operations,
    proven_frame_memberships, proven_frame_roots, verify_module, virtual_bytes_options,
};
pub use manifest::{
    Manifest, ManifestError, NativeManifest, PackageExports, PackageKind, PathDependency,
    canonicalize_native_inputs, find_manifest, load_manifest,
};
pub use native_interface::{
    SourceAbiType, SourceNativeSignature, source_native_signatures, validate_native_declarations,
};
pub use package::{
    ResolvedSdkPackage, ResolvedSourceImport, ResolvedSourceModule, SourceNativeInterface,
    SourceSdkInterface,
};
pub use package_metadata::{NativeImport, canonical_import_key, fnv1a64};
pub use package_templates::SourceTemplateSchema;
pub use package_tests::{PackageTestCase, PackageTestOptions, check_package_tests};
pub use scalar::AbiScalar;
pub use sema::{
    CheckedFunction, CheckedModule, CompilerOptions, ExternalType, FunctionFacts,
    FunctionReferenceType, NativeCallClass, NativeFunction, SourceTypeSchema, StaticHandlerType,
    SupplantCall, Type, check_module, check_module_with_options,
};
pub use source::{FileId, SourceFile, SourceRange, Utf16Position};
pub use source_locations::SourceCoordinates;
pub use source_provider::{SourceCache, SourceSnapshot};

/// Complete result of compiling one Rinzu source file through verified typed IR.
#[derive(Clone, Debug)]
pub struct FrontendArtifact {
    /// File input has no package lock, even if one is present near its sources.
    pub is_file_input: bool,
    pub native_inputs: NativeCliInputs,
    pub syntax: Module,
    pub checked: CheckedModule,
    pub ir: IrModule,
    pub sdk_packages: Vec<ResolvedSdkPackage>,
    /// Root owner's checked interface; application presence does not make it an SDK.
    pub source_interface: Option<package::SourceNativeInterface>,
    pub module_graph: Vec<ResolvedSourceModule>,
    /// Disk execution must still match this frontend's source/configuration snapshot.
    pub checked_inputs: std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>,
}

/// Parse syntax independently of semantic/backend support, retaining original byte spans.
pub fn parse_source(file: &SourceFile) -> Result<Module, Vec<Diagnostic>> {
    parser::parse(file.id(), &lexer::lex(file)?)
}

/// Compile one source file. An artifact is never returned alongside an error.
pub fn compile_source(file: &SourceFile) -> Result<FrontendArtifact, Vec<Diagnostic>> {
    compile_source_with_options(file, &CompilerOptions::default())
}

/// Compile with generated-binding metadata. Application source cannot create these options.
pub fn compile_source_with_options(
    file: &SourceFile,
    options: &CompilerOptions,
) -> Result<FrontendArtifact, Vec<Diagnostic>> {
    let tokens = lexer::lex(file)?;
    let syntax = parser::parse(file.id(), &tokens)?;
    let checked = check_module_with_options(&syntax, options)?;
    let mut ir = ir::lower(&checked);
    ir.source_files
        .insert(file.id().0, file.path().display().to_string());
    ir.embed_source_locations([file]);
    verify_module(&ir)?;
    Ok(FrontendArtifact {
        is_file_input: true,
        native_inputs: NativeCliInputs::default(),
        syntax,
        checked,
        ir,
        sdk_packages: Vec::new(),
        source_interface: None,
        module_graph: Vec::new(),
        checked_inputs: std::collections::BTreeMap::new(),
    })
}

/// Load the package entry file and compile it through verified typed IR.
pub fn check_package(
    start: &std::path::Path,
) -> Result<(Manifest, FrontendArtifact), Vec<Diagnostic>> {
    package::check(start)
}

pub fn check_package_with_native_inputs(
    start: &std::path::Path,
    inputs: &NativeCliInputs,
) -> Result<(Manifest, FrontendArtifact), Vec<Diagnostic>> {
    let (mut manifest, mut artifact) = package::check(start)?;
    artifact.native_inputs = inputs.normalize(&manifest)?;
    manifest
        .native
        .headers
        .extend(artifact.native_inputs.headers.clone());
    Ok((manifest, artifact))
}

/// Check the existing package graph from a read-only source snapshot.
pub fn check_package_with_sources(
    start: &std::path::Path,
    sources: &mut SourceSnapshot<'_>,
) -> Result<(Manifest, FrontendArtifact), Vec<Diagnostic>> {
    package::check_with_sources(start, sources)
}
