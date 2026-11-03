use rinzu_runtime::{V1_TARGET, current_target, run_empty_standalone, validate_v1_target};

#[test]
fn target_contract_is_explicit() {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        assert_eq!(current_target(), V1_TARGET);
        assert_eq!(run_empty_standalone().unwrap().target, V1_TARGET);
    } else {
        let failure = validate_v1_target().unwrap_err();
        assert_eq!(failure.expected, V1_TARGET);
        assert_eq!(failure.actual, current_target());
    }
}
