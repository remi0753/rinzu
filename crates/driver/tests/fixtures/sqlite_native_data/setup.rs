pub fn create(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    let sdk = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../sdk/sqlite")
        .canonicalize()
        .unwrap();
    for path in [
        "rinzu.toml",
        "src/sqlite.rinzu",
        "native/src/sqlite_source.c",
        "native/include/rinzu_sqlite.h",
    ] {
        let destination = root.join("sqlite").join(path);
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        std::fs::copy(sdk.join(path), destination).unwrap();
    }
    std::fs::write(root.join("rinzu.toml"), "[package]\nname=\"sqlite_native_data\"\nlanguage=\"0.5\"\n[dependencies]\n\"rinzu.sqlite\"={path=\"sqlite\"}\n").unwrap();
    std::fs::write(
        root.join("src/main.rinzu"),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../sdk/sqlite/tests/native-results.rinzu"
        )),
    )
    .unwrap();
}

pub fn create_observer(root: &std::path::Path) {
    create(root);
    std::fs::write(
        root.join("src/main.rinzu"),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../sdk/sqlite/tests/native-observer.rinzu"
        )),
    )
    .unwrap();
}
