#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn source_region_preserves_actual_cli_jit_aot_await_worker_and_trace_spans() {
    use rinzu_compiler::{FileId, SourceFile};
    use std::{fs, process::Command};
    let root = std::env::temp_dir().join(format!("rinzu-source-region-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let entry = root.join("位置.rinzu");
    let text = include_str!("fixtures/source_region.rinzu");
    fs::write(&entry, text).unwrap();
    let file = SourceFile::new(FileId(0), entry.to_string_lossy().as_ref(), text);
    let at = |token: &str| {
        let (line, column) = file.line_column(text.find(token).unwrap());
        format!("{line}:{column}")
    };
    let leaf = at("CInt8(301)");
    let restored = at("CInt8(302)");
    let later = at("CInt8(303)");
    let expected = format!(
        "nested:{leaf}\nrestored:{restored}\nawait-one:{later}\nawait-two:{later}\nworker:{leaf}\ndone:7\n"
    );
    for stress in ["0", "1"] {
        for trace in ["0", "1"] {
            let run = Command::new(frozen_cli::path())
                .arg("run")
                .arg(&entry)
                .env("RINZU_GC_STRESS", stress)
                .env("RINZU_ERROR_TRACE", trace)
                .output()
                .unwrap();
            assert!(
                run.status.success(),
                "JIT stress={stress} trace={trace}: {run:?}"
            );
            assert_eq!(run.stdout, expected.as_bytes());
            assert!(
                String::from_utf8_lossy(&run.stderr).contains("compiled JIT continuation runtime")
            );
            let verify = Command::new(frozen_cli::path())
                .arg("verify-aot")
                .arg(&entry)
                .env("RINZU_GC_STRESS", stress)
                .env("RINZU_ERROR_TRACE", trace)
                .output()
                .unwrap();
            assert!(
                verify.status.success(),
                "AOT stress={stress} trace={trace}: {verify:?}"
            );
            assert!(String::from_utf8_lossy(&verify.stdout).contains("outcome=success"));
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn compiled_source_traces_preserve_frontend_ranges_without_source_files() {
    use rinzu_compiler::{FileId, SourceFile};
    use serde_json::Value;
    use std::{fs, path::Path, process::Command};

    fn trace_packet(value: &Value) -> Option<&Value> {
        if value.get("engine").and_then(Value::as_str) == Some("compiled-jit") {
            return value.get("errorTraces");
        }
        match value {
            Value::Object(fields) => fields.values().find_map(trace_packet),
            Value::Array(values) => values.iter().find_map(trace_packet),
            _ => None,
        }
    }
    fn check(packet: &Value, file: &SourceFile, path: &Path, text: &str) {
        let frames = packet["caught"][0][0]
            .as_array()
            .expect("one caught primary trace");
        assert_eq!(frames.len(), 3, "{packet}");
        for (frame, (function, token, semicolon)) in frames.iter().zip([
            ("leaf", "throw Problem(message:\"original\");", true),
            ("relay", "leaf();", false),
            ("main", "relay();", false),
        ]) {
            let start = text.find(token).unwrap();
            let end = start
                + if semicolon {
                    token.len()
                } else {
                    token.len() - 1
                };
            let (line, column) = file.line_column(start);
            assert_eq!(frame["function"], function);
            assert_eq!(frame["source"]["name"], path.to_string_lossy().as_ref());
            assert_eq!(frame["source"]["startByte"], start);
            assert_eq!(frame["source"]["endByte"], end);
            assert_eq!(frame["source"]["line"], line);
            assert_eq!(frame["source"]["column"], column);
        }
    }
    let root = std::env::temp_dir().join(format!("rinzu-source-traces-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let entry = root.join("位置.rinzu");
    // An ordinary native declaration forces the compiled C JIT. Pure source
    // async and closed stdio/timer services can use the Rust reference engine.
    let text = "import rinzu.io;\nerror Problem {}\nfn leaf(){throw Problem(message:\"original\");}\nfn relay(){leaf();}\nasync fn main(){try{let mark=rinzu.io.monotonicMillis();relay();}catch problems{}}\n";
    fs::write(&entry, text).unwrap();
    let file = SourceFile::new(FileId(0), entry.to_string_lossy().as_ref(), text);
    for stress in ["0", "1"] {
        let report = root.join(format!("report-{stress}.json"));
        let run = Command::new(frozen_cli::path())
            .arg("run")
            .arg(&entry)
            .arg("--diagnostic-report")
            .arg(&report)
            .env("RINZU_GC_STRESS", stress)
            .env("RINZU_ERROR_TRACE", "1")
            .output()
            .unwrap();
        assert!(run.status.success(), "{run:?}");
        assert!(String::from_utf8_lossy(&run.stderr).contains("compiled JIT continuation runtime"));
        let value: Value = serde_json::from_slice(&fs::read(&report).unwrap()).unwrap();
        check(
            trace_packet(&value).expect("compiled diagnostic trace"),
            &file,
            &entry,
            text,
        );
    }
    let binary = root.join("standalone");
    let build = Command::new(frozen_cli::path())
        .arg("build")
        .arg(&entry)
        .arg("--output")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(build.status.success(), "{build:?}");
    fs::remove_file(&entry).unwrap();
    let run = Command::new(&binary)
        .env("RINZU_ERROR_TRACE", "1")
        .env("RINZU_GC_STRESS", "1")
        .output()
        .unwrap();
    assert!(run.status.success(), "{run:?}");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let packet: Value = serde_json::from_str(
        combined
            .lines()
            .find_map(|line| line.strip_prefix("RINZU_ERROR_TRACES="))
            .expect("AOT trace packet"),
    )
    .unwrap();
    check(&packet, &file, &entry, text);
    fs::remove_dir_all(root).unwrap();
}
