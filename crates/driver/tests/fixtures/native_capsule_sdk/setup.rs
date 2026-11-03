pub fn create(root: &std::path::Path) {
    let _ = std::fs::remove_dir_all(root);
    for name in ["app", "capsules"] {
        std::fs::create_dir_all(root.join(name).join("src")).unwrap();
    }
    std::fs::create_dir_all(root.join("app/data")).unwrap();
    std::fs::write(root.join("app/data/proof.bin"), vec![0x5a; 8193]).unwrap();
    std::fs::write(root.join("capsules/rinzu.toml"), "[package]\nname=\"capsules\"\nlanguage=\"0.2\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n[native]\nsources=[\"api.c\"]\nheaders=[\"api.h\"]\n").unwrap();
    std::fs::write(root.join("app/rinzu.toml"), "[package]\nname=\"capsule_app\"\nlanguage=\"0.5\"\n[dependencies]\ncapsules={path=\"../capsules\"}\n").unwrap();
    std::fs::write(
        root.join("capsules/src/api.rinzu"),
        include_str!("api.rinzu"),
    )
    .unwrap();
    std::fs::write(root.join("capsules/api.h"), include_str!("api.h")).unwrap();
    std::fs::write(root.join("capsules/api.c"), include_str!("api.c")).unwrap();
    std::fs::write(root.join("app/src/main.rinzu"), include_str!("main.rinzu")).unwrap();
}
