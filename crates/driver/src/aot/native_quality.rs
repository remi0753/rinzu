//! Generate the same translation units used by AOT/JIT and the C model tests.
//! This builds native inputs but never executes native event loops or the test programs.
use std::path::Path;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "../../tests/fixtures/ready_snapshot.rs"]
mod ready_snapshot;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) fn write_sources(directory: &Path) -> Result<(), String> {
    use super::*;

    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let package = root.join("tests/fixtures/programs/source-callback");
    let context = crate::input::check(
        &package,
        &crate::input::Options::default(),
        &rinzu_compiler::NativeCliInputs::default(),
    )?;
    build_checked_input(&context, Some(directory.join("source-platform")))?;
    let artifact = &context.artifact;
    let imports = artifact
        .sdk_packages
        .iter()
        .flat_map(|package| &package.native_imports);
    let bindings = imports
        .clone()
        .map(|import| (import.export_name.clone(), import.import_key))
        .collect();
    let imports = imports
        .filter(|import| import.runtime_service.is_none())
        .cloned()
        .collect::<Vec<_>>();
    let object = rinzu_codegen::emit_source_async_aot_plans(&artifact.ir, "main", &bindings)
        .map_err(|error| error.to_string())?;
    let plan = async_aot_plan(artifact, &object)?.ok_or("missing compiled async tooling plan")?;
    let native = crate::input::build_native(&context)?;
    let scratch = crate::verify_capture::TemporaryDirectory(native.directory.clone());
    let source = runtime_shim_with_source_packages(
        &object,
        &imports,
        Some(&plan),
        &native.source_packages,
        &scratch.0.join("source-packages"),
    )?;
    fs::write(
        directory.join("compiled-async.c"),
        crate::compiled_async::runtime_source(&object, &source, true),
    )
    .map_err(|error| error.to_string())?;

    let source = rinzu_compiler::SourceFile::new(
        rinzu_compiler::FileId(1),
        root.join("crates/driver/tests/fixtures/typed_worker_resources.rinzu"),
        include_str!("../../tests/fixtures/typed_worker_resources.rinzu"),
    );
    let artifact = rinzu_compiler::compile_source(&source).map_err(|errors| {
        crate::cli_diagnostics::emit_file(&errors, &source);
        "input failed to check".to_owned()
    })?;
    let object = emit_source_async_aot_object(&artifact.ir, "main", &HashMap::new())
        .map_err(|error| error.to_string())?;
    let plan = async_aot_plan(&artifact, &object)?.ok_or("missing async tooling plan")?;
    let runtime = runtime_shim(&object, &[], Some(&plan), &[])
        .replace("int main(", "int unused_source_main(");
    let mut wait = runtime.clone();
    wait.push_str(include_str!("../../tests/fixtures/async_wait_cycle.c"));
    fs::write(directory.join("async-wait.c"), wait).map_err(|error| error.to_string())?;
    let json = crate::input::check(
        &root.join("crates/driver/tests/fixtures/json_operations.rinzu"),
        &crate::input::Options::default(),
        &rinzu_compiler::NativeCliInputs::default(),
    )?;
    let json = rinzu_codegen::emit_baseline_aot_object(&json.artifact.ir, "main")
        .map_err(|error| error.to_string())?;
    fs::write(
        directory.join("json.runtime.c"),
        runtime_shim(&json, &[], None, &[]),
    )
    .map_err(|error| error.to_string())?;
    let worker = |name: &str| {
        let function = object
            .functions
            .iter()
            .find(|function| function.name == name)
            .ok_or_else(|| format!("missing tooling function {name}"))?;
        plan.workers
            .iter()
            .find(|worker| worker.index == function.index)
            .ok_or_else(|| format!("missing tooling worker {name}"))
    };
    let echo = worker("echo")?;
    let prefix = "#include <assert.h>\n#include <stdatomic.h>\n#include <stdlib.h>\n\
        static _Atomic size_t model_native_live;\n\
        static void *model_calloc(size_t count,size_t size) {void *memory=calloc(count,size);if(memory)atomic_fetch_add(&model_native_live,1);return memory;}\n\
        static inline void *model_malloc(size_t size) {void *memory=malloc(size);if(memory)atomic_fetch_add(&model_native_live,1);return memory;}\n\
        static void model_free(void *memory) {if(memory){assert(atomic_fetch_sub(&model_native_live,1));free(memory);}}\n\
        #define calloc model_calloc\n#define malloc model_malloc\n#define free model_free\n";
    let macros = format!(
        "\n#define MODEL_ECHO {}\n#define MODEL_ECHO_RUN rinzu_worker_run_{}\n#define MODEL_EXPANDED {}\n#define MODEL_OBSERVED {}\n#define MODEL_PACKET_TYPE {}\n",
        echo.index,
        echo.index,
        worker("expanded")?.index,
        worker("observed")?.index,
        echo.parameters[0]
    );
    fs::write(
        directory.join("typed-worker.c"),
        format!(
            "{prefix}{runtime}{macros}{}",
            include_str!("../../tests/fixtures/typed_worker_resources.c")
        ),
    )
    .map_err(|error| error.to_string())?;
    write_fixture_sources(&root, directory)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn write_fixture_sources(root: &Path, directory: &Path) -> Result<(), String> {
    use super::*;

    let fixtures = root.join("crates/driver/tests/fixtures");
    let scratch = crate::verify_capture::TemporaryDirectory::new("native-quality-fixtures")?;
    let runtime = |entry: &Path| {
        let name = entry.file_stem().ok_or("fixture entry has no file stem")?;
        let build = build_package(entry, Some(scratch.0.join(name)))?;
        fs::read_to_string(with_suffix(&build.executable, ".runtime.c"))
            .map_err(|error| error.to_string())
    };
    let prelude = |name: &str| format!("#include {:?}\n", fixtures.join(name));
    let renamed = |source: String| source.replace("int main(", "int unused_source_main(");

    let bytes = renamed(runtime(&fixtures.join("byte_construction.rinzu"))?);
    let lists = renamed(runtime(&fixtures.join("list_updates.rinzu"))?);
    let direct = usize::from(lists.contains("rinzu_managed_list_update_owned(data, value,"));
    let list_prefix = format!(
        "#define TEST_LIST_DIRECT {direct}\n{}{lists}",
        prelude("list_update_alloc.h")
    );
    let ledger = renamed(runtime(
        &root.join("crates/codegen/tests/fixtures/observed-ledger-alias.rinzu"),
    )?);
    let ready =
        ready_snapshot::ready_snapshot_prefix(&runtime(&fixtures.join("async_ready_order.rinzu"))?);
    let lengths = renamed(runtime(&fixtures.join("scalar_lengths.rinzu"))?);
    let virtual_bytes = runtime(&fixtures.join("virtual_bytes_option_kernel.rinzu"))?;

    let source = rinzu_compiler::SourceFile::new(
        rinzu_compiler::FileId(1),
        "<async-result-pin-identity>",
        "async fn main() {}",
    );
    let artifact = rinzu_compiler::compile_source(&source).map_err(|errors| {
        crate::cli_diagnostics::emit_file(&errors, &source);
        "input failed to check".to_owned()
    })?;
    let object = emit_source_async_aot_object(&artifact.ir, "main", &HashMap::new())
        .map_err(|error| error.to_string())?;
    let plan = async_aot_plan(&artifact, &object)?.ok_or("missing result pin tooling plan")?;
    let result_pin = renamed(runtime_shim(&object, &[], Some(&plan), &[]));

    let prefixes = [
        ("async_result_pin_identity.c", result_pin),
        ("bytes_concat_all_guards.c", bytes.clone()),
        (
            "bytes_initialization_guards.c",
            prelude("bytes_poison_alloc.h") + &bytes,
        ),
        ("bytes_share_guards.c", bytes),
        ("list_update_guards.c", list_prefix),
        (
            "observed_ledger_guards.c",
            format!("#define TEST_LEDGER_POLICY 2\n{ledger}"),
        ),
        ("ready_snapshot_guards.c", ready),
        ("scalar_length_guards.c", lengths),
        (
            "virtual_bytes_option_guards.c",
            format!(
                "{}#define main c06_generated_main\n{virtual_bytes}\n#undef main\n",
                prelude("virtual_bytes_option_alloc.h"),
            ),
        ),
    ];
    let mut names = Vec::new();
    for (name, mut source) in prefixes {
        source
            .push_str(&fs::read_to_string(fixtures.join(name)).map_err(|error| error.to_string())?);
        fs::write(directory.join(name), source).map_err(|error| error.to_string())?;
        names.push(name);
    }
    let poll = "poll_state_matrix.c";
    fs::write(directory.join(poll), poll_fixture::source()).map_err(|error| error.to_string())?;
    names.push(poll);
    fs::write(
        directory.join("fixture-sources.json"),
        serde_json::to_vec(&names).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
pub(crate) fn write_sources(_directory: &Path) -> Result<(), String> {
    Err("native C lint contexts require the supported macOS arm64 target".into())
}
