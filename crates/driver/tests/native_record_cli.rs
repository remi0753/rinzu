#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::path::Path;
use std::process::{Command, Output};

#[path = "fixtures/native_record_sdk/setup.rs"]
mod records;

#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

fn cli(app: &Path, command: &str) -> Output {
    Command::new(frozen_cli::path())
        .arg(command)
        .arg(app)
        .output()
        .unwrap()
}

fn success(output: &Output) {
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
}

fn numbers(stdout: &str, prefix: &str) -> Vec<usize> {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix(prefix))
        .unwrap_or_else(|| panic!("missing {prefix}: {stdout}"))
        .split(',')
        .map(|value| value.parse().unwrap())
        .collect()
}

fn baseline(output: &Output, calls: usize) {
    let stdout = String::from_utf8_lossy(&output.stderr);
    if calls != 0 || stdout.contains("RINZU_NATIVE_CALLS=") {
        let native = numbers(&stdout, "RINZU_NATIVE_CALLS=");
        assert_eq!(&native[..6], &[calls; 6], "{stdout}");
        assert_eq!(&native[8..], &[0, 0, 0], "{stdout}");
        let callbacks = numbers(&stdout, "RINZU_NATIVE_ENTRIES=");
        assert_eq!(&callbacks[4..], &[0, 0], "{stdout}");
    }
}

#[test]
fn shared_record_constructor_capture_and_display_agree_in_jit_and_aot() {
    let root = std::env::temp_dir().join(format!("rinzu-shared-record-cli-{}", std::process::id()));
    records::create(&root);
    let app = root.join("app");
    std::fs::write(
        app.join("src/main.rinzu"),
        r#"
        import records;
        import rinzu.stdio;
        class Holder{const value:records.Packet}
        fn create(value:Int)->records.Packet{
            return records.Packet(a:1,b:2,c:3,d:value,e:5,f:6,g:7,h:8,i:0.0,j:1.0,k:true);
        }
        fn main(){
            try{
            let point=create(4);
            let holder=Holder(value:point);
            let read=fn()->records.Packet{return holder.value;};
            let selected=read();
            var index=0;
            while index<40{let transient=create(index);index=index+1;}
            rinzu.stdio.writeLine(selected);
            rinzu.stdio.writeLine(f"{point.d},{selected.k}");
            }catch errors{panic("unexpected native record failure");}
        }
        "#,
    )
    .unwrap();
    let jit = cli(&app, "run");
    success(&jit);
    let stdout = String::from_utf8_lossy(&jit.stdout);
    assert!(
        stdout.contains("a: 1") && stdout.contains("k: true"),
        "{stdout}"
    );
    assert!(stdout.ends_with("4,true\n"), "{stdout}");
    success(&cli(&app, "build"));
    let aot = Command::new(app.join("build/record_app"))
        .env("RINZU_GC_STRESS", "1")
        .output()
        .unwrap();
    success(&aot);
    assert_eq!(aot.stdout, jit.stdout);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn record_cli_compares_common_sdk_and_compiled_sync_values() {
    {
        let (module, name, calls) = ("records", "record_app", 102);
        let root =
            std::env::temp_dir().join(format!("rinzu-cli-{module}-common-{}", std::process::id()));
        records::create(&root);
        let app = root.join("app");
        let run = cli(&app, "run");
        success(&run);
        assert!(String::from_utf8_lossy(&run.stderr).contains("compiled JIT continuation runtime"));
        assert!(
            app.join(".rinzu/generated/source-packages-v2/manifest.json")
                .is_file()
        );
        success(&cli(&app, "build"));
        let aot = Command::new(app.join(format!("build/{name}")))
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .output()
            .unwrap();
        success(&aot);
        baseline(&aot, calls);
        success(&cli(&app, "verify-aot"));
        let body = "let result=records.exercise();if result.caught!=2 {panic(\"wrong\");}";
        std::fs::write(
            app.join("src/main.rinzu"),
            format!("import {module}; fn main() {{{body}}}"),
        )
        .unwrap();
        let jit = cli(&app, "run");
        success(&jit);
        assert!(String::from_utf8_lossy(&jit.stderr).contains("baseline JIT"));
        success(&cli(&app, "verify-aot"));
        std::fs::remove_dir_all(root).unwrap();
    }
}

fn caught_case(
    module: &str,
    name: &str,
    expression: &str,
    cause: i32,
    failure: &str,
    calls: usize,
) {
    let root = std::env::temp_dir().join(format!(
        "rinzu-cli-{module}-uncaught-{calls}-{cause}-{}",
        std::process::id()
    ));
    records::create(&root);
    let app = root.join("app");
    for (entry, worker) in [
        ("fn main()", false),
        ("async fn main()", false),
        ("async fn main()", true),
    ] {
        let call = if worker {
            format!("await spawn {expression}")
        } else {
            expression.to_string()
        };
        std::fs::write(
            app.join("src/main.rinzu"),
            format!("import {module}; {entry} {{let failed={call};}}"),
        )
        .unwrap();
        let rejected = cli(&app, "check");
        assert!(!rejected.status.success());
        let diagnostic = String::from_utf8_lossy(&rejected.stderr);
        assert!(
            diagnostic.contains(if worker { "RZ0402" } else { "RZ0413" }),
            "{diagnostic}"
        );
        if worker {
            assert!(diagnostic.contains("not Transferable"), "{diagnostic}");
        }
        let message = if cause == 8 {
            "native value is NULL"
        } else {
            "numeric conversion is out of range or loses required integer precision"
        };
        let handled = format!(
            r#"fn handled()->Int {{
            try {{let failed={expression};}}
            catch [problem:Error,...rest] {{if problem.message!="{message}"||rinzu.list.length(rest)!=0{{panic("wrongError");}}return 1;}}
            return 999;
        }}"#
        );
        let call = if worker {
            "await spawn handled()"
        } else {
            "handled()"
        };
        std::fs::write(app.join("src/main.rinzu"), format!("import {module};{handled}{entry}{{let report={call};if report!=1{{panic(\"missingError\");}}}}" )).unwrap();
        success(&cli(&app, "run"));
        success(&cli(&app, "build"));
        let aot = Command::new(app.join(format!("build/{name}")))
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .output()
            .unwrap();
        success(&aot);
        let report = String::from_utf8_lossy(&aot.stderr);
        assert!(
            report.contains(&format!("RINZU_CAUGHT_ERRORS={cause}@")),
            "{report}"
        );
        assert!(
            report.contains("RINZU_FAILURE=none")
                && !report.contains(&format!("RINZU_FAILURE={failure}")),
            "{report}"
        );
        baseline(&aot, calls);
        success(&cli(&app, "verify-aot"));
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn record_cli_caught_null() {
    caught_case(
        "records",
        "record_app",
        "records.failure(0)",
        8,
        "null-native-value",
        1,
    );
}

#[test]
fn record_cli_caught_bool() {
    caught_case(
        "records",
        "record_app",
        "records.failure(2)",
        5,
        "conversion",
        1,
    );
}
