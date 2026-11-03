pub fn source() -> String {
    let mut source = include_str!("api.c").to_owned();
    source = source.replace(
        "static _Thread_local size_t ticks;",
        r#"
static void *return_copy_allocate(size_t length) {
  if(getenv("RINZU_TEST_RETURN_SDK_COPY")) {write(2,"RETURN_SDK_COPY_OOM\n",20);return NULL;}
  return malloc(length);
}
static _Thread_local size_t ticks;
"#,
    );
    let needle = "unsigned char *owned = malloc(length)";
    assert_eq!(source.matches(needle).count(), 1);
    source = source.replace(needle, "unsigned char *owned=return_copy_allocate(length)");
    for (signature, marker) in [
        (
            "uint64_t return_strings(const char *(*callback)(int32_t, const char *), int32_t mode) {",
            "RETURN_NATIVE_STRING_ENTER",
        ),
        (
            "uint64_t return_bytes(const uint8_t *(*callback)(int32_t, const char *), int32_t mode) {",
            "RETURN_NATIVE_BYTES_ENTER",
        ),
        (
            "int32_t return_zero(const uint8_t *(*callback)(int32_t)) {",
            "RETURN_NATIVE_ZERO_ENTER",
        ),
        (
            "int32_t return_empty(const char *(*callback)(int32_t)) {",
            "RETURN_NATIVE_EMPTY_ENTER",
        ),
    ] {
        assert_eq!(source.matches(signature).count(), 1);
        source = source.replace(
            signature,
            &format!(
                "{signature}\n  write(2,\"{marker}\\n\",{});",
                marker.len() + 1
            ),
        );
    }
    source.replace(
        "void return_tick(void) {\n  ticks++;\n}",
        "void return_tick(void) {write(2,\"RETURN_FINALLY\\n\",15);ticks++;}",
    )
}
