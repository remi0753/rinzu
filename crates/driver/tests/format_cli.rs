#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "rinzu-fmt-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }
    fn command(&self) -> Command {
        let mut command = Command::new(frozen_cli::path());
        command.arg("fmt").current_dir(&self.0);
        command
    }
    fn stdin(&self, flags: &[&str], bytes: &[u8]) -> Output {
        let mut child = self
            .command()
            .args(flags)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(bytes).unwrap();
        child.wait_with_output().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn success(output: Output) -> Output {
    assert!(output.status.success(), "{output:?}");
    output
}
fn rejected(output: Output, message: &str) {
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(message),
        "{output:?}"
    );
}

#[test]
fn syntax_aware_spacing_members_and_width_are_shared_by_stdin_and_file_modes() {
    let fixture = Fixture::new();
    let original = concat!(
        "class Box<T>{const values:Map<String,List<T>>,count:Int}",
        "enum Choice<T>{Some(value:T),None}",
        "fn main(){let value=combine(firstLongArgumentExpressionName,",
        "secondLongArgumentExpressionName,thirdLongArgumentExpressionName);",
        "if left<right{let selected=Choice<Int>.Some(value:1);}}"
    );
    let stdin = success(fixture.stdin(&["-"], original.as_bytes())).stdout;
    let text = String::from_utf8(stdin.clone()).unwrap();
    assert!(
        text.contains("values: Map<String, List<T>>,\n    count: Int\n"),
        "{text}"
    );
    assert!(text.contains("Some(value: T),\n    None\n"), "{text}");
    assert!(text.contains("left < right"), "{text}");
    assert!(text.contains("Choice<Int>.Some(value: 1)"), "{text}");
    assert!(text.contains("combine(\n"), "{text}");
    assert!(
        text.lines().all(|line| line.chars().count() <= 120),
        "{text}"
    );
    let path = fixture.file("style.rinzu", original.as_bytes());
    success(
        fixture
            .command()
            .arg("--write")
            .arg(&path)
            .output()
            .unwrap(),
    );
    assert_eq!(std::fs::read(&path).unwrap(), stdin);
    success(
        fixture
            .command()
            .arg("--check")
            .arg(&path)
            .output()
            .unwrap(),
    );
    success(fixture.stdin(&["--check", "-"], &stdin));
}

#[test]
fn stdout_and_write_preserve_current_syntax_literals_comments_and_bom() {
    let fixture = Fixture::new();
    let source = "\u{feff}import rinzu.List;\n// 日本語\r\ntype Pair={x:Int,y:Int};class Box<T>{const value:T}class Counter{const tag:String,value:Int fn read(self:Counter)->Int{return self.value;}}native{record Input{value:CInt}}async fn main(){let bytes=b\"\\x00\\xff\";let raw=r##\"日本\r\n\t\\u{0}\0\"##;var base:Int=40;let p:Pair={y=2,x=1};let c=Counter(tag:\"日本\",value:0);let add=fn(a:Int)->Int{return base+a;};let later=async fn()->Int{return add(p.x);};try all{let n=await later();c.value=n;}catch errors{let count=rinzu.List.length(errors);}finally{let text=f\"日本-{c.value}\";}} // 終端\r\n";
    let path = fixture.file("日本 語.rinzu", source.as_bytes());
    let stdout = success(fixture.command().arg(&path).output().unwrap()).stdout;
    assert_eq!(std::fs::read(&path).unwrap(), source.as_bytes());
    assert!(stdout.starts_with(&[0xef, 0xbb, 0xbf]));
    let text = String::from_utf8(stdout.clone()).unwrap();
    for preserved in [
        "// 日本語",
        "// 終端",
        "b\"\\x00\\xff\"",
        "r##\"日本\r\n\t\\u{0}\0\"##",
        "f\"日本-{c.value}\"",
    ] {
        assert!(text.contains(preserved), "missing {preserved:?}");
    }
    assert_eq!(
        fixture
            .command()
            .arg("--check")
            .arg(&path)
            .output()
            .unwrap()
            .status
            .code(),
        Some(1)
    );
    success(
        fixture
            .command()
            .arg("--write")
            .arg(&path)
            .output()
            .unwrap(),
    );
    assert_eq!(std::fs::read(&path).unwrap(), stdout);
    success(
        fixture
            .command()
            .arg("--check")
            .arg(&path)
            .output()
            .unwrap(),
    );
    let second = success(
        fixture
            .command()
            .arg("--write")
            .arg(&path)
            .output()
            .unwrap(),
    );
    assert!(
        second.stderr.is_empty(),
        "idempotent write should not replace file"
    );
    assert_eq!(std::fs::read(&path).unwrap(), stdout);
}

#[test]
fn directories_format_source_modules_without_manifest_and_skip_generated_trees() {
    let fixture = Fixture::new();
    let first = fixture.file("src/main.rinzu", b"fn main(){let x=1;}");
    let library = fixture.file(
        "src/sub/library.rinzu",
        b"fn identity<T>(value:T)->T{return value;}export identity;",
    );
    for name in [
        "build/generated.rinzu",
        "target/generated.rinzu",
        ".cache/broken.rinzu",
    ] {
        fixture.file(name, b"invalid {");
    }
    rejected(
        fixture.command().arg(&fixture.0).output().unwrap(),
        "stdout requires one file",
    );
    let check = fixture
        .command()
        .arg("--check")
        .arg(&fixture.0)
        .output()
        .unwrap();
    assert_eq!(check.status.code(), Some(1));
    assert!(check.stdout.is_empty());
    assert_eq!(std::fs::read(&first).unwrap(), b"fn main(){let x=1;}");
    success(
        fixture
            .command()
            .arg("--write")
            .arg(&fixture.0)
            .output()
            .unwrap(),
    );
    success(
        fixture
            .command()
            .arg("--check")
            .arg(&fixture.0)
            .output()
            .unwrap(),
    );
    assert!(
        std::fs::read_to_string(library)
            .unwrap()
            .contains("identity")
    );
    assert!(!fixture.0.join("rinzu.toml").exists());
    assert_eq!(
        std::fs::read(fixture.0.join("build/generated.rinzu")).unwrap(),
        b"invalid {"
    );
}

#[test]
fn invalid_syntax_in_later_file_prevents_every_write() {
    let fixture = Fixture::new();
    let original = b"fn main(){let x=1;}";
    let valid = fixture.file("a.rinzu", original);
    let broken = fixture.file("z.rinzu", b"fn broken( {");
    rejected(
        fixture
            .command()
            .arg("--write")
            .arg(&fixture.0)
            .output()
            .unwrap(),
        "RZ0101",
    );
    assert_eq!(std::fs::read(valid).unwrap(), original);
    assert_eq!(std::fs::read(broken).unwrap(), b"fn broken( {");
    assert!(std::fs::read_dir(&fixture.0).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".rinzu-fmt-")
    }));
}

#[test]
fn invalid_utf8_and_unterminated_literals_are_preserved_and_diagnosed() {
    let fixture = Fixture::new();
    for (name, bytes, code) in [
        ("utf8.rinzu", b"fn main(){}\xff".as_slice(), "RZ0001"),
        (
            "literal.rinzu",
            b"fn main(){let x=\"broken;}".as_slice(),
            "RZ0004",
        ),
    ] {
        let path = fixture.file(name, bytes);
        rejected(
            fixture
                .command()
                .arg("--write")
                .arg(&path)
                .output()
                .unwrap(),
            code,
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn stdin_can_be_formatted_or_checked_without_any_disk_write() {
    let fixture = Fixture::new();
    let original = b"fn main(){let x=1;}";
    let formatted = success(fixture.stdin(&["-"], original)).stdout;
    assert_ne!(formatted, original);
    success(fixture.stdin(&["-", "--check"], &formatted));
    assert_eq!(
        fixture.stdin(&["--check", "-"], original).status.code(),
        Some(1)
    );
    rejected(fixture.stdin(&["-"], b"fn main( {"), "RZ0101");
    rejected(
        fixture.command().args(["--write", "-"]).output().unwrap(),
        "does not support --write",
    );
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 0);
}

#[test]
fn options_paths_and_empty_inputs_are_checked() {
    let fixture = Fixture::new();
    let first = fixture.file("first.rinzu", b"fn main(){}");
    let second = fixture.file("second.rinzu", b"fn main(){}");
    rejected(
        fixture.command().output().unwrap(),
        "requires a source path",
    );
    rejected(
        fixture.command().arg("--unknown").output().unwrap(),
        "unknown fmt option",
    );
    rejected(
        fixture
            .command()
            .args(["--check", "--write"])
            .arg(&first)
            .output()
            .unwrap(),
        "one of --check or --write",
    );
    rejected(
        fixture.command().arg(&first).arg(&second).output().unwrap(),
        "stdout requires one file",
    );
    let other = fixture.file("other.txt", b"fn main(){}");
    rejected(
        fixture.command().arg(other).output().unwrap(),
        ".rinzu extension",
    );
    std::fs::create_dir(fixture.0.join("empty")).unwrap();
    rejected(
        fixture
            .command()
            .args(["--check", "empty"])
            .output()
            .unwrap(),
        "no .rinzu source",
    );
    let dashed = fixture.file("--source.rinzu", b"fn main(){}");
    success(
        fixture
            .command()
            .args(["--write", "--", "--source.rinzu"])
            .output()
            .unwrap(),
    );
    assert!(std::fs::read_to_string(dashed).unwrap().ends_with('\n'));
    let help = success(fixture.command().arg("--help").output().unwrap());
    assert!(String::from_utf8_lossy(&help.stdout).contains("--check"));
}

#[test]
fn formatting_checks_syntax_without_requiring_typecheck_or_main() {
    let fixture = Fixture::new();
    let path = fixture.file(
        "library.rinzu",
        b"fn library()->Unknown{return missing;}export library;",
    );
    success(
        fixture
            .command()
            .arg("--write")
            .arg(&path)
            .output()
            .unwrap(),
    );
    success(
        fixture
            .command()
            .arg("--check")
            .arg(&path)
            .output()
            .unwrap(),
    );
}

#[cfg(unix)]
#[test]
fn explicit_symlink_alias_is_preserved_deduplicated_and_permissions_survive() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let fixture = Fixture::new();
    let real = fixture.file("actual.rinzu", b"fn main(){let x=1;}");
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o640)).unwrap();
    let alias = fixture.0.join("alias.rinzu");
    symlink(&real, &alias).unwrap();
    symlink(&fixture.0, fixture.0.join("loop")).unwrap();
    let output = success(
        fixture
            .command()
            .arg("--write")
            .arg(&alias)
            .arg(&fixture.0)
            .output()
            .unwrap(),
    );
    assert_eq!(String::from_utf8_lossy(&output.stderr).lines().count(), 1);
    assert!(
        std::fs::symlink_metadata(alias)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::metadata(&real).unwrap().permissions().mode() & 0o777,
        0o640
    );
    success(
        fixture
            .command()
            .arg("--check")
            .arg(&fixture.0)
            .output()
            .unwrap(),
    );
}

#[cfg(unix)]
#[test]
fn read_only_later_source_prevents_batch_changes() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let original = b"fn main(){let x=1;}";
    let first = fixture.file("a.rinzu", original);
    let last = fixture.file("z.rinzu", original);
    std::fs::set_permissions(&last, std::fs::Permissions::from_mode(0o444)).unwrap();
    rejected(
        fixture
            .command()
            .arg("--write")
            .arg(&fixture.0)
            .output()
            .unwrap(),
        "read-only source",
    );
    assert_eq!(std::fs::read(first).unwrap(), original);
    assert_eq!(std::fs::read(&last).unwrap(), original);
    assert_eq!(
        fixture
            .command()
            .arg("--check")
            .arg(&fixture.0)
            .output()
            .unwrap()
            .status
            .code(),
        Some(1)
    );
}
