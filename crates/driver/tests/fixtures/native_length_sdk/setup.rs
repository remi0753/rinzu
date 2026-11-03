pub fn create(root: &std::path::Path) {
    let _ = std::fs::remove_dir_all(root);
    for name in ["app", "measure"] {
        std::fs::create_dir_all(root.join(name).join("src")).unwrap();
    }
    std::fs::write(root.join("measure/rinzu.toml"), "[package]\nname = \"measure\"\nlanguage = \"0.2\"\nkind = \"sdk\"\nentry = \"src/api.rinzu\"\n[native]\nsources = [\"api.c\"]\n").unwrap();
    std::fs::write(root.join("app/rinzu.toml"), "[package]\nname = \"length_app\"\nlanguage = \"0.2\"\n[dependencies]\nmeasure = { path = \"../measure\" }\n").unwrap();
    std::fs::write(
        root.join("measure/src/api.rinzu"),
        include_str!("api.rinzu"),
    )
    .unwrap();
    std::fs::write(root.join("measure/api.c"), include_str!("api.c")).unwrap();
    std::fs::write(root.join("app/src/main.rinzu"), include_str!("main.rinzu")).unwrap();
}
