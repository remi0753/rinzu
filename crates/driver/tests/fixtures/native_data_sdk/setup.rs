pub fn create(root: &std::path::Path) {
    let _ = std::fs::remove_dir_all(root);
    for name in ["app", "buffers"] {
        std::fs::create_dir_all(root.join(name).join("src")).unwrap();
    }
    std::fs::write(root.join("buffers/rinzu.toml"), "[package]\nname = \"buffers\"\nlanguage = \"0.2\"\nkind = \"sdk\"\nentry = \"src/api.rinzu\"\n[native]\nsources = [\"api.c\"]\n").unwrap();
    std::fs::write(root.join("app/rinzu.toml"), "[package]\nname = \"buffer_app\"\nlanguage = \"0.2\"\n[dependencies]\nbuffers = { path = \"../buffers\" }\n").unwrap();
    std::fs::write(
        root.join("buffers/src/api.rinzu"),
        include_str!("api.rinzu"),
    )
    .unwrap();
    std::fs::write(root.join("buffers/api.c"), include_str!("api.c")).unwrap();
    std::fs::write(root.join("app/src/main.rinzu"), include_str!("main.rinzu")).unwrap();
}
