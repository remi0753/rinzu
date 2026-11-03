use std::path::{Path, PathBuf};
#[path = "../../../crates/driver/tests/support/source_platform.rs"]
mod common;
pub use common::{bounded, sanitize, sign, success};
pub fn source_sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/programs/sdl2")
}
pub fn ordinary_import(root: &Path) -> PathBuf {
    std::fs::create_dir_all(root.join("app/src")).unwrap();
    let sample = source_sample();
    // Paths resolve relative to the canonical temp application root.
    let sdk = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../sdk")
        .canonicalize()
        .unwrap();
    let relative = |destination: &Path| {
        let depth = root
            .join("app")
            .canonicalize()
            .unwrap()
            .components()
            .count()
            - 1;
        format!(
            "{}{}",
            "../".repeat(depth),
            destination.strip_prefix("/").unwrap().display()
        )
    };
    std::fs::write(root.join("app/rinzu.toml"),format!("[package]\nname=\"sdl_source_app\"\nlanguage=\"0.5\"\n[dependencies]\n\"rinzu.sdl2\"={{path=\"{}\"}}\n",relative(&sdk.join("sdl2")))).unwrap();
    std::fs::copy(
        sample.join("src/main.rinzu"),
        root.join("app/src/main.rinzu"),
    )
    .unwrap();
    std::fs::create_dir_all(root.join("app/data")).unwrap();
    std::fs::copy(
        sample.join("data/input.txt"),
        root.join("app/data/input.txt"),
    )
    .unwrap();
    root.join("app")
}
