use rinzu_compiler::{
    PackageTestOptions, SourceCache, SourceSnapshot, check_package_test_file, check_package_tests,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture(PathBuf);
impl Fixture {
    fn new(kind: &str, source: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "rinzu-test-frontend-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(path.join("src")).unwrap();
        std::fs::create_dir(path.join("tests")).unwrap();
        std::fs::write(
            path.join("rinzu.toml"),
            format!("[package]\nname=\"test_library\"\nlanguage=\"0.5\"\nkind=\"{kind}\"\nentry=\"src/main.rinzu\"\n"),
        )
        .unwrap();
        std::fs::write(path.join("src/main.rinzu"), source).unwrap();
        Self(path)
    }
    fn check(
        &self,
        options: PackageTestOptions,
    ) -> Result<
        (
            rinzu_compiler::Manifest,
            rinzu_compiler::FrontendArtifact,
            Vec<rinzu_compiler::PackageTestCase>,
        ),
        Vec<rinzu_compiler::Diagnostic>,
    > {
        check_package_tests(
            &self.0,
            &mut SourceSnapshot::new(&mut SourceCache::default(), Default::default()),
            &options,
        )
    }
    fn check_file(
        &self,
        source: &str,
    ) -> Result<
        (
            rinzu_compiler::Manifest,
            rinzu_compiler::FrontendArtifact,
            rinzu_compiler::PackageTestCase,
        ),
        Vec<rinzu_compiler::Diagnostic>,
    > {
        let path = self.0.join("tests/selected.test.rinzu");
        std::fs::write(&path, source).unwrap();
        check_package_test_file(
            &self.0,
            &path,
            &mut SourceSnapshot::new(&mut SourceCache::default(), Default::default()),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn test_file_loads_its_imports_without_loading_app_main_or_unselected_files() {
    let fixture = Fixture::new("application", "invalid application main");
    std::fs::write(
        fixture.0.join("src/other.rinzu"),
        "fn test_identity<T>(x:T)->T {return x;} export test_identity;",
    )
    .unwrap();
    std::fs::write(
        fixture.0.join("tests/support.rinzu"),
        "fn value()->Int {return 42;} export value;",
    )
    .unwrap();
    std::fs::write(
        fixture.0.join("tests/unselected.test.rinzu"),
        "invalid test source",
    )
    .unwrap();
    let (_, artifact, case) = fixture
        .check_file(
            r#"import other;
import "./support.rinzu" as support;
fn main() {let value=other.test_identity<Int>(support.value());}"#,
        )
        .unwrap();
    assert_eq!(case.identity, "file:tests/selected.test.rinzu");
    assert!(
        artifact
            .ir
            .source_files
            .values()
            .any(|path| path.ends_with("/<test entry>"))
    );
    for file in [
        "src/other.rinzu",
        "tests/support.rinzu",
        "tests/selected.test.rinzu",
    ] {
        assert!(
            artifact
                .checked_inputs
                .contains_key(&fixture.0.canonicalize().unwrap().join(file))
        );
    }
    assert!(artifact.module_graph.iter().all(|node| {
        node.file_id != rinzu_compiler::FileId::MANIFEST
            && artifact.checked_inputs.contains_key(&node.path)
            && !node.path.ends_with("src/main.rinzu")
            && !node.path.ends_with("unselected.test.rinzu")
    }));
    assert!(
        artifact
            .ir
            .functions
            .iter()
            .any(|function| function.name == "main")
    );
}

#[test]
fn selected_async_main_keeps_call_future_and_await_value_ranges_distinct() {
    let fixture = Fixture::new("application", "fn main() {}");
    let (_, artifact, case) = fixture
        .check_file("async fn answer()->Int {return 42;} async fn main(){let value=await answer();if value!=42 {let bad=CInt8(300);}}")
        .unwrap();
    assert!(case.is_async);
    assert!(
        artifact
            .ir
            .functions
            .iter()
            .any(|function| function.name.ends_with("::main"))
    );
}

#[test]
fn invalid_test_entry_is_diagnosed_in_the_test_file() {
    let fixture = Fixture::new("application", "fn main() {}");
    for (source, message) in [
        ("fn main(x:Int) {}", "signature"),
        ("fn main<T>() {}", "signature"),
        ("fn main()->Int {return 1;}", "signature"),
        ("fn helper() {}", "exactly one main"),
        ("fn main() {} fn main() {}", "exactly one main"),
    ] {
        let errors = fixture.check_file(source).unwrap_err();
        assert_eq!(
            errors[0].code,
            rinzu_compiler::DiagnosticCode::InvalidEntryPoint
        );
        assert!(errors[0].message.contains(message), "{errors:?}");
    }
}

#[test]
fn sdk_test_imports_its_own_interface_without_publishing_test_main() {
    let fixture = Fixture::new("sdk", "fn answer()->Int {return 42;} export answer;");
    let (_, artifact, _) = fixture
        .check_file("import test_library; fn main() {let x=test_library.answer();}")
        .unwrap();
    let own = artifact
        .sdk_packages
        .last()
        .unwrap()
        .source
        .as_ref()
        .unwrap();
    assert!(!own.syntax.declarations.iter().any(|d| d.name() == "main"));
    assert!(own.modules["test_library"].contains_key("answer"));
}

#[test]
fn source_modules_cannot_import_test_helpers_even_during_test_execution() {
    let fixture = Fixture::new(
        "application",
        "import \"../tests/support.rinzu\" as support; fn main() {}",
    );
    std::fs::write(fixture.0.join("tests/support.rinzu"), "fn value() {}").unwrap();
    for errors in [
        rinzu_compiler::check_package(&fixture.0).unwrap_err(),
        fixture
            .check_file("import main as application; fn main() {}")
            .unwrap_err(),
    ] {
        assert_eq!(
            errors[0].code,
            rinzu_compiler::DiagnosticCode::InvalidModulePath
        );
    }
}

#[test]
fn test_sources_cannot_be_loaded_under_an_outer_package() {
    let fixture = Fixture::new("application", "fn main() {}");
    let nested = fixture.0.join("tests/nested");
    std::fs::create_dir_all(nested.join("tests")).unwrap();
    std::fs::write(
        nested.join("rinzu.toml"),
        "[package]\nname=\"nested\"\nlanguage=\"0.5\"\n",
    )
    .unwrap();
    let path = nested.join("tests/nested.test.rinzu");
    std::fs::write(&path, "fn main() {}").unwrap();

    let (_, _, case) = check_package_test_file(
        &nested,
        &path,
        &mut SourceSnapshot::new(&mut SourceCache::default(), Default::default()),
    )
    .unwrap();
    assert_eq!(case.identity, "file:tests/nested.test.rinzu");
    for errors in [
        check_package_test_file(
            &fixture.0,
            &path,
            &mut SourceSnapshot::new(&mut SourceCache::default(), Default::default()),
        )
        .unwrap_err(),
        fixture
            .check_file(r#"import "./nested/tests/nested.test.rinzu" as nested; fn main() {}"#)
            .unwrap_err(),
    ] {
        assert_eq!(
            errors[0].code,
            rinzu_compiler::DiagnosticCode::InvalidModulePath
        );
        assert!(errors[0].message.contains("belongs to a different package"));
    }
}

#[test]
fn explicit_doc_programs_preserve_bom_unicode_crlf_byte_positions() {
    let fixture = Fixture::new("application", "fn main() {panic(\"bad\");}\n");
    let text = "\u{feff}# 日本語🙂\r\n```rinzu\r\ninvalid ordinary fragment\r\n```\r\n```rinzu-test\r\nfn main() {let value=\"日本語🙂\";}\r\n```\r\n~~~rinzu-test\r\nasync fn main() {}\r\n~~~\r\n";
    std::fs::write(fixture.0.join("README.md"), text).unwrap();
    let options = PackageTestOptions {
        documents: vec!["README.md".into()],
        ..Default::default()
    };
    let (_, _, cases) = fixture.check(options).unwrap();
    assert_eq!(cases.len(), 2);
    assert!(cases[0].identity.ends_with("#L5"));
    assert_eq!(
        cases[0].range.start_byte as usize,
        text.find("fn main()").unwrap()
    );
    assert!(cases[1].is_async);
}

#[test]
fn standalone_package_and_doc_programs_infer_the_same_types() {
    let program = r#"
import rinzu.map;
import rinzu.option;

interface Measured {
    fn measure(self: Self) nothrow -> Int;
}
class Sample interface Measured {
    private const value: Int
    fn create(value: Int) -> Sample { return Sample(value: value); }
    fn measure(self: Sample) nothrow -> Int { return self.value; }
}
fn sum<T interface Measured>(values: List<T>) nothrow -> Int {
    var total = 0;
    for value in values { total = total + value.measure(); }
    return total;
}
class Box<T> {
    private const value: T
    fn create(value: T) -> Box<T> { return Box<T>(value: value); }
    fn map<U>(self: Box<T>, transform: fn (T) -> U) -> Box<U> {
        return Box<U>.create(transform(self.value));
    }
    fn get(self: Box<T>) -> T { return self.value; }
}
fn identity<T>(value: T) -> T { return value; }
let makeSample = fn (value: Int) -> Sample { return identity(Sample.create(value)); };
type Report = { title: String, total: Int };
fn main() {
    let counts = {"one": 42};
    let count = rinzu.option.getOrElse(rinzu.map.get(counts, "one"), fn () -> Int { return 0; });
    let original: Report = { title = "measurements", total = 0 };
    let updated = { ...original, total = sum([Sample.create(20), Sample.create(22)]) };
    let transform = fn (value: Int) -> Sample { return makeSample(value + 1); };
    let box = Box<Int>.create(41);
    let mapped = box.map(transform);
    let result: Sample = mapped.get();
    if count != 42 || updated.total != 42 || result.measure() != 42 { panic("invalid result"); }
}
"#;
    let fixture = Fixture::new("application", program);
    rinzu_compiler::check_input(
        &fixture.0.join("src/main.rinzu"),
        &rinzu_compiler::CompilationOptions {
            language: None,
            target: rinzu_compiler::standard_library::TARGET.into(),
        },
        &Default::default(),
    )
    .unwrap();
    rinzu_compiler::check_package(&fixture.0).unwrap();
    let document = fixture.0.join("README.md");
    std::fs::write(&document, format!("```rinzu-test\n{program}```\n")).unwrap();
    let (_, artifact, cases) = fixture
        .check(PackageTestOptions {
            documents: vec!["README.md".into()],
            selected: Some(format!(
                "doc:{}#L1",
                document.canonicalize().unwrap().display()
            )),
        })
        .unwrap();
    assert_eq!(cases.len(), 1);
    assert!(
        artifact
            .ir
            .functions
            .iter()
            .any(|f| f.name == "rinzu_doc_case_0::main")
    );
}

#[test]
fn doc_examples_respect_option_shadowing_in_their_own_module() {
    let fixture = Fixture::new("application", "fn main() {}\n");
    let options = PackageTestOptions {
        documents: vec!["README.md".into()],
        ..Default::default()
    };
    let path = fixture.0.join("README.md");
    let option = "enum Option<T> { Custom(value: T) }";
    let uses_data = "import rinzu.list; fn main() { let value = rinzu.list.get([42], 0); }";
    std::fs::write(
        &path,
        format!("```rinzu-test\n{option} fn main() {{}}\n```\n```rinzu-test\n{uses_data}\n```\n"),
    )
    .unwrap();
    fixture.check(options.clone()).unwrap();
    std::fs::write(
        &path,
        format!("```rinzu-test\nimport rinzu.list; {option} fn main() {{ let value = rinzu.list.get([42], 0); }}\n```\n"),
    )
    .unwrap();
    let errors = fixture.check(options).unwrap_err();
    assert!(errors.iter().any(|e| e.message.contains("shadows it")));
}

#[test]
fn doc_fence_errors_and_native_permission_are_not_hidden() {
    let fixture = Fixture::new("application", "fn main() {}\n");
    for text in [
        "```rinzu-test extra\nfn main() {}\n```",
        "```rinzu-test\nfn main() {}",
        "```rinzu-test\nfn helper() {}\n```",
        "```rinzu-test\nnative {fn bad()->Int symbol \"bad\";} fn main() {}\n```",
        "```rinzu-test\nimport rinzu.unavailable; fn main() {}\n```",
    ] {
        std::fs::write(fixture.0.join("README.md"), text).unwrap();
        assert!(
            fixture
                .check(PackageTestOptions {
                    documents: vec!["README.md".into()],
                    ..Default::default()
                })
                .is_err(),
            "{text}"
        );
    }
}

#[test]
fn standard_sdk_doc_uses_its_checked_local_public_api() {
    let format = |text: &str| {
        rinzu_compiler::format_source(&rinzu_compiler::SourceFile::new(
            rinzu_compiler::FileId(1),
            "fixture.rinzu",
            text,
        ))
        .unwrap()
    };
    let source = format("fn localValue()->Int {return 42;} export localValue;\n");
    let fixture = Fixture::new("sdk", &source);
    let manifest = fixture.0.join("rinzu.toml");
    let text = std::fs::read_to_string(&manifest)
        .unwrap()
        .replace("test_library", "rinzu.testing");
    std::fs::write(manifest, text).unwrap();
    let program =
        format("import rinzu.testing; fn main(){let value=rinzu.testing.localValue();}\n");
    std::fs::write(
        fixture.0.join("README.md"),
        format!("```rinzu-test\n{program}```\n"),
    )
    .unwrap();
    let (_, artifact, _) = fixture
        .check(PackageTestOptions {
            documents: vec!["README.md".into()],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        artifact
            .sdk_packages
            .iter()
            .filter(|p| p.manifest.name == "rinzu.testing")
            .count(),
        1
    );
    assert_eq!(
        artifact
            .sdk_packages
            .iter()
            .find(|p| p.manifest.name == "rinzu.testing")
            .unwrap()
            .manifest
            .root,
        fixture.0.canonicalize().unwrap()
    );
}

#[test]
fn host_observes_test_errors_without_relaxing_user_boundaries_or_future_obligations() {
    use rinzu_compiler::DiagnosticCode;
    let fixture = Fixture::new(
        "application",
        "error Failed{} fn main(){throw Failed(message:\"main\");}",
    );
    let main_errors = rinzu_compiler::check_package(&fixture.0).unwrap_err();
    assert!(
        main_errors
            .iter()
            .any(|error| error.code == DiagnosticCode::ErrorEscapesBoundary)
    );
    fixture
        .check_file("error Failed{} fn main(){throw Failed(message:\"test\");}")
        .unwrap();
    for (source, expected) in [
        (
            "error Failed{} async fn failed(){throw Failed(message:\"failed\");} fn main(){failed();}",
            DiagnosticCode::FutureDiscarded,
        ),
        (
            "error Failed{} fn worker(){throw Failed(message:\"worker\");} fn main(){let child=spawn worker();}",
            DiagnosticCode::ErrorEscapesBoundary,
        ),
        (
            "error Failed{} native{fn invoke(callback:fn(CInt)->CInt)->CInt symbol \"invoke\";} fn callback(value:CInt)->CInt{throw Failed(message:\"callback\");} fn main(){let result=invoke(callback);}",
            DiagnosticCode::ErrorEscapesBoundary,
        ),
    ] {
        let errors = fixture.check_file(source).unwrap_err();
        assert!(
            errors.iter().any(|error| error.code == expected),
            "{errors:?}"
        );
    }
}
