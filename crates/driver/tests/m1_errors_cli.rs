#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::process::Command;
#[path = "fixtures/checked_entry.rs"]
mod checked_entry;
#[path = "fixtures/native_opaque_sdk/setup.rs"]
mod fixture;
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
#[test]
fn source_sdk_maps_native_status_and_preserves_caught_metadata() {
    let root = std::env::temp_dir().join(format!("rinzu-m1-sdk-errors-{}", std::process::id()));
    fixture::create(&root);
    let sdk = root.join("objects/src/api.rinzu");
    let mut source = std::fs::read_to_string(&sdk).unwrap();
    source.push_str(
        r#"
fn statusValue(status: Int) -> Int { return nativeStatus(status,"objects status"); }
fn nullValue() -> Int {
    try { let raw = absent(); return 99; }
    catch [caught: Error, ...caughtRest] { throw errorWithContext(caught,"objects.absent"); }
}
fn fallbackLength(value: Bytes(4)) -> CSize fallback 4 {
    let metadata = errorWithContext(error(1000,"callback"),"native callback");
    let failed = nativeStatus(-1,"callback status");
    let invalid = CInt8(300);
    return byteLength(value);
}
fn callbackFallback(seed: Int) -> Int {
    let object = create(seed);
    let found = lookup(object,fallbackLength);
    let result = read(found);
    release(object);
    return result;
}
export callbackFallback;
export statusValue;
export nullValue;
"#,
    );
    std::fs::write(sdk, source).unwrap();
    let app = root.join("app");
    let manifest = std::fs::read_to_string(app.join("rinzu.toml"))
        .unwrap()
        .replace("0.2", "0.5");
    std::fs::write(app.join("rinzu.toml"), manifest).unwrap();
    std::fs::write(app.join("src/main.rinzu"),checked_entry::source(r#"
import objects;
import rinzu.stdio;
async fn main() {
    try {let unexpected=objects.nullValue();panic("missingNull");}
    catch [problem:Error,...rest] {
        rinzu.stdio.writeLine(errorCode(problem));rinzu.stdio.writeLine(errorContext(problem));
        switch errorSourceStart(problem) {case Option.Some(offset):{rinzu.stdio.writeLine(offset>=0);}case Option.None:{rinzu.stdio.writeLine(false);}}
    }
    rinzu.stdio.writeLine(objects.callbackFallback(42));
    try {let unexpectedStatus=objects.statusValue(objects.measure(5));panic("missingStatus");}
    catch [problemStatus:Error,...statusRest] {
        rinzu.stdio.writeLine(errorCode(problemStatus));rinzu.stdio.writeLine(errorNativeStatus(problemStatus));rinzu.stdio.writeLine(errorContext(problemStatus));
    }
}
"#)).unwrap();
    let expected =
        b"2\nobjects.absent\ntrue\n42\n100\nOption<Int>.Some(value: 5)\nobjects status\n";
    for command in ["check", "run", "build", "verify-aot"] {
        let output = Command::new(frozen_cli::path())
            .arg(command)
            .arg(&app)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{command}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        if command == "run" {
            assert_eq!(output.stdout, expected);
        }
    }
    let output = Command::new(app.join("build/opaque_app")).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, expected);
    std::fs::remove_dir_all(root).unwrap();
}
