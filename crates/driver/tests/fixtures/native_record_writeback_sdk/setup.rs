pub fn create(root: &std::path::Path) {
    for name in ["app", "records"] {
        std::fs::create_dir_all(root.join(name).join("src")).unwrap();
    }
    std::fs::write(root.join("records/rinzu.toml"), "[package]\nname=\"records\"\nlanguage=\"0.2\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n[native]\nsources=[\"api.c\"]\nheaders=[\"api.h\"]\n").unwrap();
    std::fs::write(root.join("app/rinzu.toml"), "[package]\nname=\"writeback_app\"\nlanguage=\"0.2\"\n[dependencies]\nrecords={path=\"../records\"}\n").unwrap();
    std::fs::write(
        root.join("records/src/api.rinzu"),
        include_str!("api.rinzu"),
    )
    .unwrap();
    std::fs::write(root.join("records/api.h"), include_str!("api.h")).unwrap();
    std::fs::write(root.join("records/api.c"), include_str!("api.c")).unwrap();
    std::fs::write(root.join("app/src/main.rinzu"), "import records; fn execute()->Int {records.exercise();return 0;} fn main(){try{let result=execute();}catch errors{panic(\"unexpected writeback failure\");}}\n").unwrap();
}

pub fn create_sdk_callbacks(root: &std::path::Path) {
    create(root);
    let body = include_str!("busy.rinzu")
        .split("fn main()")
        .next()
        .unwrap()
        .replace("import records;", "")
        .replace("import rinzu.stdio;", "")
        .replace("records.", "");
    std::fs::write(
        root.join("records/src/api.rinzu"),
        format!("{}\n{body}\nexport execute;\n", include_str!("api.rinzu")),
    )
    .unwrap();
    std::fs::write(root.join("app/src/main.rinzu"), "import records;fn execute()->Int{return records.execute();}fn main(){try{let result=execute();}catch errors{panic(\"unexpected SDK Busy failure\");}}").unwrap();
}
