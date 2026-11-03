fn main() {
    println!("cargo:rerun-if-changed=../../include/rinzu/runtime.h");
    println!("cargo:rerun-if-changed=tests/abi_layout.c");

    cc::Build::new()
        .file("tests/abi_layout.c")
        .include("../../include")
        .define("RINZU_RUNTIME_BUILD", None)
        .warnings_into_errors(true)
        .compile("rinzu_abi_layout_fixture");
}
