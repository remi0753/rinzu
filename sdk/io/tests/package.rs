pub fn create(root: &std::path::Path) -> std::path::PathBuf {
    let _ = std::fs::remove_dir_all(root);
    std::fs::create_dir_all(root.join("app/src")).unwrap();
    std::fs::create_dir_all(root.join("app/data")).unwrap();
    std::fs::write(
        root.join("app/rinzu.toml"),
        "[package]\nname=\"io_source_app\"\nlanguage=\"0.5\"\n",
    )
    .unwrap();
    std::fs::write(root.join("app/src/main.rinzu"), include_str!("main.rinzu")).unwrap();
    std::fs::write(root.join("app/data/input.txt"), b"rinzu").unwrap();
    root.join("app")
}
