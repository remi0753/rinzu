pub fn boundary(output: &std::process::Output, mode: usize) {
    use std::os::unix::process::ExitStatusExt;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let marker = |id| stderr.contains(&format!("TERMINATION_MARK={id}\n"));
    assert!(marker(0), "mode={mode}: {output:?}");
    if matches!(mode, 3 | 4 | 6 | 7) {
        assert!(output.status.success(), "mode={mode}: {output:?}");
        assert_eq!(marker(1), mode >= 6, "mode={mode}: {stderr}");
        assert_eq!(marker(2), mode >= 6, "mode={mode}: {stderr}");
        for id in [3, 4, 5, 9] {
            assert!(marker(id), "mode={mode}: {stderr}");
        }
    } else {
        if mode == 5 {
            assert_eq!(output.status.signal(), Some(6), "{output:?}");
        } else {
            assert!(
                output.status.code() == Some(86) || output.status.signal() == Some(6),
                "mode={mode}: {output:?}"
            );
        }
        assert_eq!(marker(1), mode <= 2 || mode == 5, "{stderr}");
        assert_eq!(marker(2), mode <= 2, "{stderr}");
        assert_eq!(marker(8), mode == 5, "{stderr}");
        for id in [3, 4, 5, 9] {
            assert!(!marker(id), "mode={mode}: {stderr}");
        }
        assert!(
            !stdout.contains("RINZU_NATIVE_CALLS=")
                && !stdout.contains("\"resources\":\"baseline\""),
            "{stdout}"
        );
    }
}
