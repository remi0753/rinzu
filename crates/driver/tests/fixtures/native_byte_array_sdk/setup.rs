pub fn create(root: &std::path::Path) {
    for name in ["app/src", "spans/src"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }
    std::fs::write(root.join("app/rinzu.toml"), "[package]\nname=\"span_app\"\nlanguage=\"0.5\"\n[dependencies]\nspans={path=\"../spans\"}\n").unwrap();
    std::fs::write(root.join("spans/rinzu.toml"), "[package]\nname=\"spans\"\nlanguage=\"0.5\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n[native]\nsources=[\"api.c\"]\nheaders=[\"api.h\"]\n").unwrap();
    for (name, bytes) in [
        ("spans/src/api.rinzu", include_str!("api.rinzu")),
        ("spans/api.c", include_str!("api.c")),
        ("spans/api.h", include_str!("api.h")),
        ("app/src/main.rinzu", include_str!("main.rinzu")),
    ] {
        std::fs::write(root.join(name), bytes).unwrap();
    }
}
