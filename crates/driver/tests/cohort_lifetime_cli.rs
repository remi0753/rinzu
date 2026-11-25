#![cfg(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "x86_64", target_env = "gnu")
))]
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
#[allow(dead_code)]
#[path = "support/source_platform.rs"]
mod platform;
#[allow(dead_code)]
#[path = "support/scratch.rs"]
mod scratch;
#[path = "support/target.rs"]
mod target;
use rinzu_runtime_model::native_lifetime::{
    BatchSchedule, Binding, CohortLayout, CohortMode, MixedBatchSchedule, replay_cohort,
    replay_combination, replay_extended,
};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
#[path = "../../../tests/support/cohort.rs"]
mod adversarial;
#[path = "../../../tests/support/batched.rs"]
mod batched;
#[path = "../../../tests/support/combination.rs"]
mod combination_adversarial;
#[path = "../../../tests/support/mixed_batches.rs"]
mod mixed_batches;
#[path = "../../../tests/support/unordered.rs"]
mod unordered;
fn stage(root: &Path) {
    for dir in ["app/src", "cohort/src"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    std::fs::write(root.join("app/rinzu.toml"),"[package]\nname=\"cohort_app\"\nlanguage=\"0.5\"\n[dependencies]\ncohort={path=\"../cohort\"}\n").unwrap();
    std::fs::write(root.join("cohort/rinzu.toml"),"[package]\nname=\"cohort\"\nlanguage=\"0.5\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n[native]\nsources=[\"api.c\"]\nheaders=[\"sdk_observer.h\",\"api.h\",\"completion.h\"]\n").unwrap();
    let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cohort_sdk");
    for file in ["api.c", "api.h", "observer.h", "sdk_observer.h", "relay.c"] {
        std::fs::copy(from.join(file), root.join("cohort").join(file)).unwrap();
    }
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include/rinzu/native_completion.h"),
        root.join("cohort/completion.h"),
    )
    .unwrap();
    std::fs::copy(from.join("api.rinzu"), root.join("cohort/src/api.rinzu")).unwrap();
    std::fs::copy(from.join("main.rinzu"), root.join("app/src/main.rinzu")).unwrap();
}
#[test]
fn actual_c_native_call_hands_off_to_completion_and_wake() {
    run(false, false);
}
#[test]
fn early_publication_and_polling_join_share_multiple_calls() {
    run(true, false);
}
#[test]
fn reused_carrier_preserves_callback_publication_and_joined_polling() {
    run(true, true);
}
#[test]
fn recovers_native_calls_and_polling_in_every_order() {
    let scratch = scratch::Scratch::new("rinzu-unordered-cli");
    let root = scratch.path();
    let original = build(root, "unordered.rinzu");
    for sanitized in [false, true] {
        let binary = instrument(root, &original, sanitized);
        for (id, order) in unordered::PERMUTATIONS.into_iter().enumerate() {
            let mut run = Command::new(&binary);
            completion_environment(&mut run, true);
            run.env("COHORT_POLLING", "1")
                .env("COHORT_ORDER", id.to_string());
            let events = run_events(&mut run, false);
            println!("UNORDERED C sanitized={sanitized}");
            unordered::check(Binding::CJoined, false, order, &events);
        }
    }
}

#[test]
fn shared_native_results_reach_separately_parked_awaiters() {
    let scratch = scratch::Scratch::new("rinzu-dependent-cli");
    let root = scratch.path();
    let original = build(root, "dependent.rinzu");
    for sanitized in [false, true] {
        let binary = instrument(root, &original, sanitized);
        for (id, order) in unordered::PERMUTATIONS.into_iter().enumerate() {
            let mut run = Command::new(&binary);
            completion_environment(&mut run, true);
            run.env("COHORT_POLLING", "1")
                .env("COHORT_ORDER", id.to_string());
            let output = run_cohort(&mut run, false);
            let stdout = std::str::from_utf8(&output.stdout).unwrap();
            let diagnostics = std::str::from_utf8(&output.stderr).unwrap();
            unordered::check(Binding::CJoined, false, order, &lifetime_events(stdout));
            check_shared_awaits(stdout, diagnostics, order);
            println!("DEPENDENT C sanitized={sanitized} order={order:?}");
        }
    }
}

#[test]
fn managed_capture_cycle_keeps_native_results_with_gc_stress() {
    let scratch = scratch::Scratch::new("rinzu-cohort-capture-graph");
    let root = scratch.path();
    stage(root);
    std::fs::write(
        root.join("app/src/main.rinzu"),
        format!(
            "{}\n{}",
            include_str!("fixtures/cohort_sdk/managed_graph.rinzu"),
            include_str!("../../codegen/tests/fixtures/native-capture-graph.rinzu")
        ),
    )
    .unwrap();
    let original = build_staged(root);
    for sanitized in [false, true] {
        let binary = instrument(root, &original, sanitized);
        for id in [1, 3] {
            let order = unordered::PERMUTATIONS[id];
            let mut run = Command::new(&binary);
            completion_environment(&mut run, true);
            run.env("COHORT_POLLING", "1")
                .env("COHORT_ORDER", id.to_string())
                .env("RINZU_GC_STRESS", "1");
            let output = run_cohort(&mut run, false);
            let stdout = std::str::from_utf8(&output.stdout).unwrap();
            let diagnostics = std::str::from_utf8(&output.stderr).unwrap();
            unordered::check(Binding::CJoined, false, order, &lifetime_events(stdout));
            check_shared_awaits(stdout, diagnostics, order);
            println!("MANAGED NATIVE CAPTURE C sanitized={sanitized} order={order:?}");
        }
    }
}

fn check_shared_awaits(stdout: &str, diagnostics: &str, order: [usize; 3]) {
    let rows: Vec<_> = stdout.lines().collect();
    let receipt = |generation| {
        let suffix = format!("{generation},0");
        rows.iter()
            .find_map(|row| {
                let (future, fields) = row.strip_prefix("future_bind,")?.split_once(',')?;
                (fields == suffix).then(|| future.parse::<u64>().unwrap())
            })
            .unwrap()
    };
    let native = [receipt(100), receipt(101)];
    let parked: Vec<_> = rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| {
            let fields = row.strip_prefix("await_park,")?;
            let (data, _) = fields.rsplit_once(',').unwrap();
            let (scope, future) = data.split_once(',').unwrap();
            Some((
                index,
                scope.parse::<u64>().unwrap(),
                future.parse::<u64>().unwrap(),
            ))
        })
        .collect();
    let [
        (forward_at, forward, first),
        (reverse_at, reverse, second),
        (shared_at, sharing, another),
    ] = parked.as_slice()
    else {
        panic!("expected two first waits and one overlapping second wait\n{stdout}");
    };
    assert_eq!([*first, *second], native, "{stdout}");
    assert_ne!(forward, reverse, "{stdout}");
    let first_published = order.into_iter().find(|&slot| slot != 2).unwrap();
    let later = 1 - first_published;
    assert_eq!(*sharing, [*forward, *reverse][first_published], "{stdout}");
    assert_eq!(*another, native[later], "{stdout}");
    let publish_at = |slot| {
        rows.iter()
            .position(|row| row.starts_with(&format!("publish_begin,{},", 100 + slot)))
            .unwrap()
    };
    for position in [*forward_at, *reverse_at] {
        assert!(position < publish_at(first_published), "{stdout}");
    }
    assert!(
        publish_at(first_published) < *shared_at && *shared_at < publish_at(later),
        "{stdout}"
    );

    let report = |prefix| {
        diagnostics
            .lines()
            .chain(stdout.lines())
            .find_map(|row| row.strip_prefix(prefix))
            .unwrap_or_else(|| panic!("missing {prefix}\n{diagnostics}\n{stdout}"))
    };
    let values = report("RINZU_AWAITED_TASK_VALUES=");
    for (scope, expected) in [(*forward, [0, 1]), (*reverse, [1, 0])] {
        let prefix = format!("{scope}:");
        let observed: Vec<_> = values
            .split(',')
            .filter_map(|entry| entry.strip_prefix(&prefix))
            .map(|entry| {
                let (identity, value) = entry.split_once(':').unwrap();
                let (_, bits) = value.split_once(':').unwrap();
                (
                    identity.parse::<u64>().unwrap(),
                    u64::from_str_radix(bits, 16).unwrap(),
                )
            })
            .collect();
        assert_eq!(
            observed,
            expected.map(|slot| (native[slot], [7183, 29][slot])),
            "{stdout}"
        );
    }
    let accounting: Vec<usize> = report("RINZU_CONTINUATIONS=")
        .split(',')
        .map(|value| value.parse().unwrap())
        .collect();
    assert!(
        matches!(accounting[..], [0, 3, 0, peak] if peak > 0),
        "{stdout}"
    );
}

#[test]
fn batched_calls_reuse_storage_and_coalesce_notifications() {
    let scratch = scratch::Scratch::new("rinzu-batched-cli");
    let root = scratch.path();
    let original = build(root, "batched.rinzu");
    for sanitized in [false, true] {
        let binary = instrument(root, &original, sanitized);
        for schedule in [BatchSchedule::Staggered, BatchSchedule::Together] {
            for leader in [1, 2] {
                for controlled in [false, true] {
                    let mut run = Command::new(&binary);
                    completion_environment(&mut run, true);
                    run.env("COHORT_REUSE", "1")
                        .env("COHORT_POLLING", "1")
                        .env("COHORT_BATCH_FIRST", leader.to_string());
                    if schedule == BatchSchedule::Staggered {
                        run.env("COHORT_STAGGERED", "1");
                    }
                    if controlled {
                        run.env("COHORT_CONTROLLED", "1");
                    }
                    let events = run_events(&mut run, controlled);
                    println!("BATCH C sanitized={sanitized}");
                    batched::check(
                        Binding::CJoined,
                        schedule,
                        controlled,
                        100 + leader,
                        &events,
                    );
                }
            }
        }
    }
}

#[test]
fn mixed_abis_share_consecutive_notification_batches() {
    let scratch = scratch::Scratch::new("rinzu-mixed-batches-cli");
    let root = scratch.path();
    let original = build(root, "mixed_batches.rinzu");
    for sanitized in [false, true] {
        let binary = instrument(root, &original, sanitized);
        for schedule in [
            MixedBatchSchedule::Staggered,
            MixedBatchSchedule::Together,
            MixedBatchSchedule::LateArrival,
        ] {
            for (first, second) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                if schedule == MixedBatchSchedule::LateArrival && (first, second) != (1, 1) {
                    continue;
                }
                for controlled in [false, true] {
                    let mut run = Command::new(&binary);
                    completion_environment(&mut run, true);
                    run.env("COHORT_POLLING", "1")
                        .env("COHORT_SERIES_FIRST", format!("{first}{second}"));
                    if schedule != MixedBatchSchedule::Together {
                        run.env("COHORT_STAGGERED", "1");
                    }
                    if schedule == MixedBatchSchedule::LateArrival {
                        run.env("COHORT_LATE_ARRIVAL", "1");
                    }
                    if controlled {
                        run.env("COHORT_CONTROLLED", "1");
                    }
                    let reverse = first != second;
                    if reverse {
                        run.env("COHORT_RELEASE_REVERSE", "1");
                    }
                    let events = run_events(&mut run, controlled);
                    println!("MIXED BATCH C sanitized={sanitized}");
                    mixed_batches::check(
                        Binding::CJoined,
                        schedule,
                        controlled,
                        [100 + first, 102 + second],
                        reverse,
                        &events,
                    );
                }
            }
        }
    }
}

fn build(root: &Path, source: &str) -> PathBuf {
    stage(root);
    if source != "main.rinzu" {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cohort_sdk");
        std::fs::copy(fixture.join(source), root.join("app/src/main.rinzu")).unwrap();
    }
    build_staged(root)
}

fn build_staged(root: &Path) -> PathBuf {
    let app = root.join("app");
    platform::success(&platform::bounded_tool(
        Command::new(frozen_cli::path()).arg("build").arg(&app),
    ));
    target::build_directory(&app).join("cohort_app")
}

fn instrument(root: &Path, original: &Path, sanitized: bool) -> PathBuf {
    let binary = original.with_file_name(if sanitized {
        "cohort-san"
    } else {
        "cohort-plain"
    });
    let mut compiler = platform::clang();
    compiler
        .args([
            "-std=c11",
            "-O1",
            "-g",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-Wno-unused-label",
            "-pthread",
            "-include",
        ])
        .arg(root.join("cohort/observer.h"))
        .arg(original.with_extension("o"))
        .arg(original.with_extension("runtime.c"))
        .arg(root.join("cohort/relay.c"));
    compiler.arg(if cfg!(target_os = "linux") {
        "-rdynamic"
    } else {
        "-Wl,-export_dynamic"
    });
    if sanitized {
        compiler.args(["-fsanitize=address,undefined", "-fno-sanitize-recover=all"]);
    }
    platform::link_runtime(&mut compiler);
    if cfg!(target_os = "linux") {
        compiler.args(["-lcrypto", "-ldl"]);
    }
    platform::success(&platform::bounded_tool(compiler.arg("-o").arg(&binary)));
    for extension in ["source-packages", "resources"] {
        if original.with_extension(extension).exists() {
            std::os::unix::fs::symlink(
                original.with_extension(extension),
                binary.with_extension(extension),
            )
            .unwrap();
        }
    }
    binary
}

fn completion_environment(run: &mut Command, joined: bool) {
    run.env("RINZU_TRANSPORT_EVIDENCE", "1")
        .env(
            "RINZU_NATIVE_COMPLETION_DIRECT",
            if joined { "0" } else { "1" },
        )
        .env("RINZU_NATIVE_COMPLETION_NOTIFY", "1")
        .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
        .env("UBSAN_OPTIONS", "halt_on_error=1");
}

fn run(combinations: bool, extended: bool) {
    let scratch = scratch::Scratch::new("rinzu-cohort-cli");
    let root = scratch.path();
    let original = build(
        root,
        if extended {
            "extended.rinzu"
        } else {
            "main.rinzu"
        },
    );
    for sanitized in [false, true] {
        let binary = instrument(root, &original, sanitized);
        for layout in [CohortLayout::Carriers, CohortLayout::Reuse] {
            for controlled in [false, true] {
                let joined = combinations && layout == CohortLayout::Reuse;
                let actual_layout = if extended {
                    CohortLayout::Reuse
                } else if combinations {
                    CohortLayout::Carriers
                } else {
                    layout
                };
                let mut run = Command::new(&binary);
                if extended {
                    run.env("COHORT_EXTENDED", "1");
                }
                if combinations {
                    run.env("COHORT_EARLY", "1");
                }
                if joined {
                    run.env("COHORT_POLLING", "1");
                }
                completion_environment(&mut run, joined);
                if actual_layout == CohortLayout::Reuse {
                    run.env("COHORT_REUSE", "1");
                }
                if controlled {
                    run.env("COHORT_CONTROLLED", "1");
                }
                let events = run_events(&mut run, controlled);
                let binding = if joined {
                    Binding::CJoined
                } else {
                    Binding::CDirect
                };
                let r = (if extended {
                    replay_extended(
                        binding,
                        CohortMode::Early,
                        controlled,
                        joined,
                        &events,
                        100_000,
                    )
                } else if combinations {
                    replay_combination(binding, CohortMode::Early, controlled, &events, 100_000)
                } else {
                    replay_cohort(binding, layout, controlled, &events, 100_000)
                })
                .unwrap_or_else(|e| panic!("{e}\n{}", events.join("\n")));
                let rejected = if extended {
                    combination_adversarial::reject_extended(
                        binding,
                        CohortMode::Early,
                        controlled,
                        joined,
                        &events,
                    )
                } else if combinations {
                    combination_adversarial::reject_corruptions(
                        binding,
                        CohortMode::Early,
                        controlled,
                        &events,
                    )
                } else {
                    adversarial::reject_corruptions(binding, layout, controlled, &events)
                };
                println!(
                    "COHORT C extended={extended} combinations={combinations} joined={joined} sanitized={sanitized} {actual_layout:?} controlled={controlled} events={} frontier={} rejected={rejected}",
                    r.events, r.maximum_frontier
                );
            }
        }
    }
}

fn run_events(run: &mut Command, controlled: bool) -> Vec<String> {
    lifetime_events(std::str::from_utf8(&run_cohort(run, controlled).stdout).unwrap())
}

fn lifetime_events(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter(|s| {
            s.split(',').count() == 4 && !s.starts_with("RINZU_") && !s.starts_with("await_park,")
        })
        .map(str::to_owned)
        .collect()
}

fn run_cohort(run: &mut Command, controlled: bool) -> Output {
    let output = platform::bounded(run);
    let report = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.status.code(),
        Some(i32::from(controlled)),
        "{report}"
    );
    assert!(
        report.contains(if controlled {
            "RINZU_FAILURE=cancelled"
        } else {
            "RINZU_FAILURE=none"
        }),
        "{report}"
    );
    assert!(report.contains("\"resources\":\"baseline\""), "{report}");
    assert!(
        !report.contains("runtime error:") && !report.contains("ERROR: AddressSanitizer"),
        "{report}"
    );
    output
}
