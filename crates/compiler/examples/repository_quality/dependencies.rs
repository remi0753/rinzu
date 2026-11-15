//! Check SQLite regressions with their test-only declarations.
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

pub fn sqlite(root: &Path, path: &Path, text: &str, temporary: &Path) -> Result<(), String> {
    let work = temporary.join(path.to_string_lossy().replace(['/', '.'], "_"));
    let app = work.join("app");
    std::fs::create_dir_all(app.join("src")).map_err(|error| error.to_string())?;
    let mut sdk = root.join("sdk/sqlite");
    if path.ends_with("owned-cancel.rinzu") {
        let local = work.join("sdk");
        copy_tree(&sdk, &local)?;
        sdk = local;
        let api = sdk.join("src/sqlite.rinzu");
        let mut declarations = read(&api)?;
        let extra = "async fn gateCompleted(receipt: GateReceipt) -> Unit { await nativeCompleted(receipt); }\nnative {opaque GateReceipt; fn rawGateReceipt()->GateReceipt symbol \"rinzu_owned_gate_receipt\";fn gateRelease()->Unit symbol \"rinzu_owned_gate_release\";}\nasync fn gateStarted(){await gateCompleted(rawGateReceipt());}\nexport gateStarted;export gateRelease;";
        let prototypes =
            "void *rinzu_owned_gate_receipt(void);\nvoid rinzu_owned_gate_release(void);";
        declarations.push_str(extra);
        std::fs::write(&api, declarations).map_err(|error| error.to_string())?;
        let header = sdk.join("native/include/rinzu_sqlite.h");
        std::fs::write(&header, format!("{}\n{prototypes}\n", read(&header)?))
            .map_err(|error| error.to_string())?;
    }
    let dependency = relative(
        &app.canonicalize().map_err(|error| error.to_string())?,
        &sdk.canonicalize().map_err(|error| error.to_string())?,
    );
    std::fs::write(app.join("rinzu.toml"),format!("[package]\nname=\"quality_fixture\"\nlanguage=\"0.5\"\n[dependencies]\n\"rinzu.sqlite\"={{path={:?}}}\n",dependency.to_string_lossy())).map_err(|error|error.to_string())?;
    std::fs::write(app.join("src/main.rinzu"), text).map_err(|error| error.to_string())?;
    check_input(&app, &options(), &NativeCliInputs::default())
        .map(|_| ())
        .map_err(|errors| diagnostics(path, errors))
}
