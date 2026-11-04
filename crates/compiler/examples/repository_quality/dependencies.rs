//! Match private SDK additions used by migration and interruption tests.
use super::{diagnostics, options, read};
use rinzu_compiler::{NativeCliInputs, check_input};
use std::path::{Path, PathBuf};

fn relative(from: &Path, to: &Path) -> PathBuf {
    let left: Vec<_> = from.components().collect();
    let right: Vec<_> = to.components().collect();
    let common = left.iter().zip(&right).take_while(|(a, b)| a == b).count();
    std::iter::repeat_n(std::path::Component::ParentDir, left.len() - common)
        .chain(right[common..].iter().copied())
        .collect()
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|error| error.to_string())?;
    for entry in std::fs::read_dir(from).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry.file_name() == "build" || entry.file_name() == "target" {
            continue;
        }
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        } else {
            std::fs::copy(entry.path(), to.join(entry.file_name()))
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

/// Use the same private declarations and application context as the GUI runners.
/// These fragments are checked as full packages, never exempted from typing.
pub fn macos_sheets(root: &Path, path: &Path, text: &str, temporary: &Path) -> Result<(), String> {
    let work = temporary.join(path.to_string_lossy().replace(['/', '.'], "_"));
    let app = work.join("app");
    let sdk = work.join("sdk");
    std::fs::create_dir_all(app.join("src")).map_err(|error| error.to_string())?;
    copy_tree(&root.join("sdk/macos"), &sdk)?;
    let hooks = if path.ends_with("hooks.rinzu.in") {
        text.to_owned()
    } else {
        read(&root.join("sdk/macos/tests/sheets/hooks.rinzu.in"))?
    };
    let api = sdk.join("src/macos.rinzu");
    std::fs::write(&api, format!("{}\n{hooks}", read(&api)?)).map_err(|error| error.to_string())?;
    let header = sdk.join("native/include/rinzu_macos.h");
    std::fs::write(&header, format!("{}\nint32_t rinzu_macos_test_sheet_reply(void *, int32_t, const uint8_t *, size_t);\nint32_t rinzu_macos_test_sheet_visible(void *);\n", read(&header)?)).map_err(|error| error.to_string())?;
    let native = sdk.join("native/src/macos.m");
    std::fs::write(
        &native,
        format!(
            "{}\n{}",
            read(&native)?,
            read(&sdk.join("tests/sheets/probe.m"))?
        ),
    )
    .map_err(|error| error.to_string())?;
    let entry = if path.ends_with("exercise.rinzu.in") {
        let main = read(&root.join("tests/fixtures/programs/macos-notepad/src/main.rinzu"))?
            .replace(
                "rinzu.macos.applicationWithSignals(",
                "rinzu.macos.application(",
            )
            .replace(
                "await notepad.run(app.primary);",
                "await testNotepad(notepad, app.primary);",
            );
        format!(
            "import rinzu.testing;\nimport rinzu.timer;\n{main}\n{}",
            text.replace("import rinzu.testing;", "")
                .replace("import rinzu.timer;", "")
        )
    } else if path.ends_with("hooks.rinzu.in") {
        read(&root.join("sdk/macos/tests/sheets/src/main.rinzu"))?
    } else {
        text.to_owned()
    };
    std::fs::write(app.join("rinzu.toml"), "[package]\nname=\"quality_sheets\"\nlanguage=\"0.5\"\n[dependencies]\n\"rinzu.macos\"={path=\"../sdk\"}\n").map_err(|error| error.to_string())?;
    std::fs::write(app.join("src/main.rinzu"), entry).map_err(|error| error.to_string())?;
    check_input(&app, &options(), &NativeCliInputs::default())
        .map(|_| ())
        .map_err(|errors| diagnostics(path, errors))
}

pub fn check(
    root: &Path,
    path: &Path,
    text: &str,
    temporary: &Path,
    sdk_name: &str,
) -> Result<(), String> {
    let work = temporary.join(path.to_string_lossy().replace(['/', '.'], "_"));
    let app = work.join("app");
    std::fs::create_dir_all(app.join("src")).map_err(|error| error.to_string())?;
    let mut sdk = root.join("sdk").join(sdk_name);
    if path.ends_with("owned-cancel.rinzu") || sdk_name == "macos" {
        let local = work.join("sdk");
        copy_tree(&sdk, &local)?;
        sdk = local;
        let api = sdk.join("src").join(format!("{sdk_name}.rinzu"));
        let mut declarations = read(&api)?;
        let (extra, header, prototypes) = if sdk_name == "sqlite" {
            (
                "async fn gateCompleted(receipt: GateReceipt) -> Unit { await nativeCompleted(receipt); }\nnative {opaque GateReceipt; fn rawGateReceipt()->GateReceipt symbol \"rinzu_owned_gate_receipt\";fn gateRelease()->Unit symbol \"rinzu_owned_gate_release\";}\nasync fn gateStarted(){await gateCompleted(rawGateReceipt());}\nexport gateStarted;export gateRelease;",
                "rinzu_sqlite.h",
                "void *rinzu_owned_gate_receipt(void);\nvoid rinzu_owned_gate_release(void);",
            )
        } else if path.ends_with("binding/src/main.rinzu") || path.ends_with("ime.rinzu.in") {
            (
                "native {fn rawTestInput(view:ViewHandle,mode:CInt)->CInt symbol \"rinzu_macos_test_input\";} fn testInput(view:View,mode:CInt)->CInt nothrow{return rawTestInput(view.handle,mode);} export testInput;",
                "rinzu_macos.h",
                "int32_t rinzu_macos_test_input(void *, int32_t);",
            )
        } else {
            (
                "native {fn interruptMark(marker:CInt) symbol \"rinzu_macos_interrupt_mark\";} export interruptMark;",
                "rinzu_macos.h",
                "void rinzu_macos_interrupt_mark(int32_t);",
            )
        };
        if path.ends_with("binding/src/main.rinzu") || path.ends_with("ime.rinzu.in") {
            let native = sdk.join("native/src/macos.m");
            let probe = read(&sdk.join("tests/binding/probe.m"))?;
            std::fs::write(&native, format!("{}\n{probe}\n", read(&native)?))
                .map_err(|error| error.to_string())?;
        }
        declarations.push_str(extra);
        std::fs::write(&api, declarations).map_err(|error| error.to_string())?;
        let header = sdk.join("native/include").join(header);
        std::fs::write(&header, format!("{}\n{prototypes}\n", read(&header)?))
            .map_err(|error| error.to_string())?;
    }
    let dependency = relative(
        &app.canonicalize().map_err(|error| error.to_string())?,
        &sdk.canonicalize().map_err(|error| error.to_string())?,
    );
    std::fs::write(app.join("rinzu.toml"),format!("[package]\nname=\"quality_fixture\"\nlanguage=\"0.5\"\n[dependencies]\n\"rinzu.{sdk_name}\"={{path={:?}}}\n",dependency.to_string_lossy())).map_err(|error|error.to_string())?;
    std::fs::write(app.join("src/main.rinzu"), text).map_err(|error| error.to_string())?;
    check_input(&app, &options(), &NativeCliInputs::default())
        .map(|_| ())
        .map_err(|errors| diagnostics(path, errors))
}
