use rinzu_runtime_model::reload::{
    CallbackTrampoline, DispatchCell, DomainStatus, ReloadError, RestartDecision, decide_restart,
    perform_process_restart,
};
use std::process::Command;

#[test]
fn active_and_suspended_frames_keep_old_code_while_new_dispatch_uses_new_code() {
    let dispatch = DispatchCell::new(10);
    let trampoline = CallbackTrampoline::new(dispatch.clone());
    let active = dispatch.enter_call();
    let mut suspended = dispatch.enter_call();
    suspended.suspend();

    assert_eq!(dispatch.reload_body(true, 20), Ok(2));
    let new_call = dispatch.enter_call();
    assert_eq!((active.version(), active.execute()), (1, 10));
    assert_eq!((suspended.version(), suspended.execute()), (1, 10));
    assert!(suspended.is_suspended());
    suspended.resume();
    assert_eq!((suspended.version(), suspended.execute()), (1, 10));
    assert_eq!((new_call.version(), new_call.execute()), (2, 20));
    assert_eq!(trampoline.invoke(), (2, 20));
    assert_eq!(
        dispatch.reload_body(false, 30),
        Err(ReloadError::ProcessRestartRequired)
    );
    assert_eq!(dispatch.current_version(), 2);
}

#[test]
fn native_blocker_makes_runner_fall_back_to_a_real_process_restart() {
    assert_eq!(
        decide_restart(&[DomainStatus::AtSafePoint, DomainStatus::NativeBlocking]),
        RestartDecision::ProcessRestartNativeBlocker
    );
    assert_eq!(
        decide_restart(&[DomainStatus::AtSafePoint, DomainStatus::ManagedRunning]),
        RestartDecision::InProcessReload
    );

    let mut old = Command::new("sh").args(["-c", "sleep 30"]).spawn().unwrap();
    let status =
        perform_process_restart(&mut old, Command::new("sh").args(["-c", "exit 0"])).unwrap();
    assert!(status.success());
    assert!(old.try_wait().unwrap().is_some());
}
