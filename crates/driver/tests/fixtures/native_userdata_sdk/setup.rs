pub fn create(root: &std::path::Path) {
    let _ = std::fs::remove_dir_all(root);
    for name in ["app", "userdata"] {
        std::fs::create_dir_all(root.join(name).join("src")).unwrap();
    }
    std::fs::write(root.join("userdata/rinzu.toml"), "[package]\nname=\"userdata\"\nlanguage=\"0.2\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n[native]\nsources=[\"api.c\"]\nheaders=[\"api.h\"]\n").unwrap();
    std::fs::write(root.join("app/rinzu.toml"), "[package]\nname=\"userdata_app\"\nlanguage=\"0.2\"\n[dependencies]\nuserdata={path=\"../userdata\"}\n").unwrap();
    std::fs::write(
        root.join("userdata/src/api.rinzu"),
        include_str!("api.rinzu"),
    )
    .unwrap();
    std::fs::write(root.join("userdata/api.c"), include_str!("api.c")).unwrap();
    std::fs::write(root.join("userdata/api.h"), include_str!("api.h")).unwrap();
    std::fs::write(root.join("app/src/main.rinzu"), include_str!("main.rinzu")).unwrap();
}
