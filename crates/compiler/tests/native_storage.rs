use rinzu_compiler::{
    CompilerOptions, FileId, SourceAbiType, SourceFile, compile_source_with_options, verify_module,
};

fn compile(
    body: &str,
    sdk: bool,
) -> Result<rinzu_compiler::FrontendArtifact, Vec<rinzu_compiler::Diagnostic>> {
    let source = SourceFile::new(
        FileId(0),
        "native-records.rinzu",
        format!(
            "native {{ record Pair {{ x: Int }} fn update(out: Pair) -> Int symbol \"update\"; }} {body}"
        ),
    );
    let mut options = CompilerOptions::default();
    if sdk {
        options.sdk_files.insert(FileId(0));
    }
    compile_source_with_options(&source, &options)
}

#[test]
fn native_record_references_can_be_retained_across_await_and_captured() {
    let artifact = compile(
        r#"
        async fn pause() {}
        async fn main() {
            try {
                let pair = Pair(x: 1);
                let alias = pair;
                let retained = [pair, alias];
                let read = fn() -> Int { return alias.x; };
                await pause();
                let status = update(alias);
                let value = read();
            } catch errors {}
        }
    "#,
        true,
    )
    .unwrap();
    assert_eq!(
        artifact.ir.source_native_signatures[0].parameters,
        vec![SourceAbiType::Record("Pair".into())]
    );
    verify_module(&artifact.ir).unwrap();
}

#[test]
fn removed_storage_types_and_intrinsics_are_rejected_in_sdk_and_application_source() {
    for sdk in [false, true] {
        for body in [
            "native {fn old(out:NativeStorage<Pair>) symbol \"old\";} fn main() {}",
            "fn helper(out:NativeStorage<Pair>) {} fn main() {}",
            "class Box {const out:NativeStorage<Pair>} fn main() {}",
            "fn main() {let out:NativeStorage<Pair> = Pair(x:1);}",
            "fn main() {let out=nativeCopy(Pair(x:1));}",
            "fn main() {let out=nativeSnapshot(Pair(x:1));}",
            "import rinzu.core; fn main() {let out=rinzu.core.nativeCopy(Pair(x:1));}",
            "import rinzu.core; fn main() {let out=rinzu.core.nativeSnapshot(Pair(x:1));}",
        ] {
            assert!(compile(body, sdk).is_err(), "accepted sdk={sdk}: {body}");
        }
    }
}
