pub fn create(root: &std::path::Path) {
    let _ = std::fs::remove_dir_all(root);
    for name in ["app", "pump"] {
        std::fs::create_dir_all(root.join(name).join("src")).unwrap();
    }
    std::fs::write(root.join("pump/rinzu.toml"), "[package]\nname=\"pump\"\nlanguage=\"0.2\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n[native]\nsources=[\"api.c\"]\nheaders=[\"api.h\"]\n").unwrap();
    std::fs::write(
        root.join("app/rinzu.toml"),
        "[package]\nname=\"pump_app\"\nlanguage=\"0.2\"\n[dependencies]\npump={path=\"../pump\"}\n",
    )
    .unwrap();
    for (path, contents) in [
        ("pump/src/api.rinzu", include_str!("api.rinzu")),
        ("pump/api.h", include_str!("api.h")),
        ("pump/api.c", include_str!("api.c")),
        ("app/src/main.rinzu", include_str!("main.rinzu")),
    ] {
        std::fs::write(root.join(path), contents).unwrap();
    }
}
