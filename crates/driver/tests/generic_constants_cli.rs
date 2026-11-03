#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

#[path = "fixtures/checked_entry.rs"]
mod checked_entry;
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

use std::path::PathBuf;
use std::process::Command;

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn typed_generic_byte_lengths_agree_between_jit_and_aot() {
    let root = Fixture(
        std::env::temp_dir().join(format!("rinzu-generic-constants-{}", std::process::id())),
    );
    std::fs::create_dir_all(&root.0).unwrap();
    let source = root.0.join("lengths.rinzu");
    for entry in ["fn main()", "async fn main()"] {
        std::fs::write(
            &source,
            checked_entry::source(&format!(
                "import rinzu.stdio;
             fn id<T>(value: T) -> T {{ return value; }}
             {entry} {{ let size: CUInt8 = 255;
               let bytes = id<Bytes(wrappingAdd(size, 1))>(b\"\");
               let folded: CUInt8 = wrappingAdd(size, 1);
               let other = id<Bytes(folded)>(b\"\");
               rinzu.stdio.writeLine(byteLength(bytes));
               rinzu.stdio.writeLine(byteLength(other)); }}"
            )),
        )
        .unwrap();
        for command in ["check", "run", "verify-aot"] {
            let output = Command::new(frozen_cli::path())
                .current_dir(&root.0)
                .arg(command)
                .arg(&source)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{entry} {command}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            if command == "run" {
                assert_eq!(output.stdout, b"0\n0\n", "{entry}");
            }
        }
    }
}
