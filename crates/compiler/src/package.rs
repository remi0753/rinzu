use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::{
    Arguments, Block, Declaration, Export, Expression, ExpressionKind, Module, NativeMember,
    Statement, StatementKind, TypeRef, TypeRefKind,
};
use crate::{
    CompilerOptions, Diagnostic, DiagnosticCode, FileId, FrontendArtifact, Manifest, NativeImport,
    PackageKind, SourceCache, SourceRange, SourceSnapshot, Type, check_module_with_options,
    find_manifest, verify_module,
};

#[derive(Clone, Debug)]
pub struct ResolvedSdkPackage {
    pub manifest: Manifest,
    pub native_imports: Vec<NativeImport>,
    pub source: Option<SourceSdkInterface>,
}

/// Checked source/native contracts shared by applications, files and SDKs.
#[derive(Clone, Debug)]
pub struct SourceNativeInterface {
    pub syntax: Module,
    pub modules: HashMap<String, HashMap<String, String>>,
    pub signatures: Vec<crate::SourceNativeSignature>,
    pub functions: Vec<crate::CheckedFunction>,
    /// Checked lowered bodies, including dependency targets used by callback sites.
    pub lowered_functions: Vec<crate::IrFunction>,
    /// Checked call-site contracts, including dependency declarations.
    /// Only `signatures` declares this interface's own native exports.
    pub lowered_native_signatures: Vec<crate::SourceNativeSignature>,
    pub types: std::collections::BTreeMap<String, crate::SourceTypeSchema>,
    /// Checked concrete alias targets; public names still resolve through syntax.
    pub type_aliases: std::collections::BTreeMap<String, Type>,
    pub templates: std::collections::BTreeMap<String, crate::SourceTemplateSchema>,
    pub source_inputs: std::collections::BTreeMap<String, Vec<u8>>,
    pub source_files: std::collections::BTreeMap<u32, String>,
    pub source_locations:
        std::collections::BTreeMap<u32, std::collections::BTreeMap<u32, crate::SourceCoordinates>>,
    pub native_records: std::collections::BTreeMap<String, Vec<(String, crate::AbiScalar)>>,
    pub constants: std::collections::BTreeMap<String, crate::IntegerConstant>,
    pub values: std::collections::BTreeMap<String, Type>,
    pub module_graph: Vec<ResolvedSourceModule>,
}

/// Compatibility name for SDK consumers of the shared interface.
pub type SourceSdkInterface = SourceNativeInterface;

/// Physical module identity and the bindings consumed by this frontend.
#[derive(Clone, Debug)]
pub struct ResolvedSourceModule {
    pub path: PathBuf,
    pub key: String,
    pub file_id: FileId,
    pub is_entry: bool,
    pub imports: Vec<ResolvedSourceImport>,
    pub exports: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct ResolvedSourceImport {
    pub qualifier: String,
    pub target: String,
    pub target_path: Option<PathBuf>,
    /// Absolute spelling consumed by resolution, retained for symlink invalidation.
    pub requested_path: Option<PathBuf>,
    pub kind: &'static str,
    pub range: SourceRange,
    pub path_range: Option<SourceRange>,
    pub alias_range: Option<SourceRange>,
    pub public_symbols: BTreeMap<String, String>,
}

pub(crate) fn check(start: &Path) -> Result<(Manifest, FrontendArtifact), Vec<Diagnostic>> {
    let mut cache = SourceCache::default();
    let mut sources = SourceSnapshot::new(&mut cache, Default::default());
    check_with_sources(start, &mut sources)
}

pub(crate) fn check_with_sources(
    start: &Path,
    sources: &mut SourceSnapshot<'_>,
) -> Result<(Manifest, FrontendArtifact), Vec<Diagnostic>> {
    check_namespaced(start, "", sources)
}

fn builtin_symbols() -> HashMap<String, HashMap<String, String>> {
    [
        ("rinzu.core", crate::core::FUNCTIONS.as_slice()),
        ("rinzu.list", crate::collections::LIST_SYMBOLS),
        ("rinzu.map", crate::collections::MAP_SYMBOLS),
        ("rinzu.stdio", crate::stdio::FUNCTIONS.as_slice()),
        (
            "rinzu.timer",
            crate::runtime_library::TIMER_FUNCTIONS.as_slice(),
        ),
    ]
    .into_iter()
    .map(|(module, functions)| {
        (
            module.to_owned(),
            functions
                .iter()
                .map(|name| (name.to_string(), format!("{module}::{name}")))
                .collect(),
        )
    })
    .collect()
}

pub(crate) fn check_file_with_sources(
    path: &Path,
    sources: &mut SourceSnapshot<'_>,
) -> Result<FrontendArtifact, Vec<Diagnostic>> {
    let mut external = builtin_symbols();
    let mut loader = Loader::file(
        path,
        crate::standard_library::names()
            .map(str::to_owned)
            .collect(),
        sources,
    );
    loader.load("entry", path.to_owned(), None)?;
    let sdk_packages = resolve_standard_imports(&loader.modules, "0.5", None, loader.sources)?;
    extend_standard_symbols(&mut external, &sdk_packages);
    let source_inputs = loader.source_inputs;
    let (mut syntax, mut module_graph) = link_modules("entry", loader.modules, "", &external)?;
    let own_syntax = syntax.clone();
    let own_graph = module_graph.clone();
    let mut options = CompilerOptions::default();
    append_sdk_declarations(&mut syntax, &mut options, &sdk_packages);
    for package in &sdk_packages {
        if let Some(source) = &package.source {
            module_graph.extend(source.module_graph.iter().cloned().map(|mut node| {
                node.is_entry = false;
                node
            }));
        }
    }
    let sema_timing =
        crate::timing::span("frontend.sema").detail(path.to_str().unwrap_or("<file>"));
    let checked = check_module_with_options(&syntax, &options)
        .map_err(|errors| import_context(errors, &module_graph))?;
    drop(sema_timing);
    let lower_timing =
        crate::timing::span("frontend.lower").detail(path.to_str().unwrap_or("<file>"));
    let mut ir = crate::ir::lower(&checked);
    drop(lower_timing);
    ir.source_files.extend(
        module_graph
            .iter()
            .map(|m| (m.file_id.0, m.path.display().to_string())),
    );
    for package in &sdk_packages {
        if let Some(source) = &package.source {
            ir.source_locations.extend(source.source_locations.clone());
        }
    }
    ir.embed_source_locations(sources.sources().values());
    let source_interface = if own_syntax.native_blocks.is_empty() {
        None
    } else {
        let mut source = SourceNativeInterface {
            syntax: own_syntax.clone(),
            modules: HashMap::new(),
            signatures: Vec::new(),
            functions: Vec::new(),
            lowered_functions: Vec::new(),
            lowered_native_signatures: Vec::new(),
            types: BTreeMap::new(),
            type_aliases: BTreeMap::new(),
            templates: BTreeMap::new(),
            source_inputs,
            source_files: ir.source_files.clone(),
            source_locations: Default::default(),
            native_records: BTreeMap::new(),
            constants: BTreeMap::new(),
            values: BTreeMap::new(),
            module_graph: own_graph,
        };
        complete_source_interface(&mut source, &checked, &ir, &own_syntax);
        Some(source)
    };
    retain_reachable_functions(&mut ir, vec!["main".to_owned()]);
    verify_module(&ir)?;
    Ok(FrontendArtifact {
        is_file_input: true,
        native_inputs: crate::NativeCliInputs::default(),
        syntax,
        checked,
        ir,
        sdk_packages,
        source_interface,
        module_graph,
        checked_inputs: sources.input_bytes(),
    })
}

fn append_sdk_declarations(
    syntax: &mut Module,
    package_options: &mut CompilerOptions,
    sdk_packages: &[ResolvedSdkPackage],
) {
    for package in sdk_packages {
        if let Some(source) = &package.source {
            package_options.exported_methods.extend(
                source
                    .modules
                    .values()
                    .flat_map(|exports| exports.values())
                    .filter(|name| crate::methods::parts(name).is_some())
                    .cloned(),
            );
            package_options.sdk_files.insert(source.syntax.file_id);
            package_options.sdk_files.extend(
                source
                    .syntax
                    .declarations
                    .iter()
                    .map(|declaration| declaration.range().file_id),
            );
            package_options.sdk_files.extend(
                source
                    .syntax
                    .native_blocks
                    .iter()
                    .map(|block| block.range.file_id),
            );
            syntax
                .declarations
                .extend(source.syntax.declarations.clone());
            syntax
                .native_blocks
                .extend(source.syntax.native_blocks.clone());
        }
    }
}

fn snapshot_manifest(
    path: &Path,
    sources: &mut SourceSnapshot<'_>,
) -> Result<Manifest, Vec<Diagnostic>> {
    let bytes = sources.read_config(path).map_err(|error| {
        vec![package_diagnostic(
            format!("{}: {error}", path.display()),
            "make the manifest readable",
        )]
    })?;
    let text = std::str::from_utf8(&bytes).map_err(|error| {
        vec![package_diagnostic(
            format!("{}: {error}", path.display()),
            "save the manifest as UTF-8",
        )]
    })?;
    crate::manifest::load_manifest_text(path, text).map_err(|error| vec![error.into_diagnostic()])
}

fn check_namespaced(
    start: &Path,
    namespace: &str,
    sources: &mut SourceSnapshot<'_>,
) -> Result<(Manifest, FrontendArtifact), Vec<Diagnostic>> {
    let (manifest, artifact, _) = check_request(start, namespace, sources, None)?;
    Ok((manifest, artifact))
}

pub(crate) fn check_tests(
    start: &Path,
    sources: &mut SourceSnapshot<'_>,
    tests: &crate::PackageTestOptions,
) -> Result<(Manifest, FrontendArtifact, Vec<crate::PackageTestCase>), Vec<Diagnostic>> {
    check_request(start, "", sources, Some(tests))
}

fn check_request(
    start: &Path,
    namespace: &str,
    sources: &mut SourceSnapshot<'_>,
    tests: Option<&crate::PackageTestOptions>,
) -> Result<(Manifest, FrontendArtifact, Vec<crate::PackageTestCase>), Vec<Diagnostic>> {
    let manifest_path = find_manifest(start).map_err(|error| vec![error.into_diagnostic()])?;
    let mut manifest = snapshot_manifest(&manifest_path, sources)?;
    crate::canonicalize_native_inputs(&mut manifest)
        .map_err(|error| vec![error.into_diagnostic()])?;
    let _timing = crate::timing::span("frontend.package").detail(&manifest.name);
    let entry_module = module_name_from_entry(&manifest.entry)?;
    let mut sdk_packages = if manifest.language != "0.1" {
        resolve_sdk_packages(&manifest, sources)?
    } else {
        Vec::new()
    };
    // Keep the original library's source/native interface independent of test entries.
    let original_sdk = if tests.is_some() && manifest.kind == PackageKind::Sdk {
        check_namespaced(start, namespace, sources)?
            .1
            .source_interface
    } else {
        None
    };
    let mut package_options = CompilerOptions::default();
    let direct: HashSet<_> = manifest
        .dependencies
        .iter()
        .map(|dependency| dependency.name.as_str())
        .collect();
    let mut external_symbols = HashMap::new();
    let mut external_packages = HashSet::new();
    for package in &sdk_packages {
        if !direct.contains(package.manifest.name.as_str()) {
            continue;
        }
        if let Some(source) = &package.source {
            external_packages.extend(source.modules.keys().cloned());
            for (name, symbols) in &source.modules {
                if crate::standard_library::registered(name) {
                    return Err(vec![package_diagnostic(
                        format!(
                            "dependency public module `{name}` conflicts with a registered standard module"
                        ),
                        "rename the additional SDK public module",
                    )]);
                }
                if external_symbols
                    .insert(name.clone(), symbols.clone())
                    .is_some()
                {
                    return Err(vec![package_diagnostic(
                        format!("ambiguous dependency public module `{name}`"),
                        "use unambiguous package and public module names",
                    )]);
                }
            }
        } else {
            external_packages.insert(package.manifest.name.clone());
        }
    }
    external_packages.extend(crate::standard_library::names().map(str::to_owned));
    external_symbols.extend(builtin_symbols());
    if let Some(source) = &original_sdk {
        for (name, symbols) in &source.modules {
            if name != &entry_module {
                external_packages.insert(name.clone());
                external_symbols.insert(name.clone(), symbols.clone());
            }
        }
    }
    let mut loader = Loader::new(&manifest, &entry_module, external_packages, sources);
    let mut public_files = HashSet::new();
    let mut public_bindings = BTreeMap::new();
    loader.load(&entry_module, manifest.root.join(&manifest.entry), None)?;
    if manifest.kind == PackageKind::Sdk {
        let public_modules = if manifest.exports.modules.is_empty() {
            vec![entry_module.clone()]
        } else {
            manifest.exports.modules.clone()
        };
        let mut unique = HashSet::new();
        for name in &public_modules {
            if !unique.insert(name.clone()) {
                return Err(vec![package_diagnostic(
                    format!("duplicate public module `{name}`"),
                    "list each public module once",
                )]);
            }
            let key = loader.load(
                name,
                loader
                    .source_root
                    .join(name.replace('.', "/"))
                    .with_extension("rinzu"),
                None,
            )?;
            public_bindings.insert(name.clone(), key);
        }
        if !unique.contains(&entry_module) {
            return Err(vec![package_diagnostic(
                "public modules must include the SDK entry module",
                "include the entry module in exports.modules",
            )]);
        }
        let mut source_paths = Vec::new();
        collect_source_paths(&loader.source_root, &mut source_paths)?;
        source_paths.sort();
        for path in source_paths {
            loader.load_path(path, None)?;
        }
        public_files.extend(
            loader
                .modules
                .iter()
                .filter(|module| public_bindings.values().any(|key| key == &module.name))
                .map(|module| module.syntax.file_id),
        );
        package_options.is_library = true;
        package_options
            .sdk_files
            .extend(loader.modules.iter().map(|module| module.syntax.file_id));
    }
    if tests.is_some_and(|tests| tests.unit) && manifest.kind != PackageKind::Sdk {
        let mut paths = Vec::new();
        collect_source_paths(&loader.source_root, &mut paths)?;
        paths.sort();
        for path in paths {
            loader.load_path(path, None)?;
        }
    }
    let mut cases = Vec::new();
    let mut link_entry = entry_module.clone();
    if let Some(tests) = tests {
        if tests.unit {
            for module in &loader.modules {
                for declaration in &module.syntax.declarations {
                    if let Declaration::Function(function) = declaration
                        && function.name.starts_with("test_")
                    {
                        crate::package_tests::validate(function)?;
                        cases.push(crate::PackageTestCase {
                            identity: format!("unit:{}::{}", module.name, function.name),
                            kind: "unit",
                            path: loader
                                .sources
                                .path_for_id(function.range.file_id)
                                .unwrap()
                                .to_owned(),
                            range: function.range,
                            is_async: function.is_async,
                            target: if manifest.kind == PackageKind::Sdk
                                && module.name == entry_module
                            {
                                function.name.clone()
                            } else {
                                format!("{}::{}", module.name, function.name)
                            },
                        });
                    }
                }
            }
        }
        for document in
            crate::package_tests::documents(&tests.documents, &manifest.root, loader.sources)?
        {
            if loader.loaded.contains(&document.name) {
                return Err(vec![crate::package_tests::error(
                    document.syntax.range,
                    "reserved doc module name conflicts with source",
                )]);
            }
            validate_import_aliases(&document.syntax)?;
            let mut bindings = BTreeMap::new();
            let mut requested_paths = BTreeMap::new();
            for import in &document.syntax.imports {
                let qualifier = import.path.join(".");
                if let Some(path) = loader.requested_path(import, &document.case.path) {
                    requested_paths.insert(import.range.start_byte, path);
                }
                let target = loader.resolve_import(import, &document.case.path)?;
                if bindings
                    .insert(qualifier.clone(), target.clone())
                    .is_some_and(|previous| previous != target)
                {
                    return Err(vec![crate::package_tests::error(
                        import.range,
                        format!("conflicting import alias `{qualifier}`"),
                    )]);
                }
            }
            loader.loaded.insert(document.name.clone());
            loader.modules.push(ParsedModule {
                name: document.name,
                source_path: document.case.path.display().to_string(),
                canonical_path: document.case.path.clone(),
                bindings,
                requested_paths,
                syntax: document.syntax,
            });
            cases.push(document.case);
        }
        if cases.len() > 256 {
            return Err(vec![package_diagnostic(
                "test case limit exceeded (256)",
                "split the test package",
            )]);
        }
        cases.sort_by(|a, b| a.identity.cmp(&b.identity));
        if manifest.kind != PackageKind::Sdk {
            link_entry = "rinzu_test_entry".into();
            if loader.loaded.contains(&link_entry) {
                return Err(vec![package_diagnostic(
                    "reserved test entry conflicts with a source module",
                    "rename rinzu_test_entry",
                )]);
            }
            let range = loader.modules.first().unwrap().syntax.range;
            loader.modules.push(ParsedModule {
                name: link_entry.clone(),
                source_path: "<test entry>".into(),
                canonical_path: manifest.root.join("<test entry>"),
                bindings: BTreeMap::new(),
                requested_paths: BTreeMap::new(),
                syntax: Module {
                    file_id: FileId::MANIFEST,
                    imports: Vec::new(),
                    exports: Vec::new(),
                    declarations: Vec::new(),
                    native_blocks: Vec::new(),
                    range,
                },
            });
        }
    }
    let public_module_names: Vec<_> = public_bindings
        .keys()
        .map(|name| {
            if name == &entry_module {
                manifest.name.clone()
            } else {
                format!("{}.{}", manifest.name, name)
            }
        })
        .collect();
    let source_files: std::collections::BTreeMap<_, _> = loader
        .modules
        .iter()
        .map(|module| {
            (
                module.syntax.file_id.0,
                if Path::new(&module.source_path).is_absolute() {
                    module.source_path.clone()
                } else {
                    format!("{}/{}", manifest.name, module.source_path)
                },
            )
        })
        .collect();
    let standard_packages = resolve_standard_imports(
        &loader.modules,
        &manifest.language,
        original_sdk.as_ref().map(|_| manifest.name.as_str()),
        loader.sources,
    )?;
    extend_standard_symbols(&mut external_symbols, &standard_packages);
    merge_packages(&mut sdk_packages, standard_packages)?;
    let source_inputs = loader.source_inputs;
    let (mut syntax, mut module_graph) =
        link_modules(&link_entry, loader.modules, namespace, &external_symbols)?;
    // Test main is generated in memory. Keep its diagnostic label in
    // source_files, but do not require a physical `<test entry>` input receipt.
    if tests.is_some() {
        module_graph.retain(|node| node.file_id != FileId::MANIFEST);
    }
    if manifest.kind == PackageKind::Sdk {
        validate_public_signatures(&syntax, &public_files, &HashSet::new())?;
    }
    if let Some(tests) = tests {
        if syntax
            .declarations
            .iter()
            .any(|declaration| declaration.name() == "main")
        {
            return Err(vec![crate::package_tests::error(
                syntax.range,
                "SDK main declaration conflicts with the test entry; use a separate test application",
            )]);
        }
        let selected = tests
            .selected
            .as_ref()
            .map(|identity| {
                cases
                    .iter()
                    .find(|case| &case.identity == identity)
                    .ok_or_else(|| {
                        vec![crate::package_tests::error(
                            syntax.range,
                            format!("test case disappeared: {identity}"),
                        )]
                    })
            })
            .transpose()?;
        let range = selected.map_or(syntax.range, |case| case.range);
        syntax
            .declarations
            .push(crate::package_tests::entry(selected, range));
        // This synthetic entry is invoked by the test host, which observes its
        // normal Error and reports a failed case after physical settlement.
        // It is not a user Main boundary. Future obligations and Worker/native
        // boundaries still use the ordinary checker in library mode.
        package_options.is_library = true;
    }
    let mut source_interface = if manifest.kind == PackageKind::Sdk
        || !syntax.native_blocks.is_empty()
    {
        let mut modules: HashMap<String, HashMap<String, String>> = HashMap::new();
        for (public_name, key) in &public_bindings {
            let external = if public_name == &entry_module {
                manifest.name.clone()
            } else {
                format!("{}.{}", manifest.name, public_name)
            };
            let node = module_graph
                .iter()
                .find(|node| node.key == scoped_module(namespace, key))
                .expect("loaded public module");
            for (name, target) in &node.exports {
                let range = syntax
                    .exports
                    .iter()
                    .find(|export| {
                        export.range.file_id == node.file_id && export.path.join(".") == *target
                    })
                    .expect("effective public export")
                    .range;
                if external == manifest.name
                    && public_module_names
                        .iter()
                        .filter_map(|module| module.strip_prefix(&format!("{}.", manifest.name)))
                        .any(|module| module.split('.').next() == name.split('.').next())
                {
                    return Err(vec![Diagnostic::error(
                        DiagnosticCode::InvalidModulePath,
                        range,
                        format!("entry export `{name}` conflicts with a public module prefix"),
                        "rename the declaration or public module",
                    )]);
                }
                modules
                    .entry(external.clone())
                    .or_default()
                    .insert(name.clone(), target.clone());
            }
        }
        // Public modules without exports remain importable.
        for module in public_module_names {
            modules.entry(module).or_default();
        }
        Some(SourceSdkInterface {
            syntax: syntax.clone(),
            modules,
            signatures: Vec::new(),
            functions: Vec::new(),
            lowered_functions: Vec::new(),
            lowered_native_signatures: Vec::new(),
            types: std::collections::BTreeMap::new(),
            type_aliases: std::collections::BTreeMap::new(),
            templates: std::collections::BTreeMap::new(),
            values: std::collections::BTreeMap::new(),
            source_inputs,
            source_files: source_files.clone(),
            source_locations: Default::default(),
            native_records: std::collections::BTreeMap::new(),
            module_graph: module_graph.clone(),
            constants: std::collections::BTreeMap::new(),
        })
    } else {
        None
    };
    append_sdk_declarations(&mut syntax, &mut package_options, &sdk_packages);
    if package_options.native_functions.values().any(|function| {
        function.parameters.iter().any(
            |parameter| matches!(parameter, Type::StaticHandler(handler) if handler.owner_sync),
        )
    }) {
        package_options
            .owner_sync_handlers
            .extend(
                syntax
                    .declarations
                    .iter()
                    .filter_map(|declaration| match declaration {
                        Declaration::Function(function) if function.name != "main" => {
                            Some(function.name.clone())
                        }
                        _ => None,
                    }),
            );
    }
    let options = package_options;
    for package in &sdk_packages {
        if let Some(source) = &package.source {
            module_graph.extend(source.module_graph.iter().cloned().map(|mut node| {
                node.is_entry = false;
                node
            }));
        }
    }
    let sema_timing = crate::timing::span("frontend.sema").detail(&manifest.name);
    let checked = check_module_with_options(&syntax, &options)
        .map_err(|errors| import_context(errors, &module_graph))?;
    drop(sema_timing);
    let lower_timing = crate::timing::span("frontend.lower").detail(&manifest.name);
    let mut ir = crate::ir::lower(&checked);
    drop(lower_timing);
    ir.source_files.extend(source_files);
    for package in &sdk_packages {
        if let Some(source) = &package.source {
            ir.source_files.extend(source.source_files.clone());
            ir.source_locations.extend(source.source_locations.clone());
        }
    }
    if let Some(file) = checked.closure_source {
        ir.source_files
            .insert(file.0, "<managed closure conversion>".into());
    }
    let locations_timing = crate::timing::span("frontend.locations").detail(&manifest.name);
    ir.embed_source_locations(sources.sources().values());
    drop(locations_timing);
    if let Some(source) = &mut source_interface {
        complete_source_interface(source, &checked, &ir, &syntax);
    }
    let roots = if tests.is_none() && manifest.kind == PackageKind::Sdk {
        syntax
            .exports
            .iter()
            .filter(|export| public_files.contains(&export.range.file_id))
            .map(|export| export.path.join("."))
            .collect()
    } else {
        vec!["main".to_owned()]
    };
    let verify_timing = crate::timing::span("frontend.verify").detail(&manifest.name);
    retain_reachable_functions(&mut ir, roots);
    verify_module(&ir)?;
    drop(verify_timing);
    if let Some(source) = original_sdk {
        sdk_packages.push(ResolvedSdkPackage {
            manifest: manifest.clone(),
            native_imports: Vec::new(),
            source: Some(source),
        });
    }
    if tests.is_some() && manifest.kind == PackageKind::Sdk {
        source_interface = None;
    }
    Ok((
        manifest,
        FrontendArtifact {
            is_file_input: false,
            native_inputs: crate::NativeCliInputs::default(),
            syntax,
            checked,
            ir,
            sdk_packages,
            source_interface,
            module_graph,
            checked_inputs: sources.input_bytes(),
        },
        cases,
    ))
}

fn complete_source_interface(
    source: &mut SourceNativeInterface,
    checked: &crate::CheckedModule,
    ir: &crate::IrModule,
    syntax: &Module,
) {
    let own_functions: HashSet<_> = source
        .syntax
        .native_blocks
        .iter()
        .flat_map(|block| &block.members)
        .filter_map(|member| match member {
            NativeMember::Function(function) => Some(function.name.as_str()),
            _ => None,
        })
        .collect();
    source.signatures = checked
        .source_native_signatures
        .iter()
        .filter(|signature| own_functions.contains(signature.name.as_str()))
        .cloned()
        .collect();
    source.native_records = ir.native_records.clone();
    source.source_files = ir.source_files.clone();
    source.source_locations = ir.source_locations.clone();
    source.lowered_functions = ir.functions.clone();
    source.lowered_native_signatures = ir.source_native_signatures.clone();
    source.constants = checked.constants.clone();
    let own_names: HashSet<_> = source
        .syntax
        .declarations
        .iter()
        .filter_map(|declaration| {
            if let Declaration::Function(function) = declaration {
                Some(function.name.as_str())
            } else {
                None
            }
        })
        .collect();
    let exported: HashSet<_> = source
        .modules
        .values()
        .flat_map(|exports| exports.values().map(String::as_str))
        .collect();
    source.values = checked
        .functions
        .iter()
        .filter_map(|f| {
            let name = f.declaration.name.strip_prefix("rinzu.constant::")?;
            exported
                .contains(name)
                .then(|| (name.to_owned(), f.result_type.clone()))
        })
        .collect();
    source.type_aliases = checked
        .syntax
        .declarations
        .iter()
        .filter_map(|declaration| {
            let Declaration::TypeAlias(alias) = declaration else {
                return None;
            };
            exported.contains(alias.name.as_str()).then(|| {
                checked
                    .type_references
                    .get(&alias.ty.range)
                    .map(|ty| (alias.name.clone(), ty.clone()))
            })?
        })
        .collect();
    // Private storage still participates in layout, capabilities, native and
    // Future checks. Public bindings, not omission from this complete graph,
    // determine what a caller can name or construct.
    source.types = checked.source_types.clone();
    source.templates = syntax
        .declarations
        .iter()
        .filter_map(|declaration| {
            crate::package_templates::schema(declaration)
                .map(|schema| (declaration.name().to_owned(), schema))
        })
        .collect();
    let own_function = |function: &&crate::CheckedFunction| {
        own_names.contains(function.declaration.name.as_str())
            || exported.contains(function.declaration.name.as_str())
            || own_names.contains(
                function
                    .declaration
                    .name
                    .split('<')
                    .next()
                    .unwrap_or(&function.declaration.name),
            )
    };
    let reachable = reachable_function_names(
        ir,
        checked
            .functions
            .iter()
            .filter(own_function)
            .map(|function| function.declaration.name.clone())
            .collect(),
    );
    let own_files: HashSet<_> = source
        .syntax
        .declarations
        .iter()
        .map(Declaration::range)
        .map(|range| range.file_id)
        .collect();
    source.functions = checked
        .functions
        .iter()
        .filter(|function| {
            own_function(function)
                || reachable.contains(function.declaration.name.as_str())
                    && (Some(function.declaration.range.file_id) == checked.closure_source
                        || own_files.contains(&function.declaration.range.file_id))
        })
        .cloned()
        .collect();

    // Native ABI types, context records and private captures need their schema
    // even when the structural type has no exported alias or public binding.
    fn include_type(
        ty: &Type,
        checked: &crate::CheckedModule,
        types: &mut BTreeMap<String, crate::SourceTypeSchema>,
        included: &mut HashSet<String>,
    ) {
        match ty {
            Type::Named(name) | Type::Native { name, .. } if included.insert(name.clone()) => {
                if let Some(schema) = checked.source_types.get(name) {
                    types.insert(name.clone(), schema.clone());
                    for (_, ty) in schema
                        .fields
                        .iter()
                        .chain(schema.variants.iter().flat_map(|(_, fields)| fields))
                    {
                        include_type(ty, checked, types, included);
                    }
                }
            }
            Type::List(inner) | Type::Future(inner) => {
                include_type(inner, checked, types, included)
            }
            Type::Map(key, value) => {
                include_type(key, checked, types, included);
                include_type(value, checked, types, included);
            }
            _ => {}
        }
    }
    fn include_native_type(
        abi: &crate::SourceAbiType,
        checked: &crate::CheckedModule,
        types: &mut BTreeMap<String, crate::SourceTypeSchema>,
        included: &mut HashSet<String>,
    ) {
        match abi {
            crate::SourceAbiType::UserData(name)
            | crate::SourceAbiType::Record(name)
            | crate::SourceAbiType::Opaque(name) => {
                include_type(&Type::Named(name.clone()), checked, types, included)
            }
            crate::SourceAbiType::Callback { parameters, result } => {
                for parameter in parameters {
                    include_native_type(parameter, checked, types, included);
                }
                include_native_type(result, checked, types, included);
            }
            _ => {}
        }
    }
    let mut included = HashSet::new();
    for signature in &source.signatures {
        for abi in signature
            .parameters
            .iter()
            .chain(std::iter::once(&signature.result))
        {
            include_native_type(abi, checked, &mut source.types, &mut included);
        }
    }
    for operation in ir
        .functions
        .iter()
        .filter(|function| reachable.contains(function.name.as_str()))
        .flat_map(|function| &function.blocks)
        .flat_map(|block| &block.operations)
    {
        let callbacks = match &operation.kind {
            crate::OperationKind::PrepareNativeCall {
                signature,
                callbacks,
                ..
            } => {
                for abi in signature
                    .parameters
                    .iter()
                    .chain(std::iter::once(&signature.result))
                {
                    include_native_type(abi, checked, &mut source.types, &mut included);
                }
                callbacks
            }
            crate::OperationKind::ReserveSupplant {
                targets, callbacks, ..
            } => {
                for signature in targets.iter().filter_map(|target| {
                    ir.source_native_signatures
                        .iter()
                        .find(|signature| signature.name == *target)
                }) {
                    for abi in signature
                        .parameters
                        .iter()
                        .chain(std::iter::once(&signature.result))
                    {
                        include_native_type(abi, checked, &mut source.types, &mut included);
                    }
                }
                callbacks
            }
            _ => continue,
        };
        for callback in callbacks {
            if let Some(capture) = &callback.capture {
                include_type(capture, checked, &mut source.types, &mut included);
            }
        }
    }
}

fn reachable_function_names(module: &crate::IrModule, mut pending: Vec<String>) -> HashSet<String> {
    use crate::OperationKind;
    let functions: HashMap<_, _> = module
        .functions
        .iter()
        .map(|function| (function.name.as_str(), function))
        .collect();
    let mut reachable = HashSet::new();
    while let Some(name) = pending.pop() {
        if !reachable.insert(name.clone()) {
            continue;
        }
        let Some(function) = functions.get(name.as_str()) else {
            continue;
        };
        for operation in function.blocks.iter().flat_map(|block| &block.operations) {
            match &operation.kind {
                OperationKind::ManagedCall { target, .. }
                | OperationKind::Spawn { target, .. }
                | OperationKind::FunctionReference { target }
                | OperationKind::StaticHandlerReference { target } => pending.push(target.clone()),
                OperationKind::IndirectCall { targets, .. } => {
                    pending.extend(targets.iter().cloned())
                }
                OperationKind::ReserveSupplant {
                    targets, callbacks, ..
                } => {
                    pending.extend(targets.iter().cloned());
                    pending.extend(callbacks.iter().flat_map(|c| &c.targets).cloned());
                }
                OperationKind::PrepareNativeCall { callbacks, .. } => pending.extend(
                    callbacks
                        .iter()
                        .flat_map(|callback| &callback.targets)
                        .cloned(),
                ),
                _ => {}
            }
        }
    }
    reachable
}

fn retain_reachable_functions(module: &mut crate::IrModule, pending: Vec<String>) {
    let reachable = reachable_function_names(module, pending);
    module
        .functions
        .retain(|function| reachable.contains(&function.name));
}

pub(crate) fn validate_public_signatures(
    module: &Module,
    public_files: &HashSet<FileId>,
    external_files: &HashSet<FileId>,
) -> Result<(), Vec<Diagnostic>> {
    let public: HashSet<_> = module
        .exports
        .iter()
        .filter(|export| public_files.contains(&export.range.file_id))
        .map(|export| export.path.join("."))
        .collect();
    let mut types = HashSet::new();
    let parameters: HashMap<_, HashSet<_>> = module
        .declarations
        .iter()
        .map(|declaration| {
            let parameters = match declaration {
                Declaration::Interface(value) => &value.type_parameters,
                Declaration::Function(value) => &value.type_parameters,
                Declaration::Record(value) => &value.type_parameters,
                Declaration::Enum(value) => &value.type_parameters,
                Declaration::Error(value) => &value.type_parameters,
                Declaration::TypeAlias(value) => &value.type_parameters,
                Declaration::Constant(_) => return (declaration.name(), HashSet::new()),
            };
            (
                declaration.name(),
                parameters
                    .iter()
                    .map(|parameter| parameter.name.as_str())
                    .collect(),
            )
        })
        .collect();
    let mut signatures: Vec<(&str, Vec<&TypeRef>)> = Vec::new();
    for declaration in &module.declarations {
        if external_files.contains(&declaration.range().file_id) {
            continue;
        }
        let (parameters, interfaces) = match declaration {
            Declaration::Interface(v) => (v.type_parameters.as_slice(), &[][..]),
            Declaration::Function(v) => (v.type_parameters.as_slice(), &[][..]),
            Declaration::Record(v) => (v.type_parameters.as_slice(), v.interfaces.as_slice()),
            Declaration::Enum(v) => (v.type_parameters.as_slice(), v.interfaces.as_slice()),
            Declaration::Error(v) => (v.type_parameters.as_slice(), &[][..]),
            Declaration::TypeAlias(v) => (v.type_parameters.as_slice(), &[][..]),
            Declaration::Constant(_) => (&[][..], &[][..]),
        };
        signatures.push((
            declaration.name(),
            interfaces
                .iter()
                .chain(parameters.iter().flat_map(|p| &p.interfaces))
                .collect(),
        ));
        match declaration {
            Declaration::Interface(interface) => {
                types.insert(interface.name.clone());
                signatures.push((
                    &interface.name,
                    interface
                        .methods
                        .iter()
                        .flat_map(|method| {
                            method
                                .parameters
                                .iter()
                                .map(|p| &p.ty)
                                .chain(std::iter::once(&method.result))
                                .chain(method.error_contract.iter().flat_map(|c| &c.types))
                        })
                        .collect(),
                ));
            }
            Declaration::Function(function) => signatures.push((
                &function.name,
                function
                    .parameters
                    .iter()
                    .map(|parameter| &parameter.ty)
                    .chain(std::iter::once(&function.result))
                    .chain(function.error_contract.iter().flat_map(|c| &c.types))
                    .collect(),
            )),
            Declaration::Record(record) => {
                types.insert(record.name.clone());
                signatures.push((
                    &record.name,
                    record
                        .fields
                        .iter()
                        .filter(|field| !field.is_private)
                        .map(|field| &field.ty)
                        .collect(),
                ));
            }
            Declaration::Enum(enumeration) => {
                types.insert(enumeration.name.clone());
                signatures.push((
                    &enumeration.name,
                    enumeration
                        .variants
                        .iter()
                        .flat_map(|variant| &variant.fields)
                        .map(|field| &field.ty)
                        .collect(),
                ));
            }
            Declaration::Error(error) => {
                types.insert(error.name.clone());
                signatures.push((
                    &error.name,
                    error.payload.iter().map(|payload| &payload.ty).collect(),
                ));
            }
            Declaration::TypeAlias(alias) => {
                types.insert(alias.name.clone());
                signatures.push((&alias.name, vec![&alias.ty]));
            }
            Declaration::Constant(constant) => {
                if let Some(ty) = &constant.annotation {
                    signatures.push((&constant.name, vec![ty]));
                }
            }
        }
    }
    for member in module.native_blocks.iter().flat_map(|block| &block.members) {
        let range = match member {
            NativeMember::Opaque { range, .. } => *range,
            NativeMember::Record(record) => record.range,
            NativeMember::Function(function) => function.range,
        };
        if external_files.contains(&range.file_id) {
            continue;
        }
        match member {
            NativeMember::Opaque { name, .. } => {
                types.insert(name.clone());
            }
            NativeMember::Record(record) => {
                types.insert(record.name.clone());
                signatures.push((
                    &record.name,
                    record.fields.iter().map(|field| &field.ty).collect(),
                ));
            }
            NativeMember::Function(function) => signatures.push((
                &function.name,
                function
                    .parameters
                    .iter()
                    .map(|parameter| &parameter.ty)
                    .chain(std::iter::once(&function.result))
                    .collect(),
            )),
        }
    }
    fn inspect(
        ty: &TypeRef,
        types: &HashSet<String>,
        public: &HashSet<String>,
        parameters: &HashSet<&str>,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        match &ty.kind {
            TypeRefKind::Record(fields) => {
                for f in fields {
                    inspect(&f.ty, types, public, parameters, diagnostics);
                }
            }
            TypeRefKind::Generic { path, arguments } => {
                inspect(
                    &TypeRef {
                        kind: TypeRefKind::Named(path.clone()),
                        range: ty.range,
                    },
                    types,
                    public,
                    parameters,
                    diagnostics,
                );
                for argument in arguments {
                    inspect(argument, types, public, parameters, diagnostics);
                }
            }
            TypeRefKind::Named(path) => {
                let name = path.join(".");
                if types.contains(&name)
                    && !public.contains(&name)
                    && !parameters.contains(name.as_str())
                {
                    diagnostics.push(Diagnostic::error(
                        DiagnosticCode::InvalidModulePath,
                        ty.range,
                        format!("public signature contains private type `{name}`"),
                        "export the type from a public module",
                    ));
                }
            }
            TypeRefKind::List(value) | TypeRefKind::Future(value) => {
                inspect(value, types, public, parameters, diagnostics)
            }
            TypeRefKind::Map(key, value) => {
                inspect(key, types, public, parameters, diagnostics);
                inspect(value, types, public, parameters, diagnostics);
            }
            TypeRefKind::Function {
                parameters: arguments,
                result,
                ..
            } => {
                for parameter in arguments {
                    inspect(parameter, types, public, parameters, diagnostics);
                }
                inspect(result, types, public, parameters, diagnostics);
            }
            TypeRefKind::Bytes(_) | TypeRefKind::ConcreteBytes(_) => {}
        }
    }
    let mut diagnostics = Vec::new();
    let empty = HashSet::new();
    for (name, signature) in signatures {
        if public.contains(name) {
            for ty in signature {
                inspect(
                    ty,
                    &types,
                    &public,
                    parameters.get(name).unwrap_or(&empty),
                    &mut diagnostics,
                );
            }
        }
    }
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

fn collect_source_paths(root: &Path, paths: &mut Vec<PathBuf>) -> Result<(), Vec<Diagnostic>> {
    let canonical_root = root.canonicalize().map_err(|error| {
        vec![package_diagnostic(
            error.to_string(),
            "make SDK sources readable",
        )]
    })?;
    collect_source_directory(root, &canonical_root, &mut HashSet::new(), paths)
}

fn collect_source_directory(
    root: &Path,
    canonical_root: &Path,
    visiting: &mut HashSet<PathBuf>,
    paths: &mut Vec<PathBuf>,
) -> Result<(), Vec<Diagnostic>> {
    let canonical = root.canonicalize().map_err(|error| {
        vec![package_diagnostic(
            error.to_string(),
            "make SDK sources readable",
        )]
    })?;
    if !canonical.starts_with(canonical_root) || !visiting.insert(canonical.clone()) {
        return Err(vec![package_diagnostic(
            "SDK source directory escapes src or creates a symlink cycle",
            "keep source directories inside src without cycles",
        )]);
    }
    let entries = std::fs::read_dir(root).map_err(|error| {
        vec![package_diagnostic(
            format!(
                "cannot enumerate SDK source directory `{}`: {error}",
                root.display()
            ),
            "make the source directory readable",
        )]
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            vec![package_diagnostic(
                error.to_string(),
                "make SDK sources readable",
            )]
        })?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|error| {
            vec![package_diagnostic(
                error.to_string(),
                "make SDK sources readable",
            )]
        })?;
        if kind.is_dir() || (kind.is_symlink() && path.is_dir()) {
            collect_source_directory(&path, canonical_root, visiting, paths)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "rinzu")
        {
            paths.push(path);
        }
    }
    visiting.remove(&canonical);
    Ok(())
}

fn resolve_sdk_packages(
    root: &Manifest,
    sources: &mut SourceSnapshot<'_>,
) -> Result<Vec<ResolvedSdkPackage>, Vec<Diagnostic>> {
    let mut packages = Vec::new();
    let mut visited = HashMap::<PathBuf, String>::new();
    let mut visiting = Vec::<PathBuf>::new();
    resolve_dependencies(root, &mut packages, &mut visited, &mut visiting, sources)?;
    Ok(packages)
}

fn resolve_dependencies(
    parent: &Manifest,
    packages: &mut Vec<ResolvedSdkPackage>,
    visited: &mut HashMap<PathBuf, String>,
    visiting: &mut Vec<PathBuf>,
    sources: &mut SourceSnapshot<'_>,
) -> Result<(), Vec<Diagnostic>> {
    for dependency in &parent.dependencies {
        let root = parent.root.join(&dependency.path);
        let canonical = root.canonicalize().map_err(|error| {
            vec![package_diagnostic(
                format!(
                    "cannot resolve dependency `{}` at `{}`: {error}",
                    dependency.name,
                    root.display()
                ),
                "correct the path dependency or create the package directory",
            )]
        })?;
        if let Some(index) = visiting.iter().position(|path| path == &canonical) {
            let cycle = visiting[index..]
                .iter()
                .chain(std::iter::once(&canonical))
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(" -> ");
            return Err(vec![package_diagnostic(
                format!("package dependency cycle: {cycle}"),
                "remove a path dependency so the package graph is acyclic",
            )]);
        }
        if let Some(existing) = visited.get(&canonical) {
            if existing != &dependency.name {
                return Err(vec![package_diagnostic(
                    format!(
                        "dependency path `{}` is named both `{existing}` and `{}`",
                        canonical.display(),
                        dependency.name
                    ),
                    "use the package's declared name consistently",
                )]);
            }
            continue;
        }
        let manifest_path = canonical.join("rinzu.toml");
        let mut manifest = snapshot_manifest(&manifest_path, sources)?;
        crate::canonicalize_native_inputs(&mut manifest)
            .map_err(|error| vec![error.into_diagnostic()])?;
        if packages.iter().any(|package| {
            package.manifest.name == manifest.name
                && package.manifest.root.canonicalize().ok().as_ref() != Some(&canonical)
        }) {
            return Err(vec![package_diagnostic(
                format!("package `{}` resolves to multiple roots", manifest.name),
                "use one canonical dependency root for each package name",
            )]);
        }
        if manifest.name != dependency.name {
            return Err(vec![package_diagnostic(
                format!(
                    "dependency key `{}` resolves to package `{}`",
                    dependency.name, manifest.name
                ),
                "make the dependency key equal the dependency package name",
            )]);
        }
        if !matches!(manifest.language.as_str(), "0.2" | "0.3" | "0.4" | "0.5")
            || manifest.kind != PackageKind::Sdk
        {
            return Err(vec![package_diagnostic(
                format!(
                    "dependency `{}` is not a supported source SDK package",
                    manifest.name
                ),
                "set `language = \"0.5\"` and `kind = \"sdk\"` in the dependency",
            )]);
        }
        visiting.push(canonical.clone());
        resolve_dependencies(&manifest, packages, visited, visiting, sources)?;
        visiting.pop();
        let (_, artifact) = check_namespaced(&manifest.root, &manifest.name, sources)?;
        merge_packages(packages, artifact.sdk_packages)?;
        let source = artifact.source_interface.expect("SDK source interface");
        visited.insert(canonical, manifest.name.clone());
        packages.push(ResolvedSdkPackage {
            manifest,
            native_imports: Vec::new(),
            source: Some(source),
        });
    }
    Ok(())
}

fn merge_packages(
    packages: &mut Vec<ResolvedSdkPackage>,
    incoming: Vec<ResolvedSdkPackage>,
) -> Result<(), Vec<Diagnostic>> {
    for package in incoming {
        if let Some(previous) = packages
            .iter()
            .find(|p| p.manifest.name == package.manifest.name)
        {
            if previous.manifest.root != package.manifest.root {
                return Err(vec![package_diagnostic(
                    format!(
                        "package `{}` resolves to multiple roots",
                        package.manifest.name
                    ),
                    "use one canonical package identity",
                )]);
            }
        } else {
            packages.push(package);
        }
    }
    Ok(())
}

fn extend_standard_symbols(
    symbols: &mut HashMap<String, HashMap<String, String>>,
    packages: &[ResolvedSdkPackage],
) {
    for package in packages {
        if crate::standard_library::SOURCE_PACKAGES.contains(&package.manifest.name.as_str())
            && let Some(source) = &package.source
        {
            symbols.extend(source.modules.clone());
        }
    }
}

fn resolve_standard_imports(
    modules: &[ParsedModule],
    language: &str,
    author_package: Option<&str>,
    sources: &mut SourceSnapshot<'_>,
) -> Result<Vec<ResolvedSdkPackage>, Vec<Diagnostic>> {
    let mut selected = BTreeMap::new();
    for module in modules {
        for import in &module.syntax.imports {
            let name = import.path.join(".");
            if import.file.is_none()
                && crate::standard_library::SOURCE_PACKAGES.contains(&name.as_str())
                // Explicit SDK doc/unit context uses its checked public API.
                // Consumers still resolve this reserved name through the registry.
                && author_package != Some(name.as_str())
            {
                selected.entry(name).or_insert(import.range);
            }
        }
    }
    let mut packages = Vec::new();
    for (name, range) in selected {
        if language != "0.5" {
            return Err(vec![Diagnostic::error(
                DiagnosticCode::InvalidModulePath,
                range,
                format!("standard package `{name}` requires language 0.5 (requested {language})"),
                "migrate the package language to 0.5",
            )]);
        }
        let root = crate::standard_library::package_root(&name).map_err(|e| {
            vec![Diagnostic::error(
                DiagnosticCode::PackageIo,
                range,
                e,
                "use the matching toolchain bundle and a writable RINZU_CACHE_DIR",
            )]
        })?;
        let registry = root
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("registry.json");
        sources.read_config(&registry).map_err(|e| {
            vec![package_diagnostic(
                e.to_string(),
                "restore the standard-library registry",
            )]
        })?;
        let (manifest, artifact) = check_namespaced(&root, &name, sources)?;
        merge_packages(&mut packages, artifact.sdk_packages)?;
        merge_packages(
            &mut packages,
            vec![ResolvedSdkPackage {
                manifest,
                native_imports: Vec::new(),
                source: artifact.source_interface,
            }],
        )?;
    }
    Ok(packages)
}

fn package_diagnostic(message: impl Into<String>, suggestion: impl Into<String>) -> Diagnostic {
    Diagnostic::error(
        DiagnosticCode::PackageIo,
        SourceRange::empty(FileId::MANIFEST, 0),
        message,
        suggestion,
    )
}

struct ParsedModule {
    name: String,
    source_path: String,
    canonical_path: PathBuf,
    bindings: BTreeMap<String, String>,
    requested_paths: BTreeMap<u32, PathBuf>,
    syntax: Module,
}

struct Loader<'a, 'b> {
    sources: &'a mut SourceSnapshot<'b>,
    source_inputs: BTreeMap<String, Vec<u8>>,
    source_root: PathBuf,
    canonical_root: Option<PathBuf>,
    entry_path: PathBuf,
    entry_name: String,
    modules: Vec<ParsedModule>,
    loaded: HashSet<String>,
    identities: BTreeMap<PathBuf, String>,
    requested: BTreeMap<String, String>,
    visiting: Vec<(PathBuf, Option<SourceRange>)>,
    external_packages: HashSet<String>,
}

impl<'a, 'b> Loader<'a, 'b> {
    fn new(
        manifest: &Manifest,
        entry_name: &str,
        external_packages: HashSet<String>,
        sources: &'a mut SourceSnapshot<'b>,
    ) -> Self {
        let source_root = manifest.root.join("src");
        let canonical_root = source_root.canonicalize().ok();
        let entry_path = sources
            .canonical_path(&manifest.root.join(&manifest.entry))
            .unwrap_or_else(|_| manifest.root.join(&manifest.entry));
        Self {
            sources,
            source_inputs: BTreeMap::new(),
            source_root: canonical_root.clone().unwrap_or(source_root),
            canonical_root,
            entry_path,
            entry_name: entry_name.into(),
            modules: Vec::new(),
            loaded: HashSet::new(),
            identities: BTreeMap::new(),
            requested: BTreeMap::new(),
            visiting: Vec::new(),
            external_packages,
        }
    }

    fn file(
        path: &Path,
        external_packages: HashSet<String>,
        sources: &'a mut SourceSnapshot<'b>,
    ) -> Self {
        Self {
            sources,
            source_inputs: BTreeMap::new(),
            source_root: path.parent().unwrap().to_owned(),
            canonical_root: None,
            entry_path: path.to_owned(),
            entry_name: "entry".into(),
            modules: Vec::new(),
            loaded: HashSet::new(),
            identities: BTreeMap::new(),
            requested: BTreeMap::new(),
            visiting: Vec::new(),
            external_packages,
        }
    }

    fn load(
        &mut self,
        name: &str,
        path: PathBuf,
        import_range: Option<SourceRange>,
    ) -> Result<String, Vec<Diagnostic>> {
        validate_module_name(name, import_range)?;
        let key = self.load_path(path, import_range)?;
        self.requested.insert(name.to_owned(), key.clone());
        self.loaded.insert(name.to_owned());
        Ok(key)
    }

    fn key(&self, canonical: &Path) -> String {
        if canonical == self.entry_path {
            return self.entry_name.clone();
        }
        if let Ok(relative) = canonical.strip_prefix(&self.source_root) {
            let name = relative
                .with_extension("")
                .iter()
                .map(|part| part.to_string_lossy())
                .collect::<Vec<_>>()
                .join(".");
            if validate_module_name(&name, None).is_ok()
                && name != self.entry_name
                && !self.external_packages.contains(&name)
                && !self.identities.values().any(|existing| existing == &name)
            {
                return name;
            }
        }
        let base = format!(
            "__file_{:016x}",
            crate::fnv1a64(canonical.as_os_str().as_encoded_bytes())
        );
        let mut key = base.clone();
        let mut suffix = 0;
        while self.identities.values().any(|existing| existing == &key) {
            suffix += 1;
            key = format!("{base}_{suffix}");
        }
        key
    }

    fn error(
        &self,
        code: DiagnosticCode,
        range: Option<SourceRange>,
        path: &Path,
        message: String,
    ) -> Vec<Diagnostic> {
        let mut chain: Vec<_> = self
            .visiting
            .iter()
            .map(|(path, _)| path.display().to_string())
            .collect();
        chain.push(path.display().to_string());
        let mut error = Diagnostic::error(
            code,
            range.unwrap_or_else(|| SourceRange::empty(FileId::MANIFEST, 0)),
            format!("{message}; import chain: {}", chain.join(" -> ")),
            "correct the import path and keep the module graph acyclic and readable",
        );
        for (_, edge) in &self.visiting {
            if let Some(edge) = edge {
                error = error.with_secondary(*edge, "imported from here");
            }
        }
        vec![error]
    }

    fn source_failure(
        &self,
        mut errors: Vec<Diagnostic>,
        path: &Path,
        range: Option<SourceRange>,
    ) -> Vec<Diagnostic> {
        let mut paths: Vec<_> = self
            .visiting
            .iter()
            .map(|(path, _)| path.display().to_string())
            .collect();
        paths.push(path.display().to_string());
        for error in &mut errors {
            error
                .message
                .push_str(&format!("; import chain: {}", paths.join(" -> ")));
            for edge in self
                .visiting
                .iter()
                .filter_map(|(_, range)| *range)
                .chain(range)
            {
                if edge != error.primary
                    && !error
                        .secondary
                        .iter()
                        .any(|(previous, _)| previous == &edge)
                {
                    error.secondary.push((edge, "imported from here".into()));
                }
            }
        }
        errors
    }

    fn load_path(
        &mut self,
        path: PathBuf,
        range: Option<SourceRange>,
    ) -> Result<String, Vec<Diagnostic>> {
        let canonical = self.sources.canonical_path(&path).map_err(|error| {
            self.error(
                DiagnosticCode::PackageIo,
                range,
                &path,
                format!("cannot read module at `{}`: {error}", path.display()),
            )
        })?;
        if let Some(root) = &self.canonical_root
            && !canonical.starts_with(root)
        {
            return Err(self.error(
                DiagnosticCode::InvalidModulePath,
                range,
                &canonical,
                format!("module resolves outside `{}`", root.display()),
            ));
        }
        if std::fs::metadata(&canonical).is_ok_and(|metadata| !metadata.is_file()) {
            return Err(self.error(
                DiagnosticCode::InvalidModulePath,
                range,
                &canonical,
                "import target must be a regular source file".into(),
            ));
        }
        if self.visiting.iter().any(|(path, _)| path == &canonical) {
            return Err(self.error(
                DiagnosticCode::ImportCycle,
                range,
                &canonical,
                "import cycle".into(),
            ));
        }
        if let Some(key) = self.identities.get(&canonical) {
            return Ok(key.clone());
        }
        let key = self.key(&canonical);
        self.identities.insert(canonical.clone(), key.clone());
        let bytes = self.sources.read_bytes(&canonical).map_err(|error| {
            self.error(
                DiagnosticCode::PackageIo,
                range,
                &canonical,
                format!("cannot read module at `{}`: {error}", canonical.display()),
            )
        })?;
        let source = self
            .sources
            .source(&canonical, bytes.clone())
            .map_err(|errors| self.source_failure(errors, &canonical, range))?;
        let syntax = self
            .sources
            .parse(&source)
            .map_err(|errors| self.source_failure(errors, &canonical, range))?;
        validate_import_aliases(&syntax)
            .map_err(|errors| self.source_failure(errors, &canonical, range))?;
        self.visiting.push((canonical.clone(), range));
        let mut bindings = BTreeMap::new();
        let mut requested_paths = BTreeMap::new();
        for import in &syntax.imports {
            let qualifier = import.path.join(".");
            if let Some(path) = self.requested_path(import, &canonical) {
                requested_paths.insert(import.range.start_byte, path);
            }
            let target = self.resolve_import(import, &canonical)?;
            if let Some(previous) = bindings.insert(qualifier.clone(), target.clone())
                && previous != target
            {
                let earlier = syntax
                    .imports
                    .iter()
                    .find(|other| other.range != import.range && other.path == import.path)
                    .unwrap();
                return Err(vec![
                    Diagnostic::error(
                        DiagnosticCode::InvalidModulePath,
                        import
                            .file
                            .as_ref()
                            .map_or(import.range, |file| file.alias_range),
                        format!("conflicting import alias `{qualifier}`"),
                        "use a different alias for each target",
                    )
                    .with_secondary(earlier.range, "previous binding"),
                ]);
            }
        }
        self.visiting.pop();
        self.loaded.insert(key.clone());
        let source_path = if self.canonical_root.is_some() {
            format!(
                "src/{}",
                canonical.strip_prefix(&self.source_root).unwrap().display()
            )
        } else {
            canonical.display().to_string()
        };
        self.source_inputs.insert(source_path.clone(), bytes);
        self.modules.push(ParsedModule {
            name: key.clone(),
            source_path,
            canonical_path: canonical,
            bindings,
            requested_paths,
            syntax,
        });
        Ok(key)
    }

    fn resolve_import(
        &mut self,
        import: &crate::ast::Import,
        importer: &Path,
    ) -> Result<String, Vec<Diagnostic>> {
        if let Some(file) = &import.file {
            return self.load_path(
                importer.parent().unwrap().join(&file.path),
                Some(file.path_range),
            );
        }
        let name = import.path.join(".");
        if self.external_packages.contains(&name) {
            return Ok(name);
        }
        if import
            .path
            .first()
            .is_some_and(|segment| segment == "rinzu")
        {
            return Err(vec![Diagnostic::error(
                DiagnosticCode::InvalidModulePath,
                import.range,
                format!(
                    "SDK package `{name}` is not a declared dependency or a registered standard package"
                ),
                "use a registered standard package or declare an additional SDK dependency",
            )]);
        }
        self.load(
            &name,
            self.source_root
                .join(import.path.iter().collect::<PathBuf>())
                .with_extension("rinzu"),
            Some(import.range),
        )
    }

    fn requested_path(&self, import: &crate::ast::Import, importer: &Path) -> Option<PathBuf> {
        if let Some(file) = &import.file {
            Some(importer.parent().unwrap().join(&file.path))
        } else if self.external_packages.contains(&import.path.join(".")) {
            None
        } else {
            Some(
                self.source_root
                    .join(import.path.iter().collect::<PathBuf>())
                    .with_extension("rinzu"),
            )
        }
    }
}

fn validate_import_aliases(syntax: &Module) -> Result<(), Vec<Diagnostic>> {
    for (index, import) in syntax.imports.iter().enumerate() {
        for earlier in &syntax.imports[..index] {
            if (import.file.is_some() || earlier.file.is_some())
                && import.path.first() == earlier.path.first()
                && !(import.file.is_some() && earlier.file.is_some())
            {
                return Err(vec![
                    Diagnostic::error(
                        DiagnosticCode::InvalidModulePath,
                        import.range,
                        "file alias conflicts with a dotted import root",
                        "choose a distinct file alias",
                    )
                    .with_secondary(earlier.range, "previous import"),
                ]);
            }
        }
        if let Some(file) = &import.file
            && let Some(declaration) = syntax.declarations.iter().find(|declaration| {
                declaration.name().split('.').next() == import.path.first().map(String::as_str)
            })
        {
            return Err(vec![
                Diagnostic::error(
                    DiagnosticCode::InvalidModulePath,
                    declaration.range(),
                    format!(
                        "declaration conflicts with import alias `{}`",
                        import.path[0]
                    ),
                    "rename the declaration or alias",
                )
                .with_secondary(file.alias_range, "import alias"),
            ]);
        }
    }
    Ok(())
}

fn module_name_from_entry(entry: &Path) -> Result<String, Vec<Diagnostic>> {
    let without_root = entry.strip_prefix("src").map_err(|_| {
        vec![Diagnostic::error(
            DiagnosticCode::InvalidModulePath,
            SourceRange::empty(FileId::MANIFEST, 0),
            "entry is not inside `src/`",
            "set `package.entry` to a `.rinzu` path under `src/`",
        )]
    })?;
    let mut segments: Vec<_> = without_root
        .parent()
        .into_iter()
        .flat_map(Path::iter)
        .map(|segment| segment.to_string_lossy().into_owned())
        .collect();
    let stem = without_root
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            vec![Diagnostic::error(
                DiagnosticCode::InvalidModulePath,
                SourceRange::empty(FileId::MANIFEST, 0),
                "entry file has no valid UTF-8 stem",
                "use a lowercase ASCII `.rinzu` entry file name",
            )]
        })?;
    segments.push(stem.to_owned());
    let name = segments.join(".");
    validate_module_name(&name, None)?;
    Ok(name)
}

fn validate_module_name(name: &str, range: Option<SourceRange>) -> Result<(), Vec<Diagnostic>> {
    let valid = !name.is_empty()
        && !name.starts_with("rinzu.")
        && name.split('.').all(|segment| {
            let mut chars = segment.chars();
            matches!(chars.next(), Some('a'..='z'))
                && chars.all(|character| {
                    character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
                })
        });
    if valid {
        Ok(())
    } else {
        Err(vec![Diagnostic::error(
            DiagnosticCode::InvalidModulePath,
            range.unwrap_or_else(|| SourceRange::empty(FileId::MANIFEST, 0)),
            format!("invalid package module path `{name}`"),
            "use lowercase ASCII segments matching `[a-z][a-z0-9_]*`; `rinzu` is reserved",
        )])
    }
}

fn scoped_module(namespace: &str, name: &str) -> String {
    if namespace.is_empty() {
        name.to_owned()
    } else {
        format!("{namespace}::{name}")
    }
}

fn resolved_graph(
    modules: &[ParsedModule],
    entry: &str,
    namespace: &str,
    aliases: &HashMap<(String, String), String>,
) -> Vec<ResolvedSourceModule> {
    modules
        .iter()
        .map(|module| {
            let symbols = |owner: &str| {
                aliases
                    .iter()
                    .filter(|((target, _), _)| target == owner)
                    .map(|((_, name), identity)| (name.clone(), identity.clone()))
                    .collect::<BTreeMap<_, _>>()
            };
            ResolvedSourceModule {
                path: module.canonical_path.clone(),
                key: scoped_module(namespace, &module.name),
                file_id: module.syntax.file_id,
                is_entry: module.name == entry,
                exports: symbols(&module.name),
                imports: module
                    .syntax
                    .imports
                    .iter()
                    .map(|import| {
                        let qualifier = import.path.join(".");
                        let target = &module.bindings[&qualifier];
                        let local = modules.iter().find(|candidate| &candidate.name == target);
                        let requested_path = module
                            .requested_paths
                            .get(&import.range.start_byte)
                            .cloned();
                        ResolvedSourceImport {
                            qualifier,
                            target: if local.is_some() {
                                scoped_module(namespace, target)
                            } else {
                                target.clone()
                            },
                            target_path: local.map(|module| module.canonical_path.clone()),
                            requested_path,
                            kind: if import.file.is_some() {
                                "file"
                            } else if crate::standard_library::builtin(target) {
                                "builtin"
                            } else if local.is_some() {
                                "dotted"
                            } else {
                                "sdk"
                            },
                            range: import.range,
                            path_range: import.file.as_ref().map(|file| file.path_range),
                            alias_range: import.file.as_ref().map(|file| file.alias_range),
                            public_symbols: symbols(target),
                        }
                    })
                    .collect(),
            }
        })
        .collect()
}

fn import_context(mut errors: Vec<Diagnostic>, graph: &[ResolvedSourceModule]) -> Vec<Diagnostic> {
    fn chain<'a>(
        node: &'a ResolvedSourceModule,
        file: FileId,
        graph: &'a [ResolvedSourceModule],
        visited: &mut HashSet<String>,
        edges: &mut Vec<&'a ResolvedSourceImport>,
    ) -> bool {
        if node.file_id == file {
            return true;
        }
        if !visited.insert(node.key.clone()) {
            return false;
        }
        for edge in &node.imports {
            if let Some(path) = &edge.target_path
                && let Some(target) = graph.iter().find(|node| &node.path == path)
            {
                edges.push(edge);
                if chain(target, file, graph, visited, edges) {
                    return true;
                }
                edges.pop();
            }
        }
        false
    }
    let Some(entry) = graph.iter().find(|node| node.is_entry) else {
        return errors;
    };
    for error in &mut errors {
        let mut edges = Vec::new();
        if chain(
            entry,
            error.primary.file_id,
            graph,
            &mut HashSet::new(),
            &mut edges,
        ) && !edges.is_empty()
        {
            let mut paths = vec![entry.path.display().to_string()];
            for edge in edges {
                let range = edge.path_range.unwrap_or(edge.range);
                if !error
                    .secondary
                    .iter()
                    .any(|(previous, _)| previous == &range)
                {
                    error.secondary.push((range, "imported from here".into()));
                }
                paths.push(edge.target_path.as_ref().unwrap().display().to_string());
            }
            error
                .message
                .push_str(&format!("; import chain: {}", paths.join(" -> ")));
        }
    }
    errors
}

fn link_modules(
    entry: &str,
    modules: Vec<ParsedModule>,
    namespace: &str,
    external: &HashMap<String, HashMap<String, String>>,
) -> Result<(Module, Vec<ResolvedSourceModule>), Vec<Diagnostic>> {
    let graph = resolved_graph(&modules, entry, namespace, &HashMap::new());
    link_modules_inner(entry, modules, namespace, external)
        .map_err(|errors| import_context(errors, &graph))
}

fn link_modules_inner(
    entry: &str,
    modules: Vec<ParsedModule>,
    namespace: &str,
    external: &HashMap<String, HashMap<String, String>>,
) -> Result<(Module, Vec<ResolvedSourceModule>), Vec<Diagnostic>> {
    let mut names: HashMap<_, _> = modules
        .iter()
        .map(|module| {
            (
                module.name.clone(),
                module
                    .syntax
                    .declarations
                    .iter()
                    .map(|declaration| declaration.name().to_owned())
                    .chain(
                        module
                            .syntax
                            .native_blocks
                            .iter()
                            .flat_map(|block| &block.members)
                            .map(|member| match member {
                                NativeMember::Opaque { name, .. } => name.clone(),
                                NativeMember::Record(record) => record.name.clone(),
                                NativeMember::Function(function) => function.name.clone(),
                            }),
                    )
                    .collect::<HashSet<_>>(),
            )
        })
        .collect();
    let mut aliases: HashMap<_, _> = external
        .iter()
        .flat_map(|(module, symbols)| {
            symbols
                .iter()
                .map(move |(name, target)| ((module.clone(), name.clone()), target.clone()))
        })
        .collect();
    for (module, symbols) in external {
        if names.contains_key(module) {
            return Err(vec![package_diagnostic(
                format!("ambiguous local and dependency module `{module}`"),
                "rename the local module",
            )]);
        }
        names.insert(module.clone(), symbols.keys().cloned().collect());
    }
    let mut methods: BTreeMap<String, std::collections::BTreeSet<String>> = BTreeMap::new();
    let mut all_methods = HashSet::new();
    for module in &modules {
        let context = RewriteContext {
            module: &module.name,
            entry,
            imports: &module.bindings,
            names: &names,
            aliases: &aliases,
            namespace,
        };
        for (owner, members) in crate::methods::public_methods(&module.syntax) {
            methods
                .entry(context.internal(&module.name, &owner))
                .or_default()
                .extend(
                    members
                        .iter()
                        .map(|name| context.internal(&module.name, name)),
                );
        }
        for declaration in &module.syntax.declarations {
            if let Declaration::Function(function) = declaration
                && crate::methods::parts(&function.name).is_some()
            {
                all_methods.insert(context.internal(&module.name, &function.name));
            }
        }
    }
    for target in external.values().flat_map(|symbols| symbols.values()) {
        if let Some((owner, _)) = crate::methods::parts(target) {
            methods
                .entry(owner.to_owned())
                .or_default()
                .insert(target.clone());
            all_methods.insert(target.clone());
        }
    }
    let mut implicit_exports: HashMap<String, BTreeMap<String, (String, SourceRange)>> =
        HashMap::new();
    let mut pending: Vec<_> = modules
        .iter()
        .flat_map(|module| {
            module
                .syntax
                .exports
                .iter()
                .map(move |export| (module, export))
        })
        .collect();
    while !pending.is_empty() {
        let mut next = Vec::new();
        let before = pending.len();
        for (module, export) in pending {
            let imports = &module.bindings;
            let context = RewriteContext {
                module: &module.name,
                entry,
                imports,
                names: &names,
                aliases: &aliases,
                namespace,
            };
            let mut path = export.path.clone();
            context.rewrite_symbol_path(&mut path);
            if !export.implicit && all_methods.contains(&path.join(".")) {
                return Err(vec![crate::methods::method_export_error(
                    &export.path.join("."),
                    export.range,
                )]);
            }
            let target = path
                .first()
                .filter(|_| path.len() == 1)
                .filter(|target| {
                    aliases.values().any(|value| value == *target)
                        || names.iter().any(|(owner, symbols)| {
                            symbols
                                .iter()
                                .any(|symbol| context.internal(owner, symbol) == **target)
                        })
                })
                .cloned();
            let Some(target) = target else {
                next.push((module, export));
                continue;
            };
            let name = crate::methods::export_name(&export.path, &target);
            let local = context.internal(&module.name, &name);
            let members: Vec<_> = methods
                .get(&target)
                .into_iter()
                .flatten()
                .map(|method| {
                    let (_, member) = crate::methods::parts(method).expect("method identity");
                    let member_name = format!("{name}.{member}");
                    let local = context.internal(&module.name, &member_name);
                    (member_name, method.clone(), local)
                })
                .collect();
            register_export_alias(
                &module.name,
                name,
                target,
                local,
                export.range,
                &mut names,
                &mut aliases,
            )?;
            for (name, target, local) in members {
                register_export_alias(
                    &module.name,
                    name.clone(),
                    target.clone(),
                    local,
                    export.range,
                    &mut names,
                    &mut aliases,
                )?;
                implicit_exports
                    .entry(module.name.clone())
                    .or_default()
                    .insert(name, (target, export.range));
            }
        }
        if next.len() == before {
            return Err(next
                .into_iter()
                .map(|(_, export)| {
                    Diagnostic::error(
                        DiagnosticCode::InvalidModulePath,
                        export.range,
                        format!("unresolved or cyclic export `{}`", export.path.join(".")),
                        "export an existing local or imported declaration",
                    )
                })
                .collect());
        }
        pending = next;
    }
    let (entry_file_id, entry_range) = modules
        .iter()
        .find(|module| module.name == entry)
        .map(|module| (module.syntax.file_id, module.syntax.range))
        .ok_or_else(|| {
            vec![Diagnostic::error(
                DiagnosticCode::PackageIo,
                SourceRange::empty(FileId::MANIFEST, 0),
                "entry module was not loaded",
                "check `package.entry` and its source file",
            )]
        })?;
    let module_graph = resolved_graph(&modules, entry, namespace, &aliases);
    let mut declarations = Vec::new();
    let mut native_blocks = Vec::new();
    let mut exports = Vec::new();
    for module in modules {
        let imports = &module.bindings;
        let context = RewriteContext {
            module: &module.name,
            entry,
            imports,
            names: &names,
            aliases: &aliases,
            namespace,
        };
        for mut block in module.syntax.native_blocks {
            for member in &mut block.members {
                match member {
                    NativeMember::Opaque { name, .. } => {
                        *name = context.internal(context.module, name)
                    }
                    NativeMember::Record(record) => {
                        for field in &mut record.fields {
                            rewrite_type(&mut field.ty, &context, &HashSet::new());
                        }
                        record.name = context.internal(context.module, &record.name);
                    }
                    NativeMember::Function(function) => {
                        let locals = function.parameters.iter().map(|p| p.name.clone()).collect();
                        for parameter in &mut function.parameters {
                            rewrite_type(&mut parameter.ty, &context, &locals);
                        }
                        rewrite_type(&mut function.result, &context, &locals);
                        function.name = context.internal(context.module, &function.name);
                    }
                }
            }
            native_blocks.push(block);
        }
        for mut export in module.syntax.exports {
            context.rewrite_symbol_path(&mut export.path);
            exports.push(export);
        }
        if let Some(members) = implicit_exports.remove(&module.name) {
            exports.extend(members.into_values().map(|(target, range)| Export {
                path: vec![target],
                implicit: true,
                range,
            }));
        }
        for mut declaration in module.syntax.declarations {
            rewrite_declaration(&mut declaration, &context);
            declarations.push(declaration);
        }
    }
    Ok((
        Module {
            file_id: entry_file_id,
            imports: Vec::new(),
            exports,
            declarations,
            native_blocks,
            range: entry_range,
        },
        module_graph,
    ))
}

fn register_export_alias(
    module: &str,
    name: String,
    target: String,
    local: String,
    range: SourceRange,
    names: &mut HashMap<String, HashSet<String>>,
    aliases: &mut HashMap<(String, String), String>,
) -> Result<(), Vec<Diagnostic>> {
    let key = (module.to_owned(), name.clone());
    if let Some(previous) = aliases.get(&key) {
        if previous != &target {
            return Err(vec![Diagnostic::error(
                DiagnosticCode::InvalidModulePath,
                range,
                format!("conflicting exports named `{name}`"),
                "export each name from one declaration",
            )]);
        }
    } else if names
        .get(module)
        .is_some_and(|symbols| symbols.contains(&name))
        && target != local
    {
        return Err(vec![Diagnostic::error(
            DiagnosticCode::InvalidModulePath,
            range,
            format!("export `{name}` conflicts with a local declaration"),
            "rename the local declaration",
        )]);
    }
    aliases.insert(key, target);
    names
        .get_mut(module)
        .expect("exporting module")
        .insert(name);
    Ok(())
}

struct RewriteContext<'a> {
    module: &'a str,
    entry: &'a str,
    imports: &'a BTreeMap<String, String>,
    names: &'a HashMap<String, HashSet<String>>,
    aliases: &'a HashMap<(String, String), String>,
    namespace: &'a str,
}

impl RewriteContext<'_> {
    fn internal(&self, module: &str, symbol: &str) -> String {
        let local = if module == self.entry {
            symbol.to_owned()
        } else {
            format!("{module}::{symbol}")
        };
        if self.namespace.is_empty() {
            local
        } else {
            format!("{}::{local}", self.namespace)
        }
    }

    fn local_symbol(&self, symbol: &str) -> Option<String> {
        self.names
            .get(self.module)
            .is_some_and(|names| names.contains(symbol))
            .then(|| self.symbol_identity(self.module, symbol))
    }

    fn symbol_identity(&self, module: &str, symbol: &str) -> String {
        self.aliases
            .get(&(module.to_owned(), symbol.to_owned()))
            .cloned()
            .unwrap_or_else(|| self.internal(module, symbol))
    }

    fn qualified_symbol(&self, path: &[String]) -> Option<(String, usize)> {
        for (qualifier, module) in self.imports.iter().rev() {
            let module_segments: Vec<_> = qualifier.split('.').collect();
            if path.len() <= module_segments.len()
                || !path
                    .iter()
                    .zip(&module_segments)
                    .all(|(left, right)| left == right)
            {
                continue;
            }
            let symbol_index = module_segments.len();
            let qualified = path[symbol_index..].join(".");
            if self
                .names
                .get(module)
                .is_some_and(|names| names.contains(&qualified))
                && (module == self.module
                    || self
                        .aliases
                        .contains_key(&(module.clone(), qualified.clone())))
            {
                return Some((self.symbol_identity(module, &qualified), path.len()));
            }
            let symbol = &path[symbol_index];
            if self
                .names
                .get(module)
                .is_some_and(|names| names.contains(symbol))
                && (module == self.module
                    || self.aliases.contains_key(&(module.clone(), symbol.clone())))
            {
                return Some((self.symbol_identity(module, symbol), symbol_index + 1));
            }
        }
        None
    }

    fn rewrite_symbol_path(&self, path: &mut Vec<String>) {
        if let Some(internal) = self.local_symbol(&path.join(".")) {
            *path = vec![internal];
            return;
        }
        if let Some(first) = path.first().cloned()
            && let Some(internal) = self.local_symbol(&first)
        {
            path[0] = internal;
            return;
        }
        if let Some((internal, consumed)) = self.qualified_symbol(path) {
            let mut rewritten = vec![internal];
            rewritten.extend(path.drain(consumed..));
            *path = rewritten;
        } else if path.len() == 1 && crate::core::FUNCTIONS.contains(&path[0].as_str()) {
            path[0] = format!("rinzu.core::{}", path[0]);
        }
    }
}

fn rewrite_declaration(declaration: &mut Declaration, context: &RewriteContext<'_>) {
    let type_parameters = match declaration {
        Declaration::Interface(i) => &mut i.type_parameters,
        Declaration::Function(f) => &mut f.type_parameters,
        Declaration::Record(r) => &mut r.type_parameters,
        Declaration::Enum(e) => &mut e.type_parameters,
        Declaration::Error(e) => &mut e.type_parameters,
        Declaration::TypeAlias(a) => &mut a.type_parameters,
        Declaration::Constant(_) => &mut Vec::new(),
    };
    let locals = type_parameters.iter().map(|p| p.name.clone()).collect();
    for p in type_parameters {
        for ty in &mut p.interfaces {
            rewrite_type(ty, context, &locals);
        }
    }
    match declaration {
        Declaration::Interface(interface) => {
            let mut locals: HashSet<_> = interface
                .type_parameters
                .iter()
                .map(|p| p.name.clone())
                .collect();
            locals.insert("Self".into());
            for method in &mut interface.methods {
                for p in &mut method.parameters {
                    rewrite_type(&mut p.ty, context, &locals);
                }
                rewrite_type(&mut method.result, context, &locals);
                if let Some(contract) = &mut method.error_contract {
                    for ty in &mut contract.types {
                        rewrite_type(ty, context, &locals);
                    }
                }
            }
            interface.name = context.internal(context.module, &interface.name);
        }
        Declaration::Error(error) => {
            let parameters = error
                .type_parameters
                .iter()
                .map(|parameter| parameter.name.clone())
                .collect();
            if let Some(payload) = &mut error.payload {
                rewrite_type(&mut payload.ty, context, &parameters);
            }
            error.name = context.internal(context.module, &error.name);
        }
        Declaration::TypeAlias(alias) => {
            let locals = alias
                .type_parameters
                .iter()
                .map(|p| p.name.clone())
                .collect();
            rewrite_type(&mut alias.ty, context, &locals);
            alias.name = context.internal(context.module, &alias.name);
        }
        Declaration::Function(function) => {
            if let Some(owner) = &mut function.lexical_owner {
                *owner = context.internal(context.module, owner);
            }
            let locals: HashSet<_> = function
                .parameters
                .iter()
                .map(|p| p.name.clone())
                .chain(function.type_parameters.iter().map(|p| p.name.clone()))
                .collect();
            for parameter in &mut function.parameters {
                rewrite_type(&mut parameter.ty, context, &locals);
            }
            rewrite_type(&mut function.result, context, &locals);
            if let Some(contract) = &mut function.error_contract {
                for ty in &mut contract.types {
                    rewrite_type(ty, context, &locals);
                }
            }
            rewrite_block(&mut function.body, context, &locals);
            if let Some(fallback) = &mut function.fallback {
                rewrite_expression(fallback, context, &HashSet::new());
            }
            function.name = context.internal(context.module, &function.name);
        }
        Declaration::Constant(constant) => {
            if let Some(annotation) = &mut constant.annotation {
                rewrite_type(annotation, context, &HashSet::new());
            }
            rewrite_expression(&mut constant.initializer, context, &HashSet::new());
            constant.name = context.internal(context.module, &constant.name);
        }
        Declaration::Record(record) => {
            let parameters = record
                .type_parameters
                .iter()
                .map(|p| p.name.clone())
                .collect();
            for field in &mut record.fields {
                rewrite_type(&mut field.ty, context, &parameters);
            }
            for ty in &mut record.interfaces {
                rewrite_type(ty, context, &parameters);
            }
            record.name = context.internal(context.module, &record.name);
        }
        Declaration::Enum(enum_decl) => {
            let parameters = enum_decl
                .type_parameters
                .iter()
                .map(|p| p.name.clone())
                .collect();
            for variant in &mut enum_decl.variants {
                for field in &mut variant.fields {
                    rewrite_type(&mut field.ty, context, &parameters);
                }
            }
            for ty in &mut enum_decl.interfaces {
                rewrite_type(ty, context, &parameters);
            }
            enum_decl.name = context.internal(context.module, &enum_decl.name);
        }
    }
}
fn rewrite_type(ty: &mut TypeRef, context: &RewriteContext<'_>, locals: &HashSet<String>) {
    match &mut ty.kind {
        TypeRefKind::Record(fields) => {
            for field in fields {
                rewrite_type(&mut field.ty, context, locals);
            }
        }
        TypeRefKind::Named(path) => {
            if !path.first().is_some_and(|name| locals.contains(name)) {
                context.rewrite_symbol_path(path);
            }
        }
        TypeRefKind::Generic { path, arguments } => {
            context.rewrite_symbol_path(path);
            for argument in arguments {
                rewrite_type(argument, context, locals);
            }
        }
        TypeRefKind::Bytes(length) => rewrite_expression(length, context, locals),
        TypeRefKind::ConcreteBytes(_) => {}
        TypeRefKind::Function {
            parameters, result, ..
        } => {
            for parameter in parameters {
                rewrite_type(parameter, context, locals);
            }
            rewrite_type(result, context, locals);
        }
        TypeRefKind::List(element) | TypeRefKind::Future(element) => {
            rewrite_type(element, context, locals)
        }
        TypeRefKind::Map(key, value) => {
            rewrite_type(key, context, locals);
            rewrite_type(value, context, locals);
        }
    }
}
fn rewrite_block(block: &mut Block, context: &RewriteContext<'_>, outer: &HashSet<String>) {
    let mut locals = outer.clone();
    for statement in &mut block.statements {
        rewrite_statement(statement, context, &mut locals);
    }
}
fn rewrite_statement(
    statement: &mut Statement,
    context: &RewriteContext<'_>,
    locals: &mut HashSet<String>,
) {
    match &mut statement.kind {
        StatementKind::Block(block) => rewrite_block(block, context, locals),
        StatementKind::Local {
            name,
            annotation,
            initializer,
            ..
        } => {
            if let Some(annotation) = annotation {
                rewrite_type(annotation, context, locals);
            }
            rewrite_expression(initializer, context, locals);
            locals.insert(name.clone());
        }
        StatementKind::FieldAssign { base, value, .. } => {
            rewrite_expression(base, context, locals);
            rewrite_expression(value, context, locals);
        }
        StatementKind::Assign { value, .. }
        | StatementKind::Expression(value)
        | StatementKind::Throw(value) => rewrite_expression(value, context, locals),
        StatementKind::Return(value) => {
            if let Some(value) = value {
                rewrite_expression(value, context, locals);
            }
        }
        StatementKind::If {
            condition,
            then_block,
            else_branch,
        } => {
            rewrite_expression(condition, context, locals);
            rewrite_block(then_block, context, locals);
            if let Some(branch) = else_branch {
                rewrite_statement(branch, context, &mut locals.clone());
            }
        }
        StatementKind::While { condition, body } => {
            rewrite_expression(condition, context, locals);
            rewrite_block(body, context, locals);
        }
        StatementKind::For {
            binding,
            iterable,
            body,
            ..
        } => {
            rewrite_expression(iterable, context, locals);
            let mut bindings = locals.clone();
            bindings.insert(binding.clone());
            rewrite_block(body, context, &bindings);
        }
        StatementKind::Switch { value, cases } => {
            rewrite_expression(value, context, locals);
            for case in cases {
                context.rewrite_symbol_path(&mut case.path);
                let mut bindings = locals.clone();
                bindings.extend(case.bindings.iter().cloned());
                rewrite_block(&mut case.body, context, &bindings);
            }
        }
        StatementKind::TryErrors {
            body,
            handlers,
            cleanup,
            ..
        } => {
            rewrite_block(body, context, locals);
            for handler in handlers {
                if let crate::ast::CatchPattern::List { elements, .. } = &mut handler.pattern {
                    for element in elements {
                        rewrite_type(&mut element.ty, context, locals);
                    }
                }
                let mut bindings = locals.clone();
                bindings.extend(
                    handler
                        .pattern
                        .bindings()
                        .map(|binding| binding.name.clone()),
                );
                rewrite_block(&mut handler.body, context, &bindings);
            }
            if let Some(cleanup) = cleanup {
                rewrite_block(cleanup, context, locals);
            }
        }
        StatementKind::TryAll {
            body,
            handler,
            cleanup,
        } => {
            rewrite_block(body, context, locals);
            if let Some(h) = handler {
                let mut names = locals.clone();
                names.insert(h.binding.clone());
                rewrite_block(&mut h.body, context, &names);
            }
            if let Some(c) = cleanup {
                rewrite_block(c, context, locals);
            }
        }
        StatementKind::TryCatch {
            body,
            binding,
            handler,
            cleanup,
            ..
        } => {
            rewrite_block(body, context, locals);
            let mut bindings = locals.clone();
            bindings.insert(binding.clone());
            rewrite_block(handler, context, &bindings);
            if let Some(cleanup) = cleanup {
                rewrite_block(cleanup, context, locals);
            }
        }
        StatementKind::TryFinally { body, cleanup }
        | StatementKind::TaskScope { body, cleanup } => {
            rewrite_block(body, context, locals);
            rewrite_block(cleanup, context, locals);
        }
        StatementKind::Break | StatementKind::Continue => {}
    }
}
fn rewrite_value_path(
    path: &mut Vec<String>,
    context: &RewriteContext<'_>,
    locals: &HashSet<String>,
) {
    if !path.first().is_some_and(|first| locals.contains(first)) {
        context.rewrite_symbol_path(path);
    }
}
fn rewrite_expression(
    expression: &mut Expression,
    context: &RewriteContext<'_>,
    locals: &HashSet<String>,
) {
    match &mut expression.kind {
        ExpressionKind::Function(f) => {
            let mut local = locals.clone();
            for p in &f.parameters {
                local.insert(p.name.clone());
            }
            for p in &mut f.parameters {
                rewrite_type(&mut p.ty, context, &local);
            }
            rewrite_type(&mut f.result, context, &local);
            if let Some(contract) = &mut f.error_contract {
                for ty in &mut contract.types {
                    rewrite_type(ty, context, &local);
                }
            }
            rewrite_block(&mut f.body, context, &local);
            if let Some(e) = &mut f.fallback {
                rewrite_expression(e, context, &local);
            }
        }
        ExpressionKind::Record(fields) => {
            for field in fields {
                if let Some(t) = &mut field.annotation {
                    rewrite_type(t, context, locals);
                }
                rewrite_expression(&mut field.value, context, locals);
            }
        }
        ExpressionKind::GenericCall {
            target,
            type_arguments,
            member_type_arguments,
            arguments,
            ..
        } => {
            rewrite_value_path(target, context, locals);
            for argument in type_arguments.iter_mut().chain(member_type_arguments) {
                rewrite_type(argument, context, locals);
            }
            rewrite_arguments(arguments, context, locals);
        }
        ExpressionKind::GenericReference {
            target,
            type_arguments,
            member_type_arguments,
            ..
        } => {
            rewrite_value_path(target, context, locals);
            for argument in type_arguments.iter_mut().chain(member_type_arguments) {
                rewrite_type(argument, context, locals);
            }
        }
        ExpressionKind::Bind {
            target,
            type_arguments,
            arguments,
        } => {
            rewrite_value_path(target, context, locals);
            for argument in type_arguments {
                rewrite_type(argument, context, locals);
            }
            rewrite_arguments(arguments, context, locals);
        }
        ExpressionKind::Name(path) => rewrite_value_path(path, context, locals),
        ExpressionKind::Call { target, arguments }
        | ExpressionKind::Spawn { target, arguments } => {
            rewrite_value_path(target, context, locals);
            rewrite_arguments(arguments, context, locals);
        }
        ExpressionKind::List(values) | ExpressionKind::FormatString(values) => {
            for value in values {
                rewrite_expression(value, context, locals);
            }
        }
        ExpressionKind::RecordUpdate { base, fields } => {
            rewrite_expression(base, context, locals);
            for (_, value, _) in fields {
                rewrite_expression(value, context, locals);
            }
        }
        ExpressionKind::Map(entries) => {
            for (key, value) in entries {
                rewrite_expression(key, context, locals);
                rewrite_expression(value, context, locals);
            }
        }
        ExpressionKind::Unary { operand, .. } | ExpressionKind::Await(operand) => {
            rewrite_expression(operand, context, locals)
        }
        ExpressionKind::Binary { left, right, .. } => {
            rewrite_expression(left, context, locals);
            rewrite_expression(right, context, locals);
        }
        ExpressionKind::Is { value, ty } => {
            rewrite_expression(value, context, locals);
            rewrite_type(ty, context, locals);
        }
        ExpressionKind::Literal(_) | ExpressionKind::Unit => {}
    }
}
fn rewrite_arguments(
    arguments: &mut Arguments,
    context: &RewriteContext<'_>,
    locals: &HashSet<String>,
) {
    match arguments {
        Arguments::Positional(values) => {
            for value in values {
                rewrite_expression(value, context, locals);
            }
        }
        Arguments::Named(values) => {
            for (_, value, _) in values {
                rewrite_expression(value, context, locals);
            }
        }
    }
}
