#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
use std::path::Path;
use std::process::{Command, Output};

fn success(output: &Output) {
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
}
fn cli(root: &Path, action: &str) -> Output {
    Command::new(frozen_cli::path())
        .arg(action)
        .arg(root)
        .output()
        .unwrap()
}
fn values(output: &Output, prefix: &str) -> Vec<usize> {
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    text.lines()
        .find_map(|line| line.strip_prefix(prefix))
        .unwrap_or_else(|| panic!("missing {prefix}: {text}"))
        .split(',')
        .map(|number| number.parse().unwrap())
        .collect()
}

#[test]
fn closed_consumer_releases_return_copies_on_jit_source_worker_and_c_aot() {
    let root = std::env::temp_dir().join(format!("rinzu-scoped-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    let root = root.canonicalize().unwrap();
    let sdk = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../sdk/consumption")
        .canonicalize()
        .unwrap();
    std::os::unix::fs::symlink(sdk, root.join("consumption")).unwrap();
    std::fs::write(root.join("rinzu.toml"),"[package]\nname=\"scoped_app\"\nlanguage=\"0.5\"\n[dependencies]\n\"rinzu.consumption\"={path=\"consumption\"}\n").unwrap();
    let prelude = "import rinzu.consumption;fn label(index:CSize)->String fallback \"B\" {return \"A\";}fn data(index:CSize)->Bytes(4) fallback b\"\\x01\" {return b\"\\x01\\x02\\x03\";}fn execute()->Int {try{return Int(rinzu.consumption.strings(label,CSize(10))+rinzu.consumption.bytes4(data,CSize(10)));}catch errors{panic(\"failed\");return 0;}}";
    for main in [
        "fn main(){let result=execute();if result!=1420 {panic(\"invalid\");}}",
        "async fn main(){let result=execute();if result!=1420 {panic(\"invalid\");}}",
        "async fn main(){let result=await spawn execute();if result!=1420 {panic(\"invalid\");}}",
    ] {
        std::fs::write(root.join("src/main.rinzu"), format!("{prelude}{main}")).unwrap();
        success(&cli(&root, "run"));
        success(&cli(&root, "build"));
        let run = Command::new(root.join("build/scoped_app"))
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .output()
            .unwrap();
        success(&run);
        assert_eq!(values(&run, "RINZU_CALLBACK_RETURNS="), [0, 0, 0, 0]);
        let scopes = values(&run, "RINZU_CALLBACK_SCOPES=");
        assert_eq!(&scopes[..7], &[22, 22, 0, 66, 66, 0, 0]);
        assert!(scopes[7] > 0);
        assert_eq!(&scopes[8..], &[52, 0, 0]);
        let retirement = values(&run, "RINZU_CALLBACK_RETIREMENT=");
        assert_eq!(&retirement[..2], &[2, 64]);
        assert!(retirement[2] > 0);
        assert_eq!(values(&run, "RINZU_CALLBACK_PROCESS=")[1], 0);
        success(&cli(&root, "verify-aot"));
    }
    std::fs::remove_dir_all(root).unwrap();
}
