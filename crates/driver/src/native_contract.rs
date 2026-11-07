use crate::native_clang::TargetClang;
use rinzu_compiler::ast::NativeBinding;
use rinzu_compiler::{AbiScalar, SourceAbiType, SourceNativeSignature};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;

const BYTE_ARRAY_RESULT: &str =
    "struct __rinzu_byte_array_result { uint8_t *data; size_t length; };\n";

#[derive(Clone, Debug)]
pub struct RecordLayout {
    pub c_type: String,
    pub size: u64,
    pub alignment: u64,
    pub offsets: Vec<u64>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub enum CType {
    Value(String),
    Callback(Box<CFunction>),
}

impl CType {
    pub fn declaration(&self, name: &str) -> String {
        match self {
            Self::Value(spelling) => format!("{spelling} {name}"),
            Self::Callback(function) => function.named_declaration(&format!("(*{name})"), "p"),
        }
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct CFunction {
    pub result: CType,
    pub parameters: Vec<CType>,
}

impl CFunction {
    pub fn declaration(&self, name: &str) -> String {
        self.named_declaration(name, "a")
    }

    fn named_declaration(&self, name: &str, prefix: &str) -> String {
        let parameters = if self.parameters.is_empty() {
            "void".into()
        } else {
            self.parameters
                .iter()
                .enumerate()
                .map(|(index, ty)| ty.declaration(&format!("{prefix}{index}")))
                .collect::<Vec<_>>()
                .join(", ")
        };
        self.result.declaration(&format!("{name}({parameters})"))
    }
}

#[derive(Debug)]
pub struct NativeContract {
    pub header: String,
    pub prototypes: BTreeMap<String, String>,
    pub records: BTreeMap<String, RecordLayout>,
    pub observed_types: BTreeMap<String, String>,
    pub functions: BTreeMap<String, CFunction>,
    pub fingerprint: BTreeMap<String, String>,
}

pub fn check_artifact(
    manifest: &rinzu_compiler::Manifest,
    artifact: &rinzu_compiler::FrontendArtifact,
) -> Result<(), String> {
    let _timing = rinzu_compiler::timing::span("native.contract.check");
    struct Temporary(PathBuf);
    impl Drop for Temporary {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let temporary = Temporary(std::env::temp_dir().join(format!(
            "rinzu-abi-check-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_nanos()
        )));
    let sources = artifact
        .source_interface
        .as_ref()
        .map(|source| (manifest, source))
        .into_iter()
        .chain(artifact.sdk_packages.iter().filter_map(|package| {
            package
                .source
                .as_ref()
                .map(|source| (&package.manifest, source))
        }));
    for (index, (owner, source)) in sources.enumerate() {
        let mut owner = owner.clone();
        if owner.kind == rinzu_compiler::PackageKind::Application {
            for header in &artifact.native_inputs.headers {
                if !owner.native.headers.contains(header) {
                    owner.native.headers.push(header.clone());
                }
            }
        }
        if source
            .signatures
            .iter()
            .all(|signature| matches!(signature.binding, NativeBinding::Runtime(_)))
            && source.native_records.is_empty()
            && owner.native.headers.is_empty()
        {
            continue;
        }
        crate::native_binding::validate_signatures(source)?;
        let directory = temporary.0.join(index.to_string());
        let contract = validate_package(&owner, source, &directory)?;
        let layouts: BTreeMap<_, _> = contract.records.iter().map(|(name, layout)| (name, serde_json::json!({ "cType": layout.c_type, "size": layout.size, "alignment": layout.alignment, "offsets": layout.offsets }))).collect();
        let report = serde_json::json!({ "header": contract.header, "prototypes": contract.prototypes, "records": layouts, "observedTypes": contract.observed_types, "functions": contract.functions, "fingerprint": contract.fingerprint });
        std::fs::write(
            directory.join("abi-report.json"),
            serde_json::to_vec(&report).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub fn validate_package(
    manifest: &rinzu_compiler::Manifest,
    source: &rinzu_compiler::SourceSdkInterface,
    directory: &Path,
) -> Result<NativeContract, String> {
    validate_package_with_toolchain(manifest, source, directory, &TargetClang::resolve()?)
}

pub fn validate_package_with_toolchain(
    manifest: &rinzu_compiler::Manifest,
    source: &rinzu_compiler::SourceSdkInterface,
    directory: &Path,
    toolchain: &TargetClang,
) -> Result<NativeContract, String> {
    let _timing = rinzu_compiler::timing::span("native.contract.validate").detail(&manifest.name);
    // Match SDK compilation's selected toolchain include path. Public native
    // result headers must be visible during prototype/layout validation too.
    let mut flags = vec![
        "-I".to_owned(),
        rinzu_compiler::standard_library::resource_root()?
            .join("include")
            .to_string_lossy()
            .into_owned(),
    ];
    for path in &manifest.native.include_dirs {
        flags.extend([
            "-I".to_owned(),
            manifest.root.join(path).to_string_lossy().into_owned(),
        ]);
    }
    if manifest
        .native
        .sources
        .iter()
        .any(|path| path.extension().is_some_and(|extension| extension == "m"))
    {
        flags.extend([
            "-x".to_owned(),
            "objective-c".to_owned(),
            "-fobjc-arc".to_owned(),
        ]);
    }
    let mut config_fingerprint = BTreeMap::new();
    for tool in &manifest.native.config_tools {
        let config = crate::native_config::resolve(tool)?;
        for pair in config.flags.chunks_exact(2) {
            if matches!(pair[0].as_str(), "-I" | "-D" | "-F") {
                flags.extend_from_slice(pair);
            }
        }
        config_fingerprint.extend(config.fingerprint);
    }
    let headers: Vec<_> = manifest
        .native
        .headers
        .iter()
        .map(|path| manifest.root.join(path))
        .collect();
    let mut contract = validate(
        toolchain,
        directory,
        &headers,
        &flags,
        &source.signatures,
        &source.native_records,
    )?;
    let query = directory.join("abi-query.c");
    let mut command = toolchain.command();
    command
        .args(["-M", "-MT", "rinzu", "-std=c11"])
        .args(&flags)
        .arg(&query);
    let dependencies = crate::cli_diagnostics::tool_output(
        command.output(),
        Some(&query),
        "native include dependency query",
        "clang",
    )?;
    crate::native_fingerprint::include_inputs(
        &manifest.root,
        &toolchain.sysroot,
        directory,
        &String::from_utf8(dependencies.stdout).map_err(|error| error.to_string())?,
        &mut contract.fingerprint,
    )?;
    contract.fingerprint.extend(config_fingerprint);
    Ok(contract)
}

pub fn validate(
    toolchain: &TargetClang,
    directory: &Path,
    headers: &[PathBuf],
    flags: &[String],
    signatures: &[SourceNativeSignature],
    records: &BTreeMap<String, Vec<(String, AbiScalar)>>,
) -> Result<NativeContract, String> {
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let mut record_names = BTreeMap::new();
    let mut header = String::from(
        "/* Generated source native ABI contract. */\n#include <stdint.h>\n#include <stddef.h>\n",
    );
    header.push_str(BYTE_ARRAY_RESULT);
    for (identity, fields) in records {
        let digest = rinzu_bindgen::source_package::sha256(identity.as_bytes());
        let name = format!("__rinzu_record_{}", &digest[..16]);
        header.push_str(&format!("typedef struct {name} {{\n"));
        for (field, scalar) in fields {
            header.push_str(&format!("    {} {field};\n", scalar.c_type()));
        }
        header.push_str(&format!("}} {name};\n"));
        record_names.insert(identity.clone(), name);
    }
    let mut prototypes = BTreeMap::new();
    for signature in signatures {
        if let NativeBinding::Symbol(symbol) = &signature.binding {
            let parameters = signature
                .parameters
                .iter()
                .enumerate()
                .map(|(index, ty)| declaration(ty, &format!("a{index}"), &record_names, true))
                .collect::<Result<Vec<_>, _>>()?;
            let parameters = if parameters.is_empty() {
                "void".to_owned()
            } else {
                parameters.join(", ")
            };
            let prototype = declaration(
                &signature.result,
                &format!("{symbol}({parameters})"),
                &record_names,
                false,
            )?;
            if headers.is_empty() {
                header.push_str(&format!("{prototype};\n"));
            }
            prototypes.insert(signature.name.clone(), prototype);
        }
    }
    let mut query = String::from("#include <stdint.h>\n#include <stddef.h>\n");
    query.push_str(BYTE_ARRAY_RESULT);
    for provided in headers {
        let path = provided.to_str().ok_or("header path must be UTF-8")?;
        if path.contains(['\n', '\r', '"', '\\']) {
            return Err("header path cannot be represented in a C include".to_owned());
        }
        query.push_str(&format!("#include \"{path}\"\n"));
    }
    // The generated record definitions are independent of the supplied C tag spelling.
    for (identity, name) in &record_names {
        query.push_str(&format!("typedef struct {name} {{\n"));
        for (field, scalar) in &records[identity] {
            query.push_str(&format!("    {} {field};\n", scalar.c_type()));
        }
        query.push_str(&format!("}} {name};\n"));
    }
    if headers.is_empty() {
        for prototype in prototypes.values() {
            query.push_str(&format!("{prototype};\n"));
        }
    }
    for (index, signature) in signatures.iter().enumerate() {
        if let NativeBinding::Symbol(symbol) = &signature.binding {
            query.push_str(&format!("typedef __typeof__({symbol}) rzc_fn_{index};\n"));
        }
    }
    for (index, (identity, name)) in record_names.iter().enumerate() {
        query.push_str(&format!(
            "enum {{ rzc_size_{index} = sizeof({name}), rzc_align_{index} = _Alignof({name}) }};\n"
        ));
        for (field_index, (field, _)) in records[identity].iter().enumerate() {
            query.push_str(&format!(
                "enum {{ rzc_offset_{index}_{field_index} = offsetof({name}, {field}) }};\n"
            ));
        }
    }
    let query_path = directory.join("abi-query.c");
    std::fs::write(&query_path, &query).map_err(|error| error.to_string())?;
    let mut query_command = command(toolchain, flags, &query_path);
    query_command.args(["-Xclang", "-ast-dump=json"]);
    let output = crate::cli_diagnostics::tool_output(
        query_command.output(),
        Some(&query_path),
        "native header ABI query",
        "clang",
    )?;
    let _ast_timing = rinzu_compiler::timing::span("native.contract.ast");
    let ast: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("decode target Clang AST: {error}"))?;
    let mut nodes = HashMap::new();
    let mut constants = HashMap::new();
    collect(&ast, &mut nodes, &mut constants);
    let mut assertions = String::new();
    let mut observed_types = BTreeMap::new();
    let mut functions = BTreeMap::new();
    for (index, signature) in signatures.iter().enumerate() {
        let NativeBinding::Symbol(symbol) = &signature.binding else {
            continue;
        };
        let alias = find_named(&ast, "TypedefDecl", &format!("rzc_fn_{index}"))
            .ok_or("Clang did not produce an ABI query type")?;
        let function = find_kind(alias, "FunctionProtoType")
            .ok_or("native symbol has no complete function prototype")?;
        let reference =
            find_kind(alias, "DeclRefExpr").ok_or("native symbol is not a function declaration")?;
        let declaration = reference["referencedDecl"]["id"]
            .as_str()
            .and_then(|id| nodes.get(id))
            .copied()
            .ok_or("native symbol prototype is missing")?;
        if declaration["storageClass"] == "static"
            || declaration["inline"] == true
            || find_kind(declaration, "CompoundStmt").is_some()
        {
            return Err(format!(
                "native symbol `{symbol}` is header-only inline/static; declare an external C shim"
            ));
        }
        if find_kind(alias, "DeclRefExpr").and_then(|node| node["referencedDecl"]["name"].as_str())
            != Some(symbol)
        {
            return Err(
                "native symbol is a macro alias rather than an external prototype".to_owned(),
            );
        }
        check_function(
            function,
            &signature.parameters,
            &signature.result,
            records,
            &record_names,
            &nodes,
            &mut assertions,
        )
        .map_err(|error| format!("native symbol `{symbol}`: {error}"))?;
        functions.insert(signature.name.clone(), c_function(function)?);
        observed_types.insert(
            signature.name.clone(),
            function["type"]["qualType"]
                .as_str()
                .ok_or("unresolved function type")?
                .to_owned(),
        );
    }
    drop(_ast_timing);
    let assertion_path = directory.join("abi-assert.c");
    std::fs::write(&assertion_path, query + &assertions).map_err(|error| error.to_string())?;
    crate::cli_diagnostics::tool_output(
        command(toolchain, flags, &assertion_path).output(),
        Some(&assertion_path),
        "native prototype/layout validation",
        "clang",
    )?;
    let mut layouts = BTreeMap::new();
    for (index, (identity, name)) in record_names.iter().enumerate() {
        let read = |key: &str| {
            constants
                .get(key)
                .copied()
                .ok_or_else(|| format!("Clang omitted target layout constant `{key}`"))
        };
        layouts.insert(
            identity.clone(),
            RecordLayout {
                c_type: name.clone(),
                size: read(&format!("rzc_size_{index}"))?,
                alignment: read(&format!("rzc_align_{index}"))?,
                offsets: records[identity]
                    .iter()
                    .enumerate()
                    .map(|(field_index, _)| read(&format!("rzc_offset_{index}_{field_index}")))
                    .collect::<Result<_, _>>()?,
            },
        );
    }
    let mut fingerprint = toolchain.fingerprint.clone();
    fingerprint.insert(
        "clang.options".to_owned(),
        serde_json::to_string(&(
            vec![
                "-std=c11",
                "-Werror",
                "-Wstrict-prototypes",
                "-fsyntax-only",
                "-fno-builtin",
            ],
            flags,
        ))
        .map_err(|error| error.to_string())?,
    );
    fingerprint.insert(
        "clang.contract".to_owned(),
        rinzu_bindgen::source_package::sha256(header.as_bytes()),
    );
    fingerprint.insert(
        "clang.types".into(),
        rinzu_bindgen::source_package::sha256(
            &serde_json::to_vec(&functions).map_err(|error| error.to_string())?,
        ),
    );
    for provided in headers {
        fingerprint.insert(
            format!("header:{}", provided.display()),
            rinzu_bindgen::source_package::sha256(
                &std::fs::read(provided).map_err(|error| error.to_string())?,
            ),
        );
    }
    std::fs::write(directory.join("contract.h"), &header).map_err(|error| error.to_string())?;
    Ok(NativeContract {
        header,
        prototypes,
        records: layouts,
        observed_types,
        functions,
        fingerprint,
    })
}

fn command(toolchain: &TargetClang, flags: &[String], source: &Path) -> Command {
    let mut command = toolchain.command();
    command
        .args([
            "-std=c11",
            "-Werror",
            "-Wstrict-prototypes",
            "-fsyntax-only",
            "-fno-builtin",
        ])
        .args(flags)
        .arg(source);
    command
}

fn declaration(
    ty: &SourceAbiType,
    name: &str,
    records: &BTreeMap<String, String>,
    loan: bool,
) -> Result<String, String> {
    let base = match ty {
        SourceAbiType::Unit => "void".to_owned(),
        SourceAbiType::Scalar(scalar) => scalar.c_type().to_owned(),
        SourceAbiType::String => "const char *".to_owned(),
        SourceAbiType::Bytes(_) | SourceAbiType::ByteArray => "const uint8_t *".to_owned(),
        SourceAbiType::ByteArrayResult => "const struct __rinzu_byte_array_result *".to_owned(),
        SourceAbiType::Opaque(_) | SourceAbiType::UserData(_) => "void *".to_owned(),
        SourceAbiType::Record(identity) => format!(
            "{}{} *",
            if loan { "" } else { "const " },
            records
                .get(identity)
                .ok_or("native record mapping is missing")?
        ),
        SourceAbiType::Callback { parameters, result } => {
            let parameters = parameters
                .iter()
                .enumerate()
                .map(|(index, ty)| declaration(ty, &format!("p{index}"), records, false))
                .collect::<Result<Vec<_>, _>>()?;
            let parameters = if parameters.is_empty() {
                "void".to_owned()
            } else {
                parameters.join(", ")
            };
            return declaration(result, &format!("(*{name})({parameters})"), records, false);
        }
    };
    Ok(format!("{base} {name}"))
}

fn collect<'a>(
    node: &'a Value,
    nodes: &mut HashMap<String, &'a Value>,
    constants: &mut HashMap<String, u64>,
) {
    if let Some(id) = node["id"].as_str()
        && ((node["kind"] == "RecordDecl" && node["completeDefinition"] == true)
            || node["kind"] == "FunctionDecl")
    {
        nodes.insert(id.to_owned(), node);
    }
    if node["kind"] == "EnumConstantDecl"
        && let Some(name) = node["name"].as_str()
        && name.starts_with("rzc_")
        && let Some(value) = find_kind(node, "ConstantExpr")
            .and_then(|node| node["value"].as_str())
            .and_then(|value| value.parse().ok())
    {
        constants.insert(name.to_owned(), value);
    }
    for child in children(node) {
        collect(child, nodes, constants);
    }
}
fn children(node: &Value) -> &[Value] {
    node["inner"].as_array().map_or(&[], Vec::as_slice)
}
fn find_kind<'a>(node: &'a Value, kind: &str) -> Option<&'a Value> {
    if node["kind"] == kind {
        return Some(node);
    }
    children(node)
        .iter()
        .find_map(|child| find_kind(child, kind))
}
fn find_named<'a>(node: &'a Value, kind: &str, name: &str) -> Option<&'a Value> {
    if node["kind"] == kind && node["name"] == name {
        return Some(node);
    }
    children(node)
        .iter()
        .find_map(|child| find_named(child, kind, name))
}
fn unwrap_type(mut node: &Value) -> Result<&Value, String> {
    while matches!(
        node["kind"].as_str(),
        Some("QualType" | "ElaboratedType" | "TypedefType" | "ParenType" | "AttributedType")
    ) {
        node = children(node)
            .last()
            .ok_or("unresolved C typedef/type wrapper")?;
    }
    Ok(node)
}

fn c_function(function: &Value) -> Result<CFunction, String> {
    let types = children(function);
    Ok(CFunction {
        result: c_type(types.first().ok_or("missing C result type")?)?,
        parameters: types[1..].iter().map(c_type).collect::<Result<_, _>>()?,
    })
}

fn c_type(node: &Value) -> Result<CType, String> {
    let resolved = unwrap_type(node)?;
    if resolved["kind"] == "PointerType" {
        let pointee = unwrap_type(children(resolved).first().ok_or("missing C pointee")?)?;
        if pointee["kind"] == "FunctionProtoType" {
            return Ok(CType::Callback(Box::new(c_function(pointee)?)));
        }
    }
    Ok(CType::Value(
        node["type"]["qualType"]
            .as_str()
            .ok_or("missing C type spelling")?
            .to_owned(),
    ))
}

#[allow(clippy::too_many_arguments)]
fn check_function(
    function: &Value,
    parameters: &[SourceAbiType],
    result: &SourceAbiType,
    records: &BTreeMap<String, Vec<(String, AbiScalar)>>,
    names: &BTreeMap<String, String>,
    nodes: &HashMap<String, &Value>,
    assertions: &mut String,
) -> Result<(), String> {
    let actual = children(function);
    if function["cc"] != "cdecl"
        || function["variadic"] == true
        || actual.len() != parameters.len() + 1
    {
        return Err("native function/callback arity or C calling convention mismatch".to_owned());
    }
    check_type(&actual[0], result, records, names, nodes, assertions)?;
    for (actual, expected) in actual[1..].iter().zip(parameters) {
        check_type(actual, expected, records, names, nodes, assertions)?;
    }
    Ok(())
}

fn scalar_assertion(expression: &str, scalar: AbiScalar) -> String {
    let predicate = match scalar {
        AbiScalar::Bool => format!("__builtin_types_compatible_p(__typeof__({expression}), _Bool)"),
        AbiScalar::F32 | AbiScalar::F64 => format!("__builtin_classify_type({expression}) == 8"),
        _ => {
            let signed = scalar.integer_bounds().unwrap().0 < 0;
            format!(
                "__builtin_classify_type({expression}) == 1 && (((__typeof__({expression}))-1 < 0) == {})",
                u8::from(signed)
            )
        }
    };
    format!(
        "_Static_assert(sizeof({expression}) == {} && ({predicate}), \"native scalar ABI mismatch\");\n",
        scalar.size()
    )
}

fn check_type(
    actual: &Value,
    expected: &SourceAbiType,
    records: &BTreeMap<String, Vec<(String, AbiScalar)>>,
    names: &BTreeMap<String, String>,
    nodes: &HashMap<String, &Value>,
    assertions: &mut String,
) -> Result<(), String> {
    let actual = unwrap_type(actual)?;
    let kind = actual["kind"].as_str().ok_or("unresolved C type")?;
    let spelling = actual["type"]["qualType"]
        .as_str()
        .ok_or("C type spelling is missing")?;
    match expected {
        SourceAbiType::Unit if kind == "BuiltinType" && spelling == "void" => Ok(()),
        SourceAbiType::Scalar(scalar) if matches!(kind, "BuiltinType" | "EnumType") => {
            assertions.push_str(&scalar_assertion(&format!("({spelling})0"), *scalar));
            Ok(())
        }
        SourceAbiType::Callback { parameters, result } if kind == "PointerType" => {
            let pointee = unwrap_type(
                children(actual)
                    .first()
                    .ok_or("callback pointee is missing")?,
            )?;
            if pointee["kind"] != "FunctionProtoType" {
                return Err("callback requires a complete function pointer prototype".to_owned());
            }
            check_function(
                pointee, parameters, result, records, names, nodes, assertions,
            )
        }
        SourceAbiType::String
        | SourceAbiType::Bytes(_)
        | SourceAbiType::ByteArray
        | SourceAbiType::ByteArrayResult
        | SourceAbiType::Opaque(_)
        | SourceAbiType::UserData(_)
        | SourceAbiType::Record(_)
            if matches!(kind, "PointerType" | "ObjCObjectPointerType") =>
        {
            let pointee = unwrap_type(
                children(actual)
                    .first()
                    .ok_or("object pointer pointee is missing")?,
            )?;
            if matches!(
                pointee["kind"].as_str(),
                Some("FunctionProtoType" | "FunctionNoProtoType")
            ) {
                return Err("object pointer cannot map a function pointer".to_owned());
            }
            if matches!(expected, SourceAbiType::ByteArrayResult) {
                if pointee["kind"] != "RecordType" {
                    return Err("ByteArray result requires a data/length struct pointer".into());
                }
                let declaration = pointee["decl"]["id"]
                    .as_str()
                    .and_then(|id| nodes.get(id))
                    .copied()
                    .ok_or("ByteArray result definition is unavailable")?;
                let fields: Vec<_> = children(declaration)
                    .iter()
                    .filter(|node| node["kind"] == "FieldDecl")
                    .collect();
                if declaration["tagUsed"] != "struct"
                    || children(declaration)
                        .iter()
                        .any(|node| node["kind"] == "PackedAttr")
                    || fields.len() != 2
                    || fields[0]["name"] != "data"
                    || fields[1]["name"] != "length"
                    || fields.iter().any(|node| node["isBitfield"] == true)
                {
                    return Err("ByteArray result requires unpacked data/length fields".into());
                }
                let real = pointee["type"]["qualType"]
                    .as_str()
                    .ok_or("ByteArray result C spelling is missing")?;
                assertions.push_str(&format!("_Static_assert(sizeof({real}) == sizeof(struct __rinzu_byte_array_result) && _Alignof({real}) == _Alignof(struct __rinzu_byte_array_result) && offsetof({real}, data) == 0 && offsetof({real}, length) == offsetof(struct __rinzu_byte_array_result, length) && __builtin_types_compatible_p(__typeof__((({real} *)0)->data), uint8_t *) && __builtin_types_compatible_p(__typeof__((({real} *)0)->length), size_t), \"ByteArray result layout mismatch\");\n"));
            }
            if matches!(expected, SourceAbiType::String)
                && !(pointee["kind"] == "BuiltinType" && pointee["type"]["qualType"] == "char")
            {
                return Err("String requires a char data pointer".to_owned());
            }
            if matches!(expected, SourceAbiType::UserData(_))
                && !(pointee["kind"] == "BuiltinType" && pointee["type"]["qualType"] == "void")
            {
                return Err("user data requires an opaque void pointer".to_owned());
            }
            if let SourceAbiType::Record(identity) = expected {
                if pointee["kind"] != "RecordType" {
                    return Err("native record requires a struct pointer".to_owned());
                }
                let id = pointee["decl"]["id"]
                    .as_str()
                    .ok_or("record declaration is unresolved")?;
                let declaration = nodes
                    .get(id)
                    .ok_or("native record definition is unavailable")?;
                if declaration["tagUsed"] != "struct"
                    || children(declaration)
                        .iter()
                        .any(|node| node["kind"] == "PackedAttr")
                {
                    return Err("native record cannot map a union or packed struct".to_owned());
                }
                let fields: Vec<_> = children(declaration)
                    .iter()
                    .filter(|node| node["kind"] == "FieldDecl")
                    .collect();
                let expected_fields = records
                    .get(identity)
                    .ok_or("native record schema is missing")?;
                if fields.len() != expected_fields.len()
                    || fields
                        .iter()
                        .zip(expected_fields)
                        .any(|(actual, (name, _))| {
                            actual["name"] != name.as_str() || actual["isBitfield"] == true
                        })
                {
                    return Err("native record field order/name/bitfield mismatch".to_owned());
                }
                let real = pointee["type"]["qualType"]
                    .as_str()
                    .ok_or("native record C spelling is missing")?;
                let generated = &names[identity];
                assertions.push_str(&format!("_Static_assert(sizeof({real}) == sizeof({generated}) && _Alignof({real}) == _Alignof({generated}), \"native record layout mismatch\");\n"));
                for (field, scalar) in expected_fields {
                    assertions.push_str(&scalar_assertion(
                        &format!("(({real} *)0)->{field}"),
                        *scalar,
                    ));
                    assertions.push_str(&format!("_Static_assert(offsetof({real}, {field}) == offsetof({generated}, {field}), \"native record offset mismatch\");\n"));
                }
            }
            Ok(())
        }
        _ => Err(format!(
            "native type mismatch: C `{spelling}` cannot map `{expected:?}`"
        )),
    }
}

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
mod tests {
    use super::*;
    fn fixture(source: &str, header: Option<&str>) -> Result<NativeContract, String> {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "rinzu-contract-fixture-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let file = rinzu_compiler::SourceFile::new(
            rinzu_compiler::FileId(1),
            "contract.rinzu",
            format!("{source} fn main() {{}}"),
        );
        let mut options = rinzu_compiler::CompilerOptions::default();
        options.sdk_files.insert(rinzu_compiler::FileId(1));
        let artifact = rinzu_compiler::compile_source_with_options(&file, &options).unwrap();
        let headers = if let Some(text) = header {
            let path = directory.join("api.h");
            std::fs::write(&path, text).unwrap();
            vec![path]
        } else {
            Vec::new()
        };
        let result = validate(
            &TargetClang::resolve().unwrap(),
            &directory,
            &headers,
            &[],
            &artifact.checked.source_native_signatures,
            &artifact.ir.native_records,
        );
        std::fs::remove_dir_all(directory).unwrap();
        result
    }
    const SOURCE: &str = "native { record Point { x: CInt, y: CDouble } fn visit(point: Point, length: CSize, callback: fn(String, CDouble) -> CInt) -> CInt symbol \"visit\"; }";
    const HEADER: &str = "#include <stddef.h>\ntypedef struct ForeignPoint { int x; double y; } ForeignPoint;\ntypedef int (*Callback)(const char *, double);\nint visit(const ForeignPoint *point, size_t length, Callback callback);\n";

    #[test]
    fn byte_array_result_validates_both_fields_and_layout() {
        let source = "native { fn read() -> ByteArray symbol \"read_bytes\"; }";
        assert!(fixture(source, None).is_ok());
        let header = "#include <stdint.h>\n#include <stddef.h>\nstruct Result { uint8_t *data; size_t length; }; const struct Result *read_bytes(void);\n";
        assert!(fixture(source, Some(header)).is_ok());
        for invalid in [
            header.replace("uint8_t *data", "uint16_t *data"),
            header.replace("size_t length", "int64_t length"),
            header.replace(
                "uint8_t *data; size_t length",
                "size_t length; uint8_t *data",
            ),
            header.replace("struct Result {", "struct __attribute__((packed)) Result {"),
            header.replace(
                "uint8_t *data; size_t length",
                "uint8_t *data; size_t length; int extra",
            ),
            header.replace(
                "const struct Result *read_bytes",
                "const uint8_t *read_bytes",
            ),
            header.replace("size_t length", "size_t length:32"),
        ] {
            assert!(
                fixture(source, Some(&invalid)).is_err(),
                "accepted {invalid}"
            );
        }
    }

    #[test]
    fn target_clang_resolves_typedef_callbacks_and_record_layout_without_tag_identity() {
        for header in [None, Some(HEADER)] {
            let contract = fixture(SOURCE, header).unwrap();
            let point = &contract.records["Point"];
            assert_eq!((point.size, point.alignment), (16, 8));
            assert_eq!(point.offsets, [0, 8]);
            assert!(contract.prototypes["visit"].contains("(*a2)(const char * p0, double p1)"));
            assert!(contract.fingerprint.contains_key("clang.binary"));
        }
    }

    #[test]
    fn record_pointer_positions_preserve_actual_const_and_use_mutable_default_loans() {
        let source = "native {record Point{x:CInt} fn visit(first:Point,second:Point,handler:fn(Point)->CInt)->Point symbol \"visit\";}";
        let contract = fixture(source, None).unwrap();
        let record = &contract.records["Point"].c_type;
        let generated = &contract.prototypes["visit"];
        assert!(generated.starts_with(&format!(
            "const {record} * visit({record} * a0, {record} * a1"
        )));
        assert!(generated.contains(&format!("(*a2)(const {record} * p0)")));
        for (first, second, received) in [("", "const ", ""), ("const ", "", "const ")] {
            let header = format!(
                "struct ForeignPoint{{int x;}};typedef {received}struct ForeignPoint *Received;typedef int (*Handler)(Received);{received}struct ForeignPoint *visit({first}struct ForeignPoint *,{second}struct ForeignPoint *,Handler);"
            );
            let contract = fixture(source, Some(&header)).unwrap();
            let actual = contract.functions["visit"].declaration("bridge");
            assert!(
                actual.contains(&format!("{first}struct ForeignPoint * a0")),
                "{actual}"
            );
            assert!(
                actual.contains(&format!("{second}struct ForeignPoint * a1")),
                "{actual}"
            );
            let CType::Callback(handler) = &contract.functions["visit"].parameters[2] else {
                panic!("missing callback prototype")
            };
            assert!(handler.declaration("receive").contains("Received a0"));
            assert!(contract.fingerprint.contains_key("clang.types"));
        }
        for invalid in [
            "struct Point{int x;};struct Point *visit(void *,struct Point *,int (*)(struct Point *));",
            "struct Point{int x;};struct Point *visit(struct Point **,struct Point *,int (*)(struct Point *));",
            "struct Point{int x;};struct Point *visit(struct Point *,struct Point *,int (*)(struct Point **));",
            "struct Point{int x;};struct Point *visit(struct Point *,struct Point *,int (*)(void *));",
            "struct Point{unsigned int x;};struct Point *visit(struct Point *,struct Point *,int (*)(struct Point *));",
            "struct Point{int x;};struct Point *visit(struct Point *,struct Point *,int (*)(struct Point *,int));",
            "struct Point{int x;};struct Point *visit(struct Point *,struct Point *,int (*)(struct Point *,...));",
        ] {
            assert!(
                fixture(source, Some(invalid)).is_err(),
                "accepted {invalid}"
            );
        }
    }
    #[test]
    fn provided_header_mismatches_never_fall_back_to_headerless_contract() {
        for header in [
            HEADER.replace("size_t length", "unsigned int length"),
            HEADER.replace("size_t length", "long long length"),
            HEADER.replace("int visit", "long long visit"),
            HEADER.replace("const char *, double", "const char *, double, void *"),
            HEADER.replace("const ForeignPoint *point", "ForeignPoint point"),
            HEADER.replace("int x; double y;", "double y; int x;"),
            HEADER.replace("int x; double y;", "unsigned int x; double y;"),
            HEADER.replace(
                "struct ForeignPoint",
                "struct __attribute__((packed)) ForeignPoint",
            ),
            HEADER.replace("struct ForeignPoint", "union ForeignPoint"),
            "int visit();".to_owned(),
            "#error invalid provided header".to_owned(),
        ] {
            assert!(
                fixture(SOURCE, Some(&header)).is_err(),
                "accepted header: {header}"
            );
        }
    }
    #[test]
    fn opaque_pointers_need_no_definition_and_cannot_be_function_pointers() {
        let source = "native { opaque Handle; fn create() -> Handle symbol \"create\"; fn use(value: Handle) symbol \"use\"; }";
        fixture(source, Some("typedef struct Incomplete Incomplete; Incomplete *create(void); void use(Incomplete *value);")).unwrap();
        assert!(
            fixture(
                source,
                Some("typedef void (*Handle)(void); Handle create(void); void use(Handle value);")
            )
            .is_err()
        );
    }
    #[test]
    fn header_only_inline_macros_are_rejected_and_record_pointer_qualifiers_are_preserved() {
        let source = "native { fn value() -> CInt symbol \"value\"; }";
        for header in [
            "static inline int value(void) { return 1; }",
            "int other(void);\n#define value other\n",
        ] {
            assert!(fixture(source, Some(header)).is_err());
        }
        let record = "native { record Point { x: CInt } fn fill(value: Point) symbol \"fill\"; }";
        for header in [
            "struct Point { int x; }; void fill(const struct Point *value);",
            "struct Point { int x; }; void fill(struct Point *value);",
        ] {
            fixture(record, Some(header)).unwrap();
        }
    }
    #[test]
    fn all_scalar_widths_signedness_bool_and_floating_prototypes_are_checked() {
        for scalar in [
            AbiScalar::I8,
            AbiScalar::I16,
            AbiScalar::I32,
            AbiScalar::I64,
            AbiScalar::U8,
            AbiScalar::U16,
            AbiScalar::U32,
            AbiScalar::U64,
            AbiScalar::F32,
            AbiScalar::F64,
            AbiScalar::Bool,
        ] {
            let source = format!(
                "native {{ fn identity(value: {}) -> {} symbol \"identity\"; }}",
                scalar.name(),
                scalar.name()
            );
            let header = format!(
                "#include <stdint.h>\n{0} identity({0} value);\n",
                scalar.c_type()
            );
            fixture(&source, Some(&header)).unwrap();
        }
        assert!(
            fixture(
                "native { fn flag(value: CBool) -> CBool symbol \"flag\"; }",
                Some("unsigned char flag(unsigned char value);")
            )
            .is_err()
        );
        assert!(
            fixture(
                "native { fn value(value: CFloat) -> CFloat symbol \"value\"; }",
                Some("double value(double value);")
            )
            .is_err()
        );
    }
}
