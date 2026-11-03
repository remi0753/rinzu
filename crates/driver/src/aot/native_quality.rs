//! Generate the same translation units used by AOT/JIT and the C model tests.
//! This builds native inputs but never executes native event loops or the test programs.
use std::path::Path;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) fn write_sources(directory: &Path) -> Result<(), String> {
    use super::*;

    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let package = root.join("tests/fixtures/programs/source-callback");
    let build = build_package(&package, Some(directory.join("source-platform")))?;
    let source = fs::read_to_string(with_suffix(&build.executable, ".runtime.c"))
        .map_err(|error| error.to_string())?;
    let (_, artifact) =
        rinzu_compiler::check_package(&package).map_err(|errors| format_diagnostics(&errors))?;
    let bindings = artifact
        .sdk_packages
        .iter()
        .flat_map(|package| &package.native_imports)
        .map(|import| (import.export_name.clone(), import.import_key))
        .collect();
    let object = emit_source_async_aot_object(&artifact.ir, "main", &bindings)
        .map_err(|error| error.to_string())?;
    fs::write(
        directory.join("compiled-async.c"),
        crate::compiled_async::runtime_source(&object, &source, true),
    )
    .map_err(|error| error.to_string())?;

    let artifact = rinzu_compiler::compile_source(&rinzu_compiler::SourceFile::new(
        rinzu_compiler::FileId(1),
        "typed-worker-resources.rinzu",
        include_str!("../../tests/fixtures/typed_worker_resources.rinzu"),
    ))
    .map_err(|errors| format_diagnostics(&errors))?;
    let object = emit_source_async_aot_object(&artifact.ir, "main", &HashMap::new())
        .map_err(|error| error.to_string())?;
    let plan = async_aot_plan(&artifact, &object)?.ok_or("missing async tooling plan")?;
    let runtime = runtime_shim(&object, &[], Some(&plan), &[])
        .replace("int main(", "int unused_source_main(");
    let mut wait = runtime.clone();
    wait.push_str(include_str!("../../tests/fixtures/async_wait_cycle.c"));
    fs::write(directory.join("async-wait.c"), wait).map_err(|error| error.to_string())?;
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
    Ok(())
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
pub(crate) fn write_sources(_directory: &Path) -> Result<(), String> {
    Err("native C lint contexts require the supported macOS arm64 target".into())
}
