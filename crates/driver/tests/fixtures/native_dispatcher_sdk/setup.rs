pub fn create(root: &std::path::Path) {
    let _ = std::fs::remove_dir_all(root);
    for name in ["app", "dispatcher"] {
        std::fs::create_dir_all(root.join(name).join("src")).unwrap();
    }
    std::fs::create_dir_all(root.join("app/data")).unwrap();
    std::fs::write(root.join("app/data/proof.bin"), vec![0x5a; 8193]).unwrap();
    std::fs::write(root.join("dispatcher/rinzu.toml"), "[package]\nname=\"dispatcher\"\nlanguage=\"0.2\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n[native]\nsources=[\"api.c\"]\nheaders=[\"api.h\"]\nframeworks=[\"CoreFoundation\"]\n").unwrap();
    std::fs::write(root.join("app/rinzu.toml"), "[package]\nname=\"dispatcher_app\"\nlanguage=\"0.2\"\n[dependencies]\ndispatcher={path=\"../dispatcher\"}\n").unwrap();
    for (path, contents) in [
        ("dispatcher/src/api.rinzu", include_str!("api.rinzu")),
        ("dispatcher/api.h", include_str!("api.h")),
        ("dispatcher/api.c", include_str!("api.c")),
        ("app/src/main.rinzu", include_str!("main.rinzu")),
    ] {
        std::fs::write(root.join(path), contents).unwrap();
    }
}
