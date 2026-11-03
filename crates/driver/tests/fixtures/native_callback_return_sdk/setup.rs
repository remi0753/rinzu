pub fn create(root: &std::path::Path) {
    let _ = std::fs::remove_dir_all(root);
    for name in ["app", "returns"] {
        std::fs::create_dir_all(root.join(name).join("src")).unwrap();
    }
    std::fs::write(root.join("returns/rinzu.toml"),"[package]\nname=\"returns\"\nlanguage=\"0.2\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n[native]\nsources=[\"api.c\"]\nheaders=[\"api.h\"]\n").unwrap();
    std::fs::write(root.join("app/rinzu.toml"),"[package]\nname=\"return_app\"\nlanguage=\"0.2\"\n[dependencies]\nreturns={path=\"../returns\"}\n").unwrap();
    std::fs::write(
        root.join("returns/src/api.rinzu"),
        include_str!("api.rinzu"),
    )
    .unwrap();
    std::fs::write(root.join("returns/api.h"), include_str!("api.h")).unwrap();
    std::fs::write(root.join("returns/api.c"), include_str!("api.c")).unwrap();
    std::fs::write(root.join("app/src/main.rinzu"), include_str!("main.rinzu")).unwrap();
}
