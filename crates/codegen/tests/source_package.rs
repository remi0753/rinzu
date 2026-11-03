#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::ffi::{CString, c_void};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use rinzu_abi::{SourceImportV2, SourcePackageDescriptorV2};
use rinzu_bindgen::source_package::*;
use rinzu_codegen::source_package::*;

fn interface() -> SourceInterface {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/source-package/interface.json"
    ))
    .unwrap()
}

struct Fixture {
    dir: PathBuf,
    artifact: SourcePackageArtifact,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).unwrap();
    }
}
fn fixture(
    interface: SourceInterface,
    change: impl FnOnce(&mut GeneratedSourcePackage),
) -> Fixture {
    static SERIAL: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "rinzu-source-load-{}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let mut generated = generate_source_package(interface, b"native input").unwrap();
    let expected_wire = generated.wire.clone();
    change(&mut generated);
    std::fs::write(dir.join("source-package.h"), &generated.header).unwrap();
    std::fs::write(dir.join("table.c"), &generated.c_table).unwrap();
    std::fs::write(
        dir.join("math.c"),
        "#include <stdint.h>\nint64_t add(int64_t a,int64_t b){return a+b;}\n",
    )
    .unwrap();
    let library = dir.join("facade.dylib");
    let include = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include");
    let result = std::process::Command::new("clang")
        .args(["-Wall", "-Wextra", "-Werror", "-dynamiclib", "-I"])
        .arg(include)
        .arg(dir.join("table.c"))
        .arg(dir.join("math.c"))
        .arg("-o")
        .arg(&library)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let artifact = SourcePackageArtifact {
        facade_digest: Some(sha256(&std::fs::read(&library).unwrap())),
        library: Some(library),
        expected_wire,
    };
    Fixture { dir, artifact }
}
fn replace_wire(generated: &mut GeneratedSourcePackage) {
    let wire = serde_json::to_vec(&generated.descriptor).unwrap();
    let numbers = |bytes: &[u8]| {
        bytes
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(",")
    };
    generated.c_table = generated
        .c_table
        .replace(&numbers(&generated.wire), &numbers(&wire));
}

#[test]
fn actual_facade_load_and_process_lifetime_pin() {
    let fixture = fixture(interface(), |_| {});
    let loaded = LoadedSourcePackages::load(std::slice::from_ref(&fixture.artifact)).unwrap();
    let import = loaded.imports().values().next().unwrap();
    assert_eq!(import.identity, "spike.math.add");
    // SAFETY: generated interface and contract header validate this exact C prototype.
    let add: unsafe extern "C" fn(i64, i64) -> i64 =
        unsafe { core::mem::transmute(import.glue_address()) };
    drop(loaded);
    // SAFETY: successful facade remains pinned for process lifetime, even after Drop.
    assert_eq!(unsafe { add(17, -5) }, 12);
}

#[test]
fn rejects_wire_contract_drift_before_publishing() {
    for (name, mutate) in [
        (
            "capability",
            (|d: &mut SourceDescriptor| d.interface.backend.owner_dispatcher = "unknown".into())
                as fn(&mut SourceDescriptor),
        ),
        ("callback-version", |d: &mut SourceDescriptor| {
            d.interface.backend.callback_rule = 2
        }),
        ("template-version", |d: &mut SourceDescriptor| {
            d.interface.backend.template_layout = 2
        }),
        ("service-version", |d: &mut SourceDescriptor| {
            d.interface.backend.service_registry = 2
        }),
        ("major", |d: &mut SourceDescriptor| d.major = 1),
        ("minor", |d: &mut SourceDescriptor| d.minor = 0),
        ("target", |d: &mut SourceDescriptor| {
            d.interface.package.target = "x86_64-apple-darwin".into()
        }),
        ("digest", |d: &mut SourceDescriptor| {
            d.semantic_digest = sha256(b"wrong")
        }),
        ("artifact-digest", |d: &mut SourceDescriptor| {
            d.native_artifact_digest = sha256(b"wrong")
        }),
        ("header", |d: &mut SourceDescriptor| {
            d.generated_header_digest = sha256(b"wrong")
        }),
        ("key", |d: &mut SourceDescriptor| {
            d.import_keys[0] = sha256(b"wrong")
        }),
        ("layout", |d: &mut SourceDescriptor| {
            d.interface.types[0].alignment = "3".into()
        }),
    ] {
        let fixture = fixture(interface(), |g| {
            mutate(&mut g.descriptor);
            replace_wire(g);
        });
        assert!(
            LoadedSourcePackages::load(std::slice::from_ref(&fixture.artifact)).is_err(),
            "{name}"
        );
    }
}

#[test]
fn rejects_table_symbols_version_and_legacy_entry() {
    for (from, to) in [
        (
            "\"add\", (const void *)&add",
            "\"absent\", (const void *)&add",
        ),
        ("\"add\", (const void *)&add", "\"add\", NULL"),
        (
            "\"add\", (const void *)&add",
            "\"add\", (const void *)&puts",
        ),
        (
            "sizeof(package), RINZU_SOURCE_PACKAGE_MAJOR, RINZU_SOURCE_PACKAGE_MINOR",
            "sizeof(package), 1, 0",
        ),
        (
            "sizeof(package), RINZU_SOURCE_PACKAGE_MAJOR, RINZU_SOURCE_PACKAGE_MINOR",
            "sizeof(package), 2, 2",
        ),
        (
            "sizeof(package), RINZU_SOURCE_PACKAGE_MAJOR, RINZU_SOURCE_PACKAGE_MINOR",
            "sizeof(package), 2, 3",
        ),
        (
            "sizeof(package), RINZU_SOURCE_PACKAGE_MAJOR, RINZU_SOURCE_PACKAGE_MINOR",
            "8, 2, 1",
        ),
        (
            "rinzu_source_package_v2(void)",
            "rinzu_native_package_v1(void)",
        ),
    ] {
        let fixture = fixture(interface(), |g| {
            assert!(g.c_table.contains(from));
            g.c_table = g.c_table.replace(from, to);
            if to.contains("puts") {
                g.c_table.insert_str(0, "#include <stdio.h>\n");
            }
        });
        assert!(
            LoadedSourcePackages::load(std::slice::from_ref(&fixture.artifact)).is_err(),
            "{to}"
        );
    }
    let mut fixture = fixture(interface(), |_| {});
    fixture.artifact.facade_digest = Some(sha256(b"wrong"));
    assert!(LoadedSourcePackages::load(std::slice::from_ref(&fixture.artifact)).is_err());
}

#[test]
fn rejects_entire_load_transaction_and_duplicate_packages() {
    let good = fixture(interface(), |_| {});
    let mut second = interface();
    second.package.package = "spike.second".into();
    second.functions[0].identity = "spike.second.add".into();
    let bad = fixture(second, |g| {
        g.c_table = g.c_table.replace(
            "\"add\", (const void *)&add",
            "\"absent\", (const void *)&add",
        );
    });
    assert!(LoadedSourcePackages::load(&[good.artifact.clone(), bad.artifact.clone()]).is_err());
    assert!(LoadedSourcePackages::load(&[good.artifact.clone(), good.artifact.clone()]).is_err());
    assert_eq!(
        LoadedSourcePackages::load(std::slice::from_ref(&good.artifact))
            .unwrap()
            .imports()
            .len(),
        1
    );
}

#[test]
fn aot_uses_same_table_validation_and_rejects_duplicate_entries() {
    let generated = generate_source_package(interface(), b"input").unwrap();
    let symbol = CString::new("add").unwrap();
    extern "C" fn add(a: i64, b: i64) -> i64 {
        a + b
    }
    let mut entries = vec![SourceImportV2 {
        import_key: decode_key(&generated.descriptor.import_keys[0]).unwrap(),
        symbol: symbol.as_ptr(),
        address: add as *const c_void,
    }];
    let mut table = SourcePackageDescriptorV2 {
        struct_size: size_of::<SourcePackageDescriptorV2>() as u32,
        major: 2,
        minor: rinzu_abi::SOURCE_PACKAGE_MINOR,
        descriptor: generated.wire.as_ptr(),
        descriptor_len: generated.wire.len(),
        import_count: 1,
        imports: entries.as_ptr(),
    };
    // SAFETY: local table, wire, entries and strings stay live for validation.
    let loaded =
        unsafe { LoadedSourcePackages::from_aot_tables(&[(&table, &generated.wire)]) }.unwrap();
    assert_eq!(loaded.imports().len(), 1);
    entries.push(entries[0]);
    table.import_count = 2;
    table.imports = entries.as_ptr();
    // SAFETY: both duplicate entries and backing storage are valid and readable.
    let rejected = unsafe { LoadedSourcePackages::from_aot_tables(&[(&table, &generated.wire)]) };
    assert!(rejected.is_err());
    table.import_count = 1;
    entries[0].import_key = [0; 32];
    // SAFETY: mutated local storage remains readable.
    assert!(unsafe { validate_source_table(&table, &generated.wire) }.is_err());
}

#[test]
fn closed_service_interface_needs_no_artifact() {
    let mut interface = interface();
    let function = &mut interface.functions[0];
    function.identity = "spike.timer".into();
    function.symbol = None;
    function.service = Some("reactor.timer".into());
    function.kind = FunctionKind::Async;
    function.source_type = "async(Int)->Unit".into();
    function.c_prototype.clear();
    let generated = generate_source_package(interface.clone(), b"").unwrap();
    let artifact = SourcePackageArtifact {
        library: None,
        facade_digest: None,
        expected_wire: generated.wire,
    };
    let loaded = LoadedSourcePackages::load(&[artifact]).unwrap();
    assert!(loaded.imports().is_empty());
    assert_eq!(loaded.descriptors().len(), 1);
    interface.functions[0].service = Some("custom.service".into());
    assert!(generate_source_package(interface, b"").is_err());
}
