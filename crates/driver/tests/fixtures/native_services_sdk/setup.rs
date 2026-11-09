// Trigger control shortly after submission, even with GC stress and a busy host.
pub const CONTROL_POLLS: usize = 10;
// The sleep deadline in cancel.rinzu.
pub const TIMER_DURATION: std::time::Duration = std::time::Duration::from_secs(1);

pub fn create(root: &std::path::Path, cancelled: bool) {
    let _ = std::fs::remove_dir_all(root);
    for directory in ["app/src", "app/data", "services/src"] {
        std::fs::create_dir_all(root.join(directory)).unwrap();
    }
    std::fs::write(root.join("services/rinzu.toml"), "[package]\nname=\"services\"\nlanguage=\"0.2\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n[native]\n").unwrap();
    std::fs::write(root.join("app/rinzu.toml"), "[package]\nname=\"services_app\"\nlanguage=\"0.2\"\n[dependencies]\nservices={path=\"../services\"}\n").unwrap();
    std::fs::write(
        root.join("services/src/api.rinzu"),
        include_str!("api.rinzu"),
    )
    .unwrap();
    std::fs::write(
        root.join("app/src/main.rinzu"),
        if cancelled {
            include_str!("cancel.rinzu")
        } else {
            include_str!("main.rinzu")
        },
    )
    .unwrap();
    std::fs::write(root.join("app/data/input.txt"), b"rinzu").unwrap();
}
