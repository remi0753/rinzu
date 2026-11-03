pub fn create(root: &std::path::Path) -> std::path::PathBuf {
    let _ = std::fs::remove_dir_all(root);
    std::fs::create_dir_all(root.join("sdk/src")).unwrap();
    std::fs::create_dir_all(root.join("app/src")).unwrap();
    std::fs::write(root.join("sdk/rinzu.toml"), "[package]\nname=\"perf\"\nlanguage=\"0.2\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n[native]\nsources=[\"api.c\"]\nheaders=[\"api.h\"]\nframeworks=[\"CoreFoundation\"]\n").unwrap();
    for (path, text) in [
        ("sdk/src/api.rinzu", include_str!("api.rinzu")),
        ("sdk/api.c", include_str!("api.c")),
        ("sdk/api.h", include_str!("api.h")),
        ("app/src/main.rinzu", include_str!("main.rinzu")),
        (
            "app/rinzu.toml",
            "[package]\nname=\"native_perf_app\"\nlanguage=\"0.2\"\n[dependencies]\n\"perf\"={path=\"../sdk\"}\n",
        ),
    ] {
        std::fs::write(root.join(path), text).unwrap();
    }
    root.join("app")
}
