pub fn create(root: &std::path::Path) {
    for name in ["app", "captures"] {
        std::fs::create_dir_all(root.join(name).join("src")).unwrap();
    }
    std::fs::write(root.join("captures/rinzu.toml"), "[package]\nname=\"captures\"\nlanguage=\"0.5\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n[native]\nsources=[\"api.c\"]\nheaders=[\"api.h\"]\n").unwrap();
    std::fs::write(root.join("app/rinzu.toml"), "[package]\nname=\"capture_app\"\nlanguage=\"0.5\"\n[dependencies]\ncaptures={path=\"../captures\"}\n").unwrap();
    std::fs::write(
        root.join("captures/src/api.rinzu"),
        include_str!("api.rinzu"),
    )
    .unwrap();
    std::fs::write(root.join("captures/api.h"), include_str!("api.h")).unwrap();
    std::fs::write(root.join("captures/api.c"), include_str!("api.c")).unwrap();
    std::fs::write(root.join("app/src/main.rinzu"), include_str!("app.rinzu")).unwrap();
}
