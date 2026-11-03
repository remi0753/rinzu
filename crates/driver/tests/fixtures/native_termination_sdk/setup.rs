pub fn create(root: &std::path::Path) {
    let _ = std::fs::remove_dir_all(root);
    for name in ["app", "termination"] {
        std::fs::create_dir_all(root.join(name).join("src")).unwrap();
    }
    std::fs::write(root.join("termination/rinzu.toml"), "[package]\nname=\"termination\"\nlanguage=\"0.2\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n[native]\nsources=[\"api.c\"]\nheaders=[\"api.h\"]\n").unwrap();
    std::fs::write(root.join("app/rinzu.toml"), "[package]\nname=\"termination_app\"\nlanguage=\"0.2\"\n[dependencies]\ntermination={path=\"../termination\"}\n").unwrap();
    for (path, contents) in [
        ("termination/src/api.rinzu", include_str!("api.rinzu")),
        ("termination/api.h", include_str!("api.h")),
        ("termination/api.c", include_str!("api.c")),
        ("app/src/main.rinzu", include_str!("main.rinzu")),
    ] {
        std::fs::write(root.join(path), contents).unwrap();
    }
}
