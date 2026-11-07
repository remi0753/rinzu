pub(crate) fn ready_snapshot_prefix(source: &str) -> String {
    let wrapper = source
        .find("static void rinzu_aot_async_resume_ready(")
        .unwrap();
    let begin = source[..wrapper]
        .rfind("rinzu_aot_async_resume_snapshot(")
        .map(|symbol| {
            source[..symbol]
                .rfind("static __attribute__((noinline)) void")
                .unwrap()
        })
        .unwrap_or(wrapper);
    let end = source[wrapper..].find("\n}\n").unwrap() + wrapper + 3;
    let body = &source[begin..end];
    let indexed = source.contains("static void rinzu_aot_async_queue_ready(");
    let resume = "rinzu_aot_async_resume(";
    assert_eq!(
        body.matches(resume).count(),
        if begin == wrapper { 1 } else { 2 }
    );
    assert_eq!(source.matches("int main(").count(), 1);
    format!(
        "#define TEST_READY_INDEX {}\n{}\nstatic __attribute__((noinline)) void ready_probe_resume(RinzuContext *, RinzuAsyncTask *);\n{}{}",
        usize::from(indexed),
        &source[..begin],
        body.replace(resume, "ready_probe_resume("),
        &source[end..],
    )
    .replacen("int main(", "int unused_source_main(", 1)
}
