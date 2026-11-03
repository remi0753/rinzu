pub fn create(root: &std::path::Path) {
    let _ = std::fs::remove_dir_all(root);
    for name in ["app", "threads"] {
        std::fs::create_dir_all(root.join(name).join("src")).unwrap();
    }
    std::fs::write(root.join("threads/rinzu.toml"),"[package]\nname=\"threads\"\nlanguage=\"0.2\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n[native]\nsources=[\"api.c\"]\nheaders=[\"api.h\"]\n").unwrap();
    std::fs::write(root.join("app/rinzu.toml"),"[package]\nname=\"thread_app\"\nlanguage=\"0.2\"\n[dependencies]\nthreads={path=\"../threads\"}\n").unwrap();
    for name in ["api.rinzu", "api.c", "api.h"] {
        let data = match name {
            "api.rinzu" => include_str!("api.rinzu"),
            "api.c" => include_str!("api.c"),
            _ => include_str!("api.h"),
        };
        let path = if name == "api.rinzu" {
            root.join("threads/src").join(name)
        } else {
            root.join("threads").join(name)
        };
        std::fs::write(path, data).unwrap();
    }
    std::fs::write(root.join("app/src/main.rinzu"), include_str!("main.rinzu")).unwrap();
}
