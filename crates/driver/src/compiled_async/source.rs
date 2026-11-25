//! Split the compiler's explicitly marked program fragments from the shared
//! compiled runtime. Both objects consume the same private declarations.
use rinzu_codegen::BaselineAotObject;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::Path;

pub(crate) const COMMON: &str = "compiled-async.runtime.c";
pub(crate) const PROGRAM: &str = "compiled-async.program.c";
pub(crate) const HEADER: &str = "compiled-async.shared.h";

pub(crate) struct Sources {
    pub(crate) files: BTreeMap<String, String>,
}

impl Sources {
    pub(crate) fn split(
        aot: &BaselineAotObject,
        source: String,
        resource_reporting: bool,
    ) -> Result<Self, String> {
        Self::split_with_locations(aot, source, resource_reporting, None)
    }

    pub(crate) fn split_with_locations(
        aot: &BaselineAotObject,
        source: String,
        resource_reporting: bool,
        locations: Option<&Path>,
    ) -> Result<Self, String> {
        let (common, program) = extract(&source, "PROGRAM")?;
        let (common, _) = extract(&common, "AOT_ONLY")?;
        let (mut common, types) = extract(&common, "SHARED")?;
        let mut features = String::new();
        for line in source.lines() {
            if line.starts_with("#define RINZU_AOT_HAS_")
                || line.starts_with("#define RINZU_ERROR_TRACE_DEFAULT ")
            {
                features.push_str(line);
                features.push('\n');
            }
        }
        // These are emitted as part of the program's immutable type catalog.
        let (program, operations) = catalog(program)?;
        let reporting = u8::from(resource_reporting);
        let header = format!(
            "#ifndef RINZU_JIT_SHARED_H\n#define RINZU_JIT_SHARED_H\n             #define RINZU_PROGRAM_API __attribute__((visibility(\"hidden\")))\n             #define RINZU_RESOURCE_API RINZU_PROGRAM_API\n#define RINZU_RESOURCE_REPORTING {reporting}\n             #define RINZU_JIT_FUNCTION_COUNT rinzu_jit_function_count\n             #define RINZU_JIT_STUB_COUNT rinzu_jit_stub_count\n             #define RINZU_AOT_PREPARED_RECORD_RECEIVERS 1\n             {features}{}\n{types}\n{operations}\n{}\n#endif\n",
            template(
                "compiled_async_shared.h",
                include_str!("../compiled_async_shared.h"),
                locations
            ),
            template(
                "compiled_async_program.h",
                include_str!("../compiled_async_program.h"),
                locations
            )
        );
        let mut program = program;
        if !program.contains("int rinzu_native_receive_record_prepared(") {
            program.push_str("RINZU_PROGRAM_API int rinzu_native_receive_record_prepared(rinzu_managed_data_t *data, const rinzu_managed_root_set_t *root, uint64_t index, const unsigned char *input, uint64_t *output) { (void)root; return rinzu_managed_receive_record(data, rinzu_native_record_layout(index), input, output); }\n");
        }

        let mut remove = String::new();
        for function in &aot.functions {
            let types = std::iter::once("void *")
                .chain(std::iter::repeat_n("int64_t", function.parameter_count))
                .collect::<Vec<_>>()
                .join(",");
            remove.push_str(&format!(
                "#define {} (*(int64_t (*)({types}))rinzu_jit_functions[{}])\n",
                function.symbol, function.index
            ));
        }
        common = common.replace(&remove, "");
        let signatures = super::native_signatures(aot);
        for (index, signature) in signatures.iter().enumerate() {
            if signature.is_supplant {
                let wrapper = format!(
                    "uint64_t rinzu_source_native_{index}(uint64_t address,const uint64_t *words) {{return ((uint64_t (*)(uint64_t,const uint64_t *))rinzu_jit_stubs[{index}])(address,words);}}\n"
                );
                common = common.replace(&wrapper, "");
                program.insert_str(0, &wrapper);
            }
        }
        let counts = format!(
            "#define RINZU_JIT_FUNCTION_COUNT {}\n#define RINZU_JIT_STUB_COUNT {}\n",
            aot.functions.len(),
            signatures.len()
        );
        common = common.replace(&counts, "");
        common = common.replace("static uintptr_t rinzu_jit_functions[RINZU_JIT_FUNCTION_COUNT ? RINZU_JIT_FUNCTION_COUNT : 1];\n", "");
        common = common.replace(
            "static uintptr_t rinzu_jit_stubs[RINZU_JIT_STUB_COUNT ? RINZU_JIT_STUB_COUNT : 1];\n",
            "",
        );
        for declaration in [
            "rinzu_jit_timing_t *rinzu_jit_timing;",
            "uint64_t rinzu_jit_cancel_after, rinzu_jit_shutdown_after, rinzu_jit_cancel_worker_after;",
            "rinzu_value_schema_t rinzu_program_schema;",
            "uint64_t (*rinzu_jit_context_identity)(void);",
        ] {
            common = common.replace(
                &format!("static {declaration}"),
                &format!("__attribute__((visibility(\"hidden\"))) {declaration}"),
            );
        }
        common = common.replace(
            "sizeof rinzu_program_types / sizeof *rinzu_program_types",
            "rinzu_program_type_count",
        );
        common = common.replace(
            "sizeof rinzu_program_sources / sizeof *rinzu_program_sources - 1",
            "rinzu_program_source_count",
        );
        common = common.replace(
            "sizeof rinzu_jit_functions",
            "(rinzu_jit_function_count * sizeof *rinzu_jit_functions)",
        );
        common = common.replace(
            "sizeof rinzu_jit_stubs",
            "(rinzu_jit_stub_count * sizeof *rinzu_jit_stubs)",
        );
        // The report uses the current program's source map.
        let resource = common
            .find("#define RINZU_RESOURCE_SOURCES ")
            .ok_or("missing JIT resource source map")?;
        let end = resource
            + common[resource..]
                .find('\n')
                .ok_or("invalid resource source map")?;
        let resource_literal = common[resource..end]
            .strip_prefix("#define RINZU_RESOURCE_SOURCES ")
            .ok_or("invalid resource source map")?
            .to_owned();
        common.replace_range(
            resource..end,
            "#define RINZU_RESOURCE_SOURCES rinzu_program_resource_sources",
        );
        program.push_str(&format!(
            "\nconst char rinzu_program_resource_sources[] = {resource_literal};\n"
        ));
        program = program.replace(
            "static int rinzu_jit_main(",
            "RINZU_PROGRAM_API int rinzu_jit_main(",
        );
        for declaration in [
            "const uint32_t rinzu_program_json_errors[]",
            "const RinzuJsonType rinzu_program_json_types[]",
            "const RinzuJsonField rinzu_program_json_fields[]",
            "const unsigned char rinzu_program_json_names[]",
            "const rinzu_value_type_t rinzu_program_types[]",
            "const unsigned char rinzu_program_slot_references[]",
            "const rinzu_display_type_t rinzu_program_display[]",
            "const rinzu_source_entry_t rinzu_program_sources[]",
            "const rinzu_native_record_layout_t rinzu_native_record_layouts[]",
            "const size_t rinzu_native_record_layout_count",
            "const RinzuNativePlan rinzu_native_plans[]",
            "const size_t rinzu_native_plan_count",
            "const void *rinzu_native_addresses[",
            "const RinzuNativeTarget rinzu_native_targets[]",
            "const size_t rinzu_native_target_count",
            "const void *rinzu_native_target_addresses[",
        ] {
            program = program.replace(
                &format!("static {declaration}"),
                &format!("__attribute__((visibility(\"hidden\"))) {declaration}"),
            );
        }
        program = program.replace(
            "static void rinzu_configure_program_sources(",
            "RINZU_PROGRAM_API void rinzu_configure_program_sources(",
        );
        program = program.replace(
            "static __attribute__((unused)) int rinzu_native_receive_record_prepared(",
            "RINZU_PROGRAM_API int rinzu_native_receive_record_prepared(",
        );
        program.push_str(&format!(
            "\nconst size_t rinzu_program_type_count = sizeof rinzu_program_types / sizeof *rinzu_program_types;\n             const size_t rinzu_program_source_count = sizeof rinzu_program_sources / sizeof *rinzu_program_sources - 1;\n             const size_t rinzu_jit_function_count = {};\n             const size_t rinzu_jit_stub_count = {};\n             uintptr_t rinzu_jit_functions[{}];\nuintptr_t rinzu_jit_stubs[{}];\n",
            aot.functions.len(), signatures.len(),
            aot.functions.len().max(1), signatures.len().max(1)
        ));
        let prefix = format!("#include \"{HEADER}\"\n");
        Ok(Self {
            files: BTreeMap::from([
                (
                    COMMON.into(),
                    format!("#define RINZU_RUNTIME_IMPLEMENTATION 1\n{prefix}{common}"),
                ),
                (PROGRAM.into(), format!("{prefix}{remove}{program}")),
                (HEADER.into(), header),
            ]),
        })
    }
}

/// Attach locations before splitting/replacing template text, so removed
/// declarations and changed linkage cannot shift a later diagnostic. Keep
/// preprocessor continuation lines together; #line inside a macro is invalid.
pub(super) fn template<'a>(name: &str, contents: &'a str, root: Option<&Path>) -> Cow<'a, str> {
    let Some(root) = root else {
        return Cow::Borrowed(contents);
    };
    let path = root.join("crates/driver/src").join(name);
    let quoted = crate::resource_report::c_string(&path.to_string_lossy());
    let mut mapped = String::new();
    let mut continuation = false;
    for (index, line) in contents.split_inclusive('\n').enumerate() {
        if !continuation {
            mapped.push_str(&format!("#line {} {quoted}\n", index + 1));
        }
        mapped.push_str(line);
        continuation = line.trim_end_matches(['\n', '\r']).ends_with('\\');
    }
    Cow::Owned(mapped)
}

fn extract(source: &str, kind: &str) -> Result<(String, String), String> {
    let begin = format!("/* RINZU_JIT_{kind}_BEGIN */");
    let end = format!("/* RINZU_JIT_{kind}_END */");
    let mut shared = String::new();
    let mut selected = String::new();
    let mut remaining = source;
    while let Some(at) = marker(remaining, &begin) {
        shared.push_str(&remaining[..at]);
        remaining = &remaining[at + begin.len()..];
        let at = marker(remaining, &end).ok_or("unterminated JIT source section")?;
        if marker(&remaining[..at], &begin).is_some() {
            return Err("nested JIT source section".into());
        }
        selected.push_str(&remaining[..at]);
        selected.push('\n');
        remaining = &remaining[at + end.len()..];
    }
    shared.push_str(remaining);
    Ok((shared, selected))
}

// Generated section markers occupy their own lines. A quoted SDK name or
// source path may contain the same text and remains ordinary program data.
fn marker(source: &str, marker: &str) -> Option<usize> {
    source.match_indices(marker).find_map(|(at, _)| {
        let before = source[..at].rsplit('\n').next()?;
        let after = source[at + marker.len()..].split('\n').next()?;
        (before.trim().is_empty() && after.trim().is_empty()).then_some(at)
    })
}

fn catalog(source: String) -> Result<(String, String), String> {
    let mut program = String::new();
    let mut operations = String::new();
    for line in source.lines() {
        if line.starts_with("enum { RINZU_DATA_") {
            operations.push_str(line);
            operations.push('\n');
        } else if let Some(constant) = line.strip_prefix("#define rinzu_")
            && constant.contains("_length_overflow ")
        {
            let (name, value) = constant.split_once(' ').ok_or("invalid overflow type")?;
            program.push_str(&format!("const size_t rinzu_{name} = {value};\n"));
        } else {
            program.push_str(line);
            program.push('\n');
        }
    }
    Ok((program, operations))
}
