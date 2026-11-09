use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new(prefix: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp = std::env::temp_dir();
        std::fs::create_dir_all(&temp).unwrap();
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = temp.join(format!(
            "{prefix}-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn artifacts(&self) -> PathBuf {
        std::env::var_os("RINZU_GATE_ARTIFACTS")
            .map_or_else(|| self.0.join("artifacts"), PathBuf::from)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let result = std::fs::remove_dir_all(&self.0);
        // Report cleanup failures on success without aborting an unwinding test.
        if !std::thread::panicking() {
            result.unwrap();
        }
    }
}
