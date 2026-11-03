use rinzu_compiler::check_package;

#[allow(dead_code)]
mod userdata {
    include!("../../driver/tests/fixtures/native_userdata_sdk/setup.rs");
}
#[allow(dead_code)]
mod callback {
    include!("../../driver/tests/fixtures/native_callback_sdk/setup.rs");
}
#[allow(dead_code)]
mod writeback {
    include!("../../driver/tests/fixtures/native_record_writeback_sdk/setup.rs");
}
mod supplant_capture {
    include!("../../driver/tests/fixtures/native_supplant_capture_sdk/setup.rs");
}

#[test]
fn migrated_context_and_callback_fixtures_use_ordinary_records() {
    for (name, create) in [
        ("context", userdata::create as fn(&std::path::Path)),
        ("callback", callback::create),
        ("writeback", writeback::create),
        ("writeback-callback", writeback::create_sdk_callbacks),
        ("supplant-capture", supplant_capture::create),
    ] {
        let root = std::env::temp_dir().join(format!(
            "rinzu-check-native-fixture-{name}-{}",
            std::process::id()
        ));
        create(&root);
        check_package(&root.join("app")).unwrap_or_else(|errors| panic!("{name}: {errors:#?}"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
