pub fn create(root: &std::path::Path) {
    for name in ["app", "qualifiers"] {
        std::fs::create_dir_all(root.join(name).join("src")).unwrap();
    }
    std::fs::write(root.join("qualifiers/rinzu.toml"), "[package]\nname=\"qualifiers\"\nlanguage=\"0.2\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n[native]\nsources=[\"api.c\"]\nheaders=[\"api.h\"]\n").unwrap();
    std::fs::write(root.join("app/rinzu.toml"), "[package]\nname=\"qualifier_app\"\nlanguage=\"0.2\"\n[dependencies]\nqualifiers={path=\"../qualifiers\"}\n").unwrap();
    std::fs::write(
        root.join("qualifiers/src/api.rinzu"),
        include_str!("api.rinzu"),
    )
    .unwrap();
    std::fs::write(root.join("qualifiers/api.h"), include_str!("api.h")).unwrap();
    std::fs::write(root.join("qualifiers/api.c"), include_str!("api.c")).unwrap();
    std::fs::write(root.join("app/src/main.rinzu"), "import qualifiers;import rinzu.stdio;fn execute()->Int{return qualifiers.exercise();}async fn main(){try{let value=execute();rinzu.stdio.writeLine(value);}catch errors{panic(\"unexpected qualifier Error\");}}").unwrap();
}
