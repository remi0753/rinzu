use rinzu_runtime::supplant::{Cancellation, Phase, SupplantCoordinator, SupplantError, Winner};
use std::sync::{Arc, Barrier};

#[test]
fn reservation_remains_exclusive_through_physical_return_and_origin_settlement() {
    let state = SupplantCoordinator::new();
    let generation = state.reserve().unwrap();
    assert_eq!(state.snapshot().phase, Phase::Preparing);
    assert_eq!(state.reserve(), Err(SupplantError::Busy));
    assert!(state.queue(generation).unwrap());
    assert_eq!(state.reserve(), Err(SupplantError::Busy));
    assert!(state.begin_entry(generation).unwrap());
    assert_eq!(state.reserve(), Err(SupplantError::Busy));
    state.entered(generation).unwrap();
    assert!(state.cancel(generation, Cancellation::Task));
    assert!(!state.cancel(generation, Cancellation::Shutdown));
    assert_eq!(state.snapshot().phase, Phase::CancelRequested);
    assert_eq!(
        state.finish_settlement(generation),
        Err(SupplantError::InvalidPhase)
    );
    assert_eq!(state.reserve(), Err(SupplantError::Busy));
    state.returned(generation).unwrap();
    assert_eq!(state.snapshot().phase, Phase::Settling);
    assert_eq!(state.reserve(), Err(SupplantError::Busy));
    assert_eq!(
        state.finish_settlement(generation).unwrap(),
        Winner::Cancelled
    );
    let next = state.reserve().unwrap();
    assert!(next.bits() > generation.bits());
    assert!(!state.cancel(generation, Cancellation::Task));
    assert_eq!(
        state.returned(generation),
        Err(SupplantError::StaleGeneration)
    );
    assert_eq!(
        state.finish_settlement(generation),
        Err(SupplantError::StaleGeneration)
    );
    assert_eq!(state.begin_entry(next), Err(SupplantError::InvalidPhase));
    assert!(state.fail_preparation(next).unwrap());
    assert!(!state.cancel(next, Cancellation::Task));
    assert!(!state.queue(next).unwrap());
    assert!(!state.begin_entry(next).unwrap());
    assert_eq!(state.finish_settlement(next).unwrap(), Winner::Failed);
    assert_eq!(state.snapshot().phase, Phase::Idle);
}

#[test]
fn preentry_cancel_and_shutdown_never_require_or_synthesize_native_return() {
    let state = SupplantCoordinator::new();
    for queued in [false, true] {
        for cancellation in [Cancellation::Task, Cancellation::Shutdown] {
            let generation = state.reserve().unwrap();
            if queued {
                assert!(state.queue(generation).unwrap());
            }
            assert!(state.cancel(generation, cancellation));
            assert!(!state.snapshot().entered);
            assert_eq!(state.reserve(), Err(SupplantError::Busy));
            assert!(!state.queue(generation).unwrap());
            assert!(!state.begin_entry(generation).unwrap());
            assert_eq!(state.returned(generation), Err(SupplantError::InvalidPhase));
            assert_eq!(
                state.finish_settlement(generation).unwrap(),
                if cancellation == Cancellation::Task {
                    Winner::Cancelled
                } else {
                    Winner::Shutdown
                }
            );
        }
    }
}

#[test]
fn concurrent_entry_cancel_and_return_choose_one_terminal_winner_and_keep_generations() {
    let state = Arc::new(SupplantCoordinator::new());
    let mut entered = 0;
    let mut returned = 0;
    let mut last_generation = 0;
    for round in 0..1024 {
        let generation = state.reserve().unwrap();
        assert!(generation.bits() > last_generation);
        last_generation = generation.bits();
        state.queue(generation).unwrap();
        if round % 2 == 0 {
            assert!(state.begin_entry(generation).unwrap());
            state.entered(generation).unwrap();
            entered += 1;
        }
        let barrier = Arc::new(Barrier::new(2));
        let notifier_state = Arc::clone(&state);
        let notifier_barrier = Arc::clone(&barrier);
        let notifier = std::thread::spawn(move || {
            notifier_barrier.wait();
            let won = notifier_state.cancel(generation, Cancellation::Task);
            assert!(!notifier_state.cancel(generation, Cancellation::Shutdown));
            won
        });
        barrier.wait();
        let started = if round % 2 == 0 {
            true
        } else {
            state.begin_entry(generation).unwrap()
        };
        if started {
            if round % 2 == 1 {
                state.entered(generation).unwrap();
                entered += 1;
            }
            // This marks an actual model callee return; cancel alone cannot
            // advance Running/CancelRequested into settlement.
            state.returned(generation).unwrap();
            returned += 1;
        }
        let cancelled = notifier.join().unwrap();
        assert!(started || cancelled);
        assert_eq!(state.reserve(), Err(SupplantError::Busy));
        assert_eq!(
            state.finish_settlement(generation).unwrap(),
            if cancelled {
                Winner::Cancelled
            } else {
                Winner::Returned
            }
        );
        assert!(!state.cancel(generation, Cancellation::Task));
        assert_eq!(state.snapshot().phase, Phase::Idle);
    }
    assert_eq!(entered, returned);
    assert!(entered >= 512);
}

#[test]
fn only_origin_can_reserve_enter_and_release_while_notifiers_only_cancel() {
    let state = Arc::new(SupplantCoordinator::new());
    let generation = state.reserve().unwrap();
    state.queue(generation).unwrap();
    let notifier = Arc::clone(&state);
    std::thread::spawn(move || {
        assert_eq!(notifier.reserve(), Err(SupplantError::WrongThread));
        assert_eq!(
            notifier.begin_entry(generation),
            Err(SupplantError::WrongThread)
        );
        assert!(notifier.cancel(generation, Cancellation::Shutdown));
        assert_eq!(
            notifier.finish_settlement(generation),
            Err(SupplantError::WrongThread)
        );
    })
    .join()
    .unwrap();
    assert!(!state.begin_entry(generation).unwrap());
    assert_eq!(
        state.finish_settlement(generation).unwrap(),
        Winner::Shutdown
    );
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn c_start_gate_passes_signed_sanitizer_races_and_quiet_protocol_rejections() {
    use std::process::Command;
    let root = std::env::temp_dir().join(format!("rinzu-supplant-protocol-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for sanitized in [false, true] {
        let binary = root.join(if sanitized { "sanitized" } else { "signed" });
        let mut clang = Command::new("clang");
        clang
            .args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread"])
            .arg(manifest.join("tests/fixtures/supplant.c"))
            .arg("-I")
            .arg(manifest.join("../../include"));
        if sanitized {
            clang.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
        }
        let output = clang.arg("-o").arg(&binary).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if !sanitized {
            for args in [
                vec!["--force", "--sign", "-", "--options", "runtime"],
                vec!["--verify", "--strict"],
            ] {
                let output = Command::new("codesign")
                    .args(args)
                    .arg(&binary)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            let output = Command::new("codesign")
                .args(["-d", "--entitlements", "-"])
                .arg(&binary)
                .output()
                .unwrap();
            assert!(output.status.success() && output.stdout.is_empty());
        }
        let output = Command::new(&binary).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1024 entry/cancel/return races"));
        assert!(output.stderr.is_empty());
        for mode in [
            "owner",
            "overflow",
            "early-entry",
            "early-return",
            "early-release",
            "dispose",
            "stale",
            "double-entry",
            "double-return",
            "cancel-release",
            "bad-cancel",
        ] {
            let output = Command::new(&binary).arg(mode).output().unwrap();
            assert_eq!(
                output.status.code(),
                Some(86),
                "{mode}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                output.stdout.is_empty() && output.stderr.is_empty(),
                "{mode}"
            );
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
