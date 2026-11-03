pub fn create(root: &std::path::Path) {
    let _ = std::fs::remove_dir_all(root);
    for name in ["app", "records"] {
        std::fs::create_dir_all(root.join(name).join("src")).unwrap();
    }
    std::fs::write(root.join("records/rinzu.toml"),"[package]\nname=\"records\"\nlanguage=\"0.2\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n[native]\nsources=[\"api.c\"]\nheaders=[\"api.h\"]\n").unwrap();
    std::fs::write(root.join("app/rinzu.toml"),"[package]\nname=\"record_app\"\nlanguage=\"0.2\"\n[dependencies]\nrecords={path=\"../records\"}\n").unwrap();
    std::fs::write(
        root.join("records/src/api.rinzu"),
        include_str!("api.rinzu"),
    )
    .unwrap();
    std::fs::write(root.join("records/api.c"), include_str!("api.c")).unwrap();
    std::fs::write(root.join("records/api.h"), include_str!("api.h")).unwrap();
    std::fs::write(root.join("app/src/main.rinzu"), include_str!("main.rinzu")).unwrap();
}
