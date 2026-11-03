use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use rinzu_compiler::{
    CompilationOptions, FileId, NativeCliInputs, SourceFile, check_input, format_source,
};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "rinzu-native-module-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
    fn source(&self, name: &str, source: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text = format_source(&SourceFile::new(FileId(1), &path, source)).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }
    fn check(
        &self,
        path: &std::path::Path,
    ) -> Result<rinzu_compiler::CompilationContext, Vec<rinzu_compiler::Diagnostic>> {
        check_input(
            path,
            &CompilationOptions {
                language: None,
                target: "native-frontend-test".into(),
            },
            &NativeCliInputs::default(),
        )
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
#[cfg(unix)]
fn external_cli_objects_deduplicate_physical_aliases_but_keep_copies() {
    let scratch = Scratch::new();
    let entry = scratch.source("source/main.rinzu", "fn main(){}");
    let object = scratch.0.join("outside.o");
    std::fs::write(
        &object,
        b"frontend path fixture; driver checks the actual target",
    )
    .unwrap();
    let symlink = scratch.0.join("alias.o");
    let hardlink = scratch.0.join("hard.o");
    let copy = scratch.0.join("copy.o");
    std::os::unix::fs::symlink(&object, &symlink).unwrap();
    std::fs::hard_link(&object, &hardlink).unwrap();
    std::fs::copy(&object, &copy).unwrap();
    let context = check_input(
        &entry,
        &CompilationOptions {
            language: None,
            target: "native-frontend-test".into(),
        },
        &NativeCliInputs {
            objects: vec![
                object.clone(),
                symlink,
                hardlink,
                object.clone(),
                copy.clone(),
            ],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(context.artifact.native_inputs.objects, vec![object, copy]);
    assert!(
        context
            .artifact
            .native_inputs
            .objects
            .iter()
            .all(|path| path.is_absolute())
    );
    let errors = check_input(
        &entry,
        &CompilationOptions {
            language: None,
            target: "native-frontend-test".into(),
        },
        &NativeCliInputs {
            objects: vec![scratch.0.clone()],
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("regular file"))
    );
}

#[test]
fn file_path_aliases_share_checked_native_contract_and_original_position() {
    let scratch = Scratch::new();
    let native = scratch.source("api.rinzu", "native{fn value(x:CInt)->CInt symbol \"physical_value\";fn unused()->Int symbol \"unused_symbol\";}export value;");
    let main = scratch.source("main.rinzu", "import \"./api.rinzu\" as a;import \"././api.rinzu\" as b;fn main(){let x=a.value(CInt(1));let y=b.value(x);}");
    std::fs::write(scratch.0.join("rinzu.toml"), "unrelated invalid manifest").unwrap();
    let context = scratch.check(&main).unwrap();
    assert!(context.artifact.is_file_input);
    assert!(context.artifact.sdk_packages.is_empty());
    let source = context.artifact.source_interface.as_ref().unwrap();
    assert_eq!(source.signatures.len(), 2); // Includes unused declarations in reached modules.
    assert_eq!(source.source_inputs.len(), 2);
    assert!(source.modules.is_empty()); // Native publication grants no SDK exports.
    assert!(
        source
            .source_files
            .values()
            .any(|path| path == &native.display().to_string())
    );
    assert_eq!(context.artifact.ir.source_native_signatures.len(), 2);
    assert_eq!(context.artifact.checked.source_native_calls.len(), 2);
}

#[test]
fn application_and_dependency_retain_separate_owner_interfaces() {
    let scratch = Scratch::new();
    scratch.source(
        "sdk/src/api.rinzu",
        "native{fn sdkValue()->CInt symbol \"same_symbol\";}export sdkValue;",
    );
    std::fs::write(
        scratch.0.join("sdk/rinzu.toml"),
        "[package]\nname=\"model\"\nlanguage=\"0.5\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n",
    )
    .unwrap();
    scratch.source("app/src/main.rinzu", "import model;native{fn ownValue()->Int symbol \"same_symbol\";}fn main(){let x=ownValue();let y=model.sdkValue();}");
    std::fs::write(
        scratch.0.join("app/rinzu.toml"),
        "[package]\nname=\"app\"\nlanguage=\"0.5\"\n[dependencies]\nmodel={path=\"../sdk\"}\n",
    )
    .unwrap();
    let context = scratch.check(&scratch.0.join("app")).unwrap();
    assert!(!context.artifact.is_file_input);
    let own = context.artifact.source_interface.as_ref().unwrap();
    let dependency = context.artifact.sdk_packages[0].source.as_ref().unwrap();
    assert_eq!(own.signatures.len(), 1);
    assert_eq!(dependency.signatures.len(), 1);
    assert_eq!(own.signatures[0].name, "ownValue");
    assert_ne!(
        own.signatures[0].canonical_type(),
        dependency.signatures[0].canonical_type()
    );
    assert_eq!(context.artifact.ir.source_native_signatures.len(), 2);
}

#[test]
fn ordinary_modules_keep_existing_abi_errors_at_the_declaration() {
    let scratch = Scratch::new();
    let declaration = "fn bad(x:Unit)->Unit symbol \"bad\";";
    let native = scratch.source("invalid.rinzu", &format!("native{{{declaration}}}"));
    let main = scratch.source(
        "main.rinzu",
        "import \"./invalid.rinzu\" as api;fn main(){}",
    );
    let errors = scratch.check(&main).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("unsupported")
                || error.message.contains("registry")
                || error.message.contains("no source native ABI mapping"))
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("import chain:"))
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains(&native.display().to_string()))
    );
}

#[test]
fn checked_timer_library_needs_no_native_provider_or_manifest() {
    let scratch = Scratch::new();
    let main = scratch.source(
        "timer.rinzu",
        "import rinzu.timer; async fn main(){try{await rinzu.timer.sleep(1);}catch errors{}}",
    );
    let context = scratch.check(&main).unwrap();
    assert!(context.artifact.source_interface.is_none());
    assert!(context.artifact.ir.source_native_signatures[0].is_async);
    assert!(context.artifact.native_inputs.is_empty());
    assert!(!scratch.0.join("rinzu.toml").exists());
}

#[test]
fn optional_header_is_a_receipted_input_without_becoming_a_reference_permit() {
    let scratch = Scratch::new();
    let main = scratch.source(
        "source/main.rinzu",
        "native{fn value()->CInt symbol \"value\";}fn main(){}",
    );
    let header = scratch.0.join("external.h");
    std::fs::write(&header, "int value(void);\n").unwrap();
    let context = check_input(
        &main,
        &CompilationOptions {
            language: None,
            target: "native-frontend-test".into(),
        },
        &NativeCliInputs {
            headers: vec![header.clone()],
            ..NativeCliInputs::default()
        },
    )
    .unwrap();
    assert_eq!(
        context.metadata.native.headers.as_slice(),
        std::slice::from_ref(&header)
    );
    assert_eq!(
        context.artifact.native_inputs.headers.as_slice(),
        std::slice::from_ref(&header)
    );
    assert!(
        context
            .inputs
            .iter()
            .any(|input| input.path == header && input.kind == "disk-native-header")
    );
    assert!(context.artifact.native_inputs.objects.is_empty());
    assert!(context.artifact.native_inputs.libraries.is_empty());
}
