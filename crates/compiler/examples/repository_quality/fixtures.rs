//! Contexts supplied by compiler and SDK regression fixtures.
use super::{options, read, source};
use crate::diagnostics;
use rinzu_compiler::{NativeCliInputs, check_input};
use std::path::Path;

#[path = "dependencies.rs"]
mod dependencies;

pub fn check(root: &Path, path: &Path, text: &str, temporary: &Path) -> Option<Result<(), String>> {
    let name = path.to_string_lossy();
    if name == "crates/codegen/tests/fixtures/m2_closures.rinzu" {
        // Match the backend test's explicit compile_library context.
        return Some(
            rinzu_compiler::compile_source_with_options(
                &source(path, text.into()),
                &rinzu_compiler::CompilerOptions {
                    is_library: true,
                    ..Default::default()
                },
            )
            .map(|_| ())
            .map_err(|errors| diagnostics(path, errors)),
        );
    }
    if name.starts_with("sdk/sqlite/tests/") && !name.contains("/rows/") {
        return Some(dependencies::check(root, path, text, temporary, "sqlite"));
    }
    if name == "sdk/log/tests/main.rinzu" {
        return Some(dependencies::check(root, path, text, temporary, "log"));
    }
    if name == "crates/driver/tests/fixtures/macos_interrupt/main.rinzu"
        || name == "sdk/macos/tests/binding/src/main.rinzu"
    {
        return Some(dependencies::check(root, path, text, temporary, "macos"));
    }
    if name == "sdk/macos/tests/sheets/src/main.rinzu"
        || name == "sdk/macos/tests/sheets/hooks.rinzu.in"
        || name == "tests/fixtures/programs/macos-notepad/tests/exercise.rinzu.in"
    {
        return Some(dependencies::macos_sheets(root, path, text, temporary));
    }
    if name == "tests/fixtures/programs/editor/tests/ime.rinzu.in" {
        return Some((|| {
            let main = read(&root.join("tests/fixtures/programs/editor/src/main.rinzu"))?
                .replace("async fn main()", "async fn legacyMain()");
            dependencies::check(root, path, &format!("{main}\n{text}"), temporary, "macos")
        })());
    }
    if name == "crates/codegen/tests/fixtures/async_composite_cancel.rinzu"
        || name == "crates/codegen/tests/fixtures/worker_composite_cancel.rinzu"
    {
        return Some((|| {
            let base = root.join(name.replace("_cancel.rinzu", ".rinzu"));
            let text = format!(
                "{}\n{text}",
                read(&base)?.replace("async fn main()", "async fn successful_main()")
            );
            let file = source(path, text);
            rinzu_compiler::compile_source(&file)
                .map(|_| ())
                .map_err(|errors| diagnostics(path, errors))
        })());
    }
    if name.starts_with("crates/driver/tests/fixtures/")
        && root
            .join(path.parent().unwrap())
            .join("api.rinzu")
            .is_file()
    {
        return Some(sdk_fixture(root, path, text, temporary));
    }
    None
}

fn sdk_fixture(root: &Path, path: &Path, text: &str, temporary: &Path) -> Result<(), String> {
    let directory = root.join(path.parent().unwrap());
    let main = read(&directory.join("main.rinzu"))?;
    let syntax = rinzu_compiler::parse_source(&source(path, main.clone()))
        .map_err(|errors| diagnostics(path, errors))?;
    let dependency = syntax
        .imports
        .iter()
        .find(|import| !rinzu_compiler::standard_library::registered(&import.path.join(".")))
        .ok_or("SDK fixture has no import")?
        .path
        .join(".");
    let work = temporary.join(path.parent().unwrap().file_name().unwrap());
    let sdk = work.join("sdk");
    let app = work.join("app");
    std::fs::create_dir_all(sdk.join("src")).map_err(|error| error.to_string())?;
    std::fs::create_dir_all(app.join("src")).map_err(|error| error.to_string())?;
    for entry in std::fs::read_dir(&directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_file()
        {
            std::fs::copy(entry.path(), sdk.join(entry.file_name()))
                .map_err(|error| error.to_string())?;
        }
    }
    std::fs::copy(directory.join("api.rinzu"), sdk.join("src/api.rinzu"))
        .map_err(|error| error.to_string())?;
    let header = if sdk.join("api.h").is_file() {
        "headers=[\"api.h\"]\n"
    } else {
        ""
    };
    std::fs::write(sdk.join("rinzu.toml"),format!("[package]\nname={dependency:?}\nlanguage=\"0.5\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n[native]\n{header}")).map_err(|error|error.to_string())?;
    std::fs::write(app.join("rinzu.toml"),format!("[package]\nname=\"quality_fixture\"\nlanguage=\"0.5\"\n[dependencies]\n{dependency:?}={{path=\"../sdk\"}}\n")).map_err(|error|error.to_string())?;
    let input = if path.file_name().unwrap() == "api.rinzu" {
        &main
    } else {
        text
    };
    std::fs::write(app.join("src/main.rinzu"), input).map_err(|error| error.to_string())?;
    check_input(&app, &options(), &NativeCliInputs::default())
        .map(|_| ())
        .map_err(|errors| diagnostics(path, errors))
}
