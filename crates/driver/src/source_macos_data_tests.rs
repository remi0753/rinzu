#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod fixture {
    include!("../tests/fixtures/macos_native_data/setup.rs");
}

#[test]
fn macos_native_output_records_match_all_c_layouts() {
    let sdk = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../sdk/macos");
    let (manifest, artifact) = rinzu_compiler::check_package(&sdk).unwrap();
    let source = artifact.source_interface.as_ref().unwrap();
    let ast = rinzu_compiler::parse_source(&rinzu_compiler::SourceFile::new(
        rinzu_compiler::FileId(1),
        "macos.rinzu",
        std::fs::read_to_string(sdk.join("src/macos.rinzu")).unwrap(),
    ))
    .unwrap();
    for name in [
        "Session",
        "Window",
        "View",
        "Device",
        "Buffer",
        "Texture",
        "Sampler",
        "Font",
        "TextLayout",
        "FrameClock",
        "Library",
        "ComputePipeline",
        "RenderPipeline",
        "Command",
        "Menu",
    ] {
        assert!(ast.declarations.iter().any(|declaration| matches!(declaration,rinzu_compiler::ast::Declaration::Record(record) if record.name==name && record.mutable)));
        assert!(ast.native_blocks.iter().flat_map(|block| &block.members).any(|member| matches!(member,rinzu_compiler::ast::NativeMember::Opaque{name:handle,..} if handle==&format!("{name}Handle"))));
        assert!(!ast.declarations.iter().any(|declaration| matches!(declaration,rinzu_compiler::ast::Declaration::Function(method) if method.lexical_owner.as_deref()==Some(&format!("{name}Handle")))));
    }
    let run = source
        .signatures
        .iter()
        .find(|signature| signature.name.ends_with("rawRun"))
        .unwrap();
    assert!(
        matches!(&run.parameters[0],rinzu_compiler::SourceAbiType::Opaque(name) if name.ends_with("SessionHandle"))
    );
    assert!(matches!(
        &run.parameters[1],
        rinzu_compiler::SourceAbiType::UserData(_)
    ));
    let rinzu_compiler::SourceAbiType::Callback { parameters, .. } = &run.parameters[2] else {
        panic!("missing run callback")
    };
    assert_eq!(parameters[0], run.parameters[1]);
    let native_names = ast
        .native_blocks
        .iter()
        .flat_map(|block| &block.members)
        .filter_map(|member| match member {
            rinzu_compiler::ast::NativeMember::Record(record) => Some(record.name.as_str()),
            rinzu_compiler::ast::NativeMember::Opaque { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert!(!ast.declarations.iter().any(|declaration| matches!(declaration,rinzu_compiler::ast::Declaration::Function(method) if method.lexical_owner.as_deref().is_some_and(|owner|native_names.contains(owner)))));
    assert!(source.modules["rinzu.macos"].contains_key("compose2D"));
    let declared_records = ast
        .native_blocks
        .iter()
        .flat_map(|block| &block.members)
        .filter_map(|member| match member {
            rinzu_compiler::ast::NativeMember::Record(record) => Some(record.name.as_str()),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    let checked_records = source
        .native_records
        .keys()
        .map(|identity| identity.rsplit("::").next().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(checked_records, declared_records);
    let statistics = source
        .signatures
        .iter()
        .find(|signature| signature.name.ends_with("rawStatistics"))
        .unwrap();
    assert!(
        matches!(&statistics.parameters[0], rinzu_compiler::SourceAbiType::Record(name) if name.ends_with("Statistics"))
    );
    let directory =
        std::env::temp_dir().join(format!("rinzu-macos-record-layout-{}", std::process::id()));
    let contract = crate::native_contract::validate_package(&manifest, source, &directory).unwrap();
    for (name, size, alignment) in [
        ("Transform2D", 48, 8),
        ("FrameStatistics", 40, 8),
        ("WidgetState", 16, 8),
        ("Event", 144, 8),
    ] {
        let layout = contract
            .records
            .iter()
            .find(|(identity, _)| identity.ends_with(name))
            .unwrap()
            .1;
        assert_eq!((layout.size, layout.alignment), (size, alignment), "{name}");
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn native_macos_rotation_writes_back_without_storage_or_snapshots() {
    let root = std::env::temp_dir().join(format!("rinzu-macos-native-math-{}", std::process::id()));
    fixture::create_math(&root);
    let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    let jit = rinzu_codegen::run_baseline_jit_with_package_bindings(
        &artifact.ir,
        "execute",
        rinzu_codegen::BaselineOptions {
            collect_every_allocation: true,
            ..Default::default()
        },
        None,
        Some(built.load_source_packages().unwrap()),
    )
    .unwrap();
    assert_eq!(jit.result, Ok(42), "{jit:?}");
    assert!(jit.native_calls.is_baseline());
    assert_eq!(jit.heap_objects, 0);
    let run = rinzu_codegen::run_source_async_report(
        &artifact.ir,
        "main",
        rinzu_codegen::SourceAsyncOptions {
            source_packages: Some(built.load_source_packages().unwrap()),
            capture_stdout: true,
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        run.failure_details.is_none() && run.resources_returned && run.native_calls.is_baseline(),
        "{run:?}"
    );
    assert_eq!(run.stdout, b"42\n");
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
    std::fs::remove_dir_all(root).unwrap();
}
