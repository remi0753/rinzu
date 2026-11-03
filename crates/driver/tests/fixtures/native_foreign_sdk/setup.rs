use crate::source_edit::ReplaceChecked;

pub fn cancellation_source(process_main: i32) -> String {
    let (callbacks, _) = include_str!("main.rinzu")
        .split_once("async fn main() {")
        .unwrap();
    format!(
        "{callbacks}\nasync fn main(){{\ntry {{foreign.reset({process_main});\ntry {{\nlet result=await foreign.runProtected(scalar,unit,text,bytes,failed,failedText,failedBytes,byteLength,unsupported,0);\npanic(\"cancellation was lost\");\n}} finally {{if foreign.finish()!=1{{panic(\"wrongFinish\");}}}}\n}}catch unexpectedErrors{{panic(\"unexpectedFailure\");}}\n}}\n"
    )
}
pub fn negative_sources(process_main: i32) -> Vec<(&'static str, String, &'static str)> {
    let source = include_str!("main.rinzu");
    let run = run_statement(source);
    [
        ("unsupported",with_mode(source,1),"unsupported-signature"),
        ("bounded",with_mode(source,2),"nested-native-wait"),
        ("unknown-mode",with_mode(source,3),"unverified-runloop-progress"),
        ("ordinary",source.replace_checked(run,"let result=foreign.ordinary(text);"),"no-compatible-origin"),
        ("worker",format!("{}\nfn workerText(label:String)->String fallback \"fallback\" {{foreign.bodyMarker();return label;}}\nfn work()->CInt {{return foreign.ordinary(workerText);}}\n",
            source.replace_checked(run,"let pending=spawn work();let result=await pending;")),"no-compatible-origin"),
    ].into_iter().map(|(name,source,reason)|(name,source.replace_checked("foreign.reset(0)",&format!("foreign.reset({process_main})")),reason)).collect()
}

fn run_statement(source: &str) -> &str {
    let start = source
        .find("let result = await foreign.run(")
        .expect("foreign fixture call");
    let end = source[start..]
        .find(';')
        .expect("foreign fixture statement end");
    &source[start..=start + end]
}

pub fn with_mode(source: &str, mode: i32) -> String {
    let statement = run_statement(source);
    let close = statement
        .rfind(')')
        .expect("foreign call closing parenthesis");
    let comma = statement[..close]
        .rfind(',')
        .expect("foreign call mode separator");
    let argument = &statement[comma + 1..close];
    assert_eq!(argument.trim(), "0", "foreign fixture default mode changed");
    let start = comma + 1 + argument.find('0').unwrap();
    let replacement = format!("{}{mode}{}", &statement[..start], &statement[start + 1..]);
    source.replace_checked(statement, &replacement)
}
pub fn create(root: &std::path::Path) {
    let _ = std::fs::remove_dir_all(root);
    for name in ["app", "foreign"] {
        std::fs::create_dir_all(root.join(name).join("src")).unwrap();
    }
    std::fs::create_dir_all(root.join("app/data")).unwrap();
    std::fs::write(root.join("app/data/proof.bin"), vec![0x5a; 8193]).unwrap();
    std::fs::write(root.join("foreign/rinzu.toml"), "[package]\nname=\"foreign\"\nlanguage=\"0.2\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n[native]\nsources=[\"api.c\"]\nheaders=[\"api.h\"]\nframeworks=[\"CoreFoundation\"]\n").unwrap();
    std::fs::write(root.join("app/rinzu.toml"), "[package]\nname=\"foreign_app\"\nlanguage=\"0.2\"\n[dependencies]\nforeign={path=\"../foreign\"}\n").unwrap();
    for (path, contents) in [
        ("foreign/src/api.rinzu", include_str!("api.rinzu")),
        ("foreign/api.h", include_str!("api.h")),
        ("foreign/api.c", include_str!("api.c")),
        ("app/src/main.rinzu", include_str!("main.rinzu")),
    ] {
        std::fs::write(root.join(path), contents).unwrap();
    }
}
