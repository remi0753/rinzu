fn main() {
    println!("cargo:rerun-if-changed=../../include/rinzu/resource_counters.h");
    println!("cargo:rerun-if-changed=../../include/rinzu/managed_data.h");
    println!("cargo:rerun-if-changed=../../include/rinzu/gc_policy.h");
    println!("cargo:rerun-if-changed=../../include/rinzu/managed_composite.h");
    println!("cargo:rerun-if-changed=../../include/rinzu/native_input.h");
    println!("cargo:rerun-if-changed=../../include/rinzu/native_bytes.h");
    println!("cargo:rerun-if-changed=src/display.c");
    println!("cargo:rerun-if-changed=../../include/rinzu/display.h");
    println!("cargo:rerun-if-changed=src/stdio.c");
    println!("cargo:rerun-if-changed=../../include/rinzu/posix_write.h");
    println!("cargo:rerun-if-changed=src/standalone_interrupt.c");
    println!("cargo:rerun-if-changed=../../include/rinzu/standalone_interrupt.h");
    println!("cargo:rerun-if-changed=src/ready_work.c");
    println!("cargo:rerun-if-changed=../../include/rinzu/ready_work.h");
    println!("cargo:rerun-if-changed=src/native_diagnostics.c");
    println!("cargo:rerun-if-changed=../../include/rinzu/native_diagnostics.h");
    println!("cargo:rerun-if-changed=src/foreign_queue.c");
    println!("cargo:rerun-if-changed=../../include/rinzu/foreign_queue.h");
    for resource in ["json.c", "json.h", "key.h", "vendor"] {
        println!("cargo:rerun-if-changed=../../include/rinzu/{resource}");
    }
    if std::env::var("CARGO_CFG_TARGET_FAMILY").as_deref() == Ok("unix") {
        cc::Build::new()
            .file("../../include/rinzu/json.c")
            .include("../../include")
            .flag("-std=c11")
            .opt_level(3)
            .warnings_into_errors(true)
            .compile("rinzu_json");
        cc::Build::new()
            .file("src/display.c")
            .file("src/stdio.c")
            .file("src/foreign_queue.c")
            .file("src/native_diagnostics.c")
            .file("src/ready_work.c")
            .file("src/standalone_interrupt.c")
            .include("../../include")
            .flag("-std=c11")
            .warnings_into_errors(true)
            .compile("rinzu_foreign_queue");
    }
    println!("cargo:rerun-if-changed=src/callback_entry.c");
    println!("cargo:rerun-if-changed=src/callback_template.S");
    println!("cargo:rerun-if-changed=../../include/rinzu/callback_entry.h");
    println!("cargo:rerun-if-changed=src/callback_return.c");
    println!("cargo:rerun-if-changed=src/owner_dispatcher.c");
    println!("cargo:rerun-if-changed=../../include/rinzu/owner_dispatcher.h");
    println!("cargo:rerun-if-changed=../../include/rinzu/callback_return.h");
    println!("cargo:rerun-if-changed=../../include/rinzu/callback_consumption.h");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos")
        && std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("aarch64")
    {
        let mut callback = cc::Build::new();
        if std::env::var_os("CARGO_FEATURE_TEST_DISPATCHER_PROBE").is_some() {
            callback.define("RINZU_DISPATCHER_TESTING", None);
        }
        callback
            .file("src/callback_entry.c")
            .file("src/callback_template.S")
            .file("src/callback_return.c")
            .file("src/owner_dispatcher.c")
            .include("../../include")
            .flag("-std=c11")
            .warnings_into_errors(true)
            .compile("rinzu_callback_entry");
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
    }
}
