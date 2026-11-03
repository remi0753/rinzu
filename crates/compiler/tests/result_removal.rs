use rinzu_compiler::{FileId, SourceFile, compile_source};
fn check(text: &str) -> bool {
    compile_source(&SourceFile::new(FileId(398), "result-removal.rinzu", text)).is_ok()
}
#[test]
fn result_requires_an_ordinary_user_declaration() {
    assert!(!check(
        "fn main(){let value=Result<Int,String>.Ok(value:1);}"
    ));
    assert!(check(
        "enum Result<T,E>{Ok(value:T),Err(error:E)} fn main(){let value=Result<Int,String>.Ok(value:1);switch value{case Result.Ok(found):{}case Result.Err(problem):{}}}"
    ));
}
#[test]
fn checked_intrinsic_errors_obey_main_and_future_boundaries() {
    for call in [
        "parseInt(\"1\")",
        "utf8Decode(byteArray(b\"text\"))",
        "checkedAdd(1,2)",
        "shiftLeft(1,2)",
        "nativeStatus(0,\"test\")",
    ] {
        assert!(!check(&format!("fn main(){{let value={call};}}")), "{call}");
        assert!(
            check(&format!(
                "fn main(){{try{{let value={call};}}catch errors{{}}}}"
            )),
            "{call}"
        );
        assert!(
            !check(&format!(
                "async fn job(){{let value={call};}}fn main(){{job();}}"
            )),
            "{call}"
        );
        assert!(
            check(&format!(
                "async fn job(){{try{{let value={call};}}catch errors{{}}}}fn main(){{job();}}"
            )),
            "{call}"
        );
    }
}
