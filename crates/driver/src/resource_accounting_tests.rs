#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use rinzu_runtime::resource_accounting::{self as accounting, Kind};
mod fixture {
    include!("../tests/fixtures/native_callback_return_sdk/setup.rs");
}

#[test]
fn resource_vectors_quiescent_gc_workers_and_multiple_runs_preserve_retained_growth() {
    const CHILD: &str = "RINZU_M0_ACCOUNTING_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "resource_accounting_tests::resource_vectors_quiescent_gc_workers_and_multiple_runs_preserve_retained_growth", "--nocapture"])
            .env(CHILD, "1").output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        print!("{}", String::from_utf8_lossy(&output.stdout));
        return;
    }
    let root =
        std::env::temp_dir().join(format!("rinzu-m0-resource-vectors-{}", std::process::id()));
    fixture::create(&root);
    let app = root.join("app");
    let (_, initial) = rinzu_compiler::check_package(&app).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &initial.sdk_packages,
        &app,
        &initial.native_inputs,
    )
    .unwrap();
    accounting::enable();
    let mut records = Vec::new();
    for (engine, source, owners) in [
        (
            "baseline",
            "import returns;fn main(){let result=returns.exercise();}",
            1,
        ),
        (
            "source",
            "import returns;async fn main(){let result=returns.exercise();}",
            1,
        ),
        (
            "source-worker",
            "import returns;async fn main(){let first=spawn returns.exercise();let second=spawn returns.exercise();let a=await first;let b=await second;}",
            2,
        ),
    ] {
        std::fs::write(app.join("src/main.rinzu"), source).unwrap();
        let (manifest, artifact) = rinzu_compiler::check_package(&app).unwrap();
        for iteration in 0..3 {
            let before = accounting::snapshot();
            let evidence = if engine == "baseline" {
                let run = rinzu_codegen::run_baseline_jit_with_package_bindings(
                    &artifact.ir,
                    "main",
                    rinzu_codegen::BaselineOptions {
                        collect_every_allocation: true,
                        ..Default::default()
                    },
                    None,
                    Some(built.load_source_packages().unwrap()),
                )
                .unwrap();
                assert!(run.result.is_ok() && run.heap_objects == 0);
                run.native_calls
            } else {
                let run = rinzu_codegen::run_source_async_report(
                    &artifact.ir,
                    "main",
                    rinzu_codegen::SourceAsyncOptions {
                        source_packages: Some(built.load_source_packages().unwrap()),
                        ..Default::default()
                    },
                )
                .unwrap();
                assert!(run.resources_returned);
                run.native_calls
            };
            assert!(evidence.is_baseline());
            assert_eq!(evidence.callback_returns.bytes, 266 * owners);
            assert_eq!(evidence.callback_returns.normal, 24 * owners);
            assert_eq!(evidence.callback_returns.fallback, 12 * owners);
            let after = accounting::snapshot();
            let quantity = |rows: &[accounting::Category], id| {
                rows.iter().find(|row| row.id == id).unwrap().live.unwrap()
            };
            assert_eq!(
                quantity(&after, "callback_normal") - quantity(&before, "callback_normal"),
                (184 * owners) as u64
            );
            assert_eq!(
                quantity(&after, "callback_fallback") - quantity(&before, "callback_fallback"),
                (82 * owners) as u64
            );
            assert_eq!(
                quantity(&after, "callback_entries") - quantity(&before, "callback_entries"),
                (13 * owners) as u64
            );
            for kind in Kind::ALL {
                if matches!(kind, Kind::NativeCopy) {
                    continue;
                }
                let counter = accounting::counter(kind);
                assert_eq!(
                    counter.live, 0,
                    "{engine}/{iteration}: {kind:?}: {counter:?}"
                );
                assert_eq!(counter.allocated, counter.released);
            }
            let copies = after
                .iter()
                .find(|row| row.id == "native_copy")
                .unwrap()
                .allocated
                .unwrap()
                - before
                    .iter()
                    .find(|row| row.id == "native_copy")
                    .unwrap()
                    .allocated
                    .unwrap();
            assert_eq!(copies, evidence.copied_bytes as u64);
            records.push(serde_json::json!({"engine": engine, "iteration": iteration, "owners": owners,
                "native_prepared": evidence.prepared, "copy_bytes": copies, "before": before, "after": after}));
        }
        if engine != "baseline" {
            let output = root.join(format!("{engine}.jsonl"));
            let run = crate::compiled_async::run_with_resource_report(
                &manifest,
                &artifact,
                Some(&rinzu_runtime::resource_report::Config {
                    path: output.clone(),
                    interval_ms: 1,
                }),
            )
            .unwrap();
            assert_eq!(run.exit_code, 0);
            let rows: Vec<serde_json::Value> = std::fs::read_to_string(output)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            let end = rows.last().unwrap();
            assert_eq!(end["phase"], "end");
            let category = |id: &str| {
                end["categories"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|row| row["id"] == id)
                    .unwrap()
            };
            assert_eq!(category("callback_normal")["live"], 184 * owners);
            assert_eq!(category("callback_fallback")["live"], 82 * owners);
            for id in [
                "managed",
                "managed_objects",
                "task",
                "future",
                "pending_operation",
                "transport",
                "transport_in_flight",
                "native_calls",
            ] {
                assert_eq!(category(id)["live"], 0, "{engine}: {id}");
                assert_eq!(category(id)["allocated"], category(id)["released"]);
            }
            records.push(serde_json::json!({"engine":format!("compiled-{engine}"), "owners": owners, "start": rows.first(), "end":end, "snapshot_count":rows.len()}));
        }
    }
    println!(
        "M0_RESOURCE_VECTOR={}",
        serde_json::to_string(&records).unwrap()
    );
    accounting::disable();
    std::fs::remove_dir_all(root).unwrap();
}
