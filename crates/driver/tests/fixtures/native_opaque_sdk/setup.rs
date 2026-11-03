pub fn create(root: &std::path::Path) {
    let _ = std::fs::remove_dir_all(root);
    for name in ["app", "objects"] {
        std::fs::create_dir_all(root.join(name).join("src")).unwrap();
    }
    std::fs::write(root.join("objects/rinzu.toml"), "[package]\nname = \"objects\"\nlanguage = \"0.2\"\nkind = \"sdk\"\nentry = \"src/api.rinzu\"\n[native]\nsources = [\"api.c\"]\n").unwrap();
    std::fs::write(root.join("app/rinzu.toml"), "[package]\nname = \"opaque_app\"\nlanguage = \"0.2\"\n[dependencies]\nobjects = { path = \"../objects\" }\n").unwrap();
    std::fs::write(
        root.join("objects/src/api.rinzu"),
        include_str!("api.rinzu"),
    )
    .unwrap();
    std::fs::write(root.join("objects/api.c"), include_str!("api.c")).unwrap();
    std::fs::write(root.join("app/src/main.rinzu"), include_str!("main.rinzu")).unwrap();
}
