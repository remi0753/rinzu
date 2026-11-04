use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use rinzu_compiler::{
    FileId, SourceCache, SourceFile, SourceRange, SourceSnapshot, Utf16Position,
    check_package_with_sources,
};

struct Fixture(PathBuf);

#[test]
fn interface_constraints_and_error_contracts_navigate_to_source_declarations() {
    let fixture = Fixture::new();
    let text = "error Failed{}interface Getter<T>{fn get(self:Self)->T throws Failed;}class Value interface Getter<Int>{fn get(self:Value)->Int nothrow{return 42;}}fn read<V interface Getter<Int>>(v:V)->Int throws Failed{return v.get();}fn main(){try{let result=read(Value());}catch problems{}}";
    let entry = fixture.source("main.rinzu", text);
    let source = SourceFile::new(FileId(902), &entry, text);
    let artifact = rinzu_compiler::compile_source(&source).unwrap();
    let sources = BTreeMap::from([(entry, source.clone())]);
    let index = rinzu_compiler::build_editor_index(
        &sources,
        &[(source.clone(), artifact.syntax.clone())],
        &[&artifact.syntax, &artifact.checked.syntax],
    );
    for (usage, definition) in [
        ("interface Getter<Int>", "interface Getter<T>"),
        ("throws Failed", "error Failed"),
    ] {
        let name = if usage.starts_with("interface") {
            "Getter"
        } else {
            "Failed"
        };
        let at = text.find(usage).unwrap() + usage.find(name).unwrap();
        let expected = text.find(definition).unwrap() + definition.find(name).unwrap();
        assert_eq!(
            index
                .definition_at(SourceRange::empty(source.id(), at))
                .unwrap()
                .start_byte as usize,
            expected
        );
    }
    assert!(
        index
            .visible_symbols(SourceRange::empty(source.id(), text.len()))
            .iter()
            .any(|s| s.name == "Getter" && s.kind == "interface")
    );
    let docs = rinzu_compiler::source_documentation(&source).unwrap();
    let interface = docs.iter().find(|d| d.name == "Getter").unwrap();
    assert_eq!(interface.kind, "interface");
    assert!(interface.members[0].signature.ends_with("throws Failed;"));
}

#[test]
fn native_roles_navigate_and_invalid_unsaved_members_are_rejected() {
    let fixture = Fixture::new();
    let text = "native{opaque Handle;record Packet{value:Int}}class Owner{const handle:Handle,const packet:Packet fn read(self:Owner)->Int{return self.packet.value;}}type Context={owner:Owner};fn main(){}";
    let entry = fixture.source("main.rinzu", text);
    let source = SourceFile::new(FileId(901), &entry, text);
    let artifact = rinzu_compiler::compile_source(&source).unwrap();
    let sources = BTreeMap::from([(entry.clone(), source.clone())]);
    let index = rinzu_compiler::build_editor_index(
        &sources,
        &[(source.clone(), artifact.syntax.clone())],
        &[&artifact.syntax, &artifact.checked.syntax],
    );
    for (usage, definition) in [
        ("handle:Handle", "opaque Handle"),
        ("packet:Packet", "record Packet"),
    ] {
        let use_at = text.find(usage).unwrap() + usage.find(':').unwrap() + 1;
        let expected = text.find(definition).unwrap() + definition.find(' ').unwrap() + 1;
        assert_eq!(
            index
                .definition_at(SourceRange::empty(source.id(), use_at))
                .unwrap()
                .start_byte as usize,
            expected
        );
    }
    let visible = index.visible_symbols(SourceRange::empty(source.id(), text.len()));
    assert!(visible.iter().any(|s| s.name == "Owner.read"));
    assert!(
        !visible
            .iter()
            .any(|s| s.name.starts_with("Handle.") || s.name.starts_with("Packet."))
    );
    let docs = rinzu_compiler::source_documentation(&source).unwrap();
    for (name, kind) in [
        ("Handle", "opaque"),
        ("Packet", "native-record"),
        ("Owner", "class"),
        ("Context", "type-alias"),
    ] {
        assert!(
            docs.iter().any(|d| d.name == name && d.kind == kind),
            "{name}: {docs:#?}"
        );
    }
    let mut cache = SourceCache::default();
    for invalid in [
        "native{userdata Context;}",
        "fn Handle.read()->Int{return 1;}",
        "type Alias=Packet;private async fn Alias.read()->Int{return 1;}",
    ] {
        let mut snapshot = SourceSnapshot::new(
            &mut cache,
            BTreeMap::from([(entry.clone(), format!("{text}{invalid}").into_bytes())]),
        );
        assert!(
            check_package_with_sources(&fixture.0, &mut snapshot).is_err(),
            "{invalid}"
        );
    }
    assert_eq!(std::fs::read_to_string(entry).unwrap(), text);
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "rinzu-source-editor-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("rinzu.toml"),
            "[package]\nname=\"edit\"\nlanguage=\"0.5\"\n",
        )
        .unwrap();
        Self(root.canonicalize().unwrap())
    }
    fn source(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join("src").join(name);
        std::fs::write(&path, text).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn snapshot_detects_same_bytes_symlink_resolution_race() {
    let fixture = Fixture::new();
    let first = fixture.0.join("src/first.rinzu");
    let second = fixture.0.join("src/second.rinzu");
    let text = rinzu_compiler::format_source(&SourceFile::new(
        FileId(1),
        &first,
        "fn value()->Int{return 42;}export value;",
    ))
    .unwrap();
    std::fs::write(&first, &text).unwrap();
    std::fs::write(&second, &text).unwrap();
    let alias = fixture.0.join("src/alias.rinzu");
    std::os::unix::fs::symlink(&first, &alias).unwrap();
    let mut cache = SourceCache::default();
    let mut snapshot = SourceSnapshot::new(&mut cache, BTreeMap::new());
    let resolved = snapshot.canonical_path(&alias).unwrap();
    snapshot.read_bytes(&resolved).unwrap();
    assert!(snapshot.disk_is_current());
    std::fs::remove_file(&alias).unwrap();
    std::os::unix::fs::symlink(&second, &alias).unwrap();
    assert!(!snapshot.disk_is_current());
    assert_eq!(snapshot.canonical_path(&alias).unwrap(), second);
    assert!(!snapshot.disk_is_current());
}

#[test]
fn unicode_bom_crlf_and_non_bmp_positions_are_explicit_and_checked() {
    let text = "A😀日本\r\nβ";
    for bom in [false, true] {
        let mut bytes = Vec::new();
        if bom {
            bytes.extend_from_slice(&[0xef, 0xbb, 0xbf]);
        }
        bytes.extend_from_slice(text.as_bytes());
        let source = SourceFile::from_utf8(FileId(1), "位置.rinzu", bytes).unwrap();
        let prefix = if bom { 3 } else { 0 };
        for (byte, line, character) in [
            (0, 0, 0),
            (1, 0, 1),
            (5, 0, 3),
            (8, 0, 4),
            (11, 0, 5),
            (13, 1, 0),
            (15, 1, 1),
        ] {
            let position = Utf16Position { line, character };
            assert_eq!(source.utf16_position(prefix + byte).unwrap(), position);
            assert_eq!(
                source.original_byte_at_utf16(position).unwrap(),
                prefix + byte
            );
        }
        assert_eq!(
            source.utf16_position(prefix + 12).unwrap(),
            Utf16Position {
                line: 0,
                character: 5
            }
        );
        for byte in [2, 3, 4, 16] {
            assert!(source.utf16_position(prefix + byte).is_err());
        }
        for position in [
            Utf16Position {
                line: 0,
                character: 2,
            },
            Utf16Position {
                line: 0,
                character: 6,
            },
            Utf16Position {
                line: 2,
                character: 0,
            },
        ] {
            assert!(source.original_byte_at_utf16(position).is_err());
        }
        if bom {
            assert!(source.utf16_position(1).is_err());
        }
    }
    let source = SourceFile::new(FileId(2), "combining", "e\u{301}\0\n");
    assert_eq!(
        source
            .original_byte_at_utf16(Utf16Position {
                line: 0,
                character: 2
            })
            .unwrap(),
        3
    );
    assert_eq!(
        source
            .original_byte_at_utf16(Utf16Position {
                line: 1,
                character: 0
            })
            .unwrap(),
        5
    );
    let errors =
        SourceFile::from_utf8(FileId(3), "bad", vec![0xef, 0xbb, 0xbf, b'a', 0xff]).unwrap_err();
    assert_eq!(errors[0].primary.start_byte, 4);
}

#[test]
fn scalar_line_columns_clamp_arbitrary_offsets_to_character_starts() {
    let text = "A😀日\r\nβ";
    for bom in [false, true] {
        let mut bytes = if bom {
            vec![0xef, 0xbb, 0xbf]
        } else {
            Vec::new()
        };
        bytes.extend_from_slice(text.as_bytes());
        let file = SourceFile::from_utf8(FileId(1), "positions.rinzu", bytes).unwrap();
        let prefix = file.original_offset(0);
        for byte in 0..prefix {
            assert_eq!(file.line_column(byte), (1, 1));
        }
        for (byte, expected) in [
            (0, (1, 1)),
            (1, (1, 2)),
            (2, (1, 2)),
            (3, (1, 2)),
            (4, (1, 2)),
            (5, (1, 3)),
            (6, (1, 3)),
            (7, (1, 3)),
            (8, (1, 4)),
            (9, (1, 5)),
            (10, (2, 1)),
            (11, (2, 1)),
            (12, (2, 2)),
            (usize::MAX - prefix, (2, 2)),
        ] {
            assert_eq!(file.line_column(prefix + byte), expected, "byte {byte}");
        }
    }
}

#[test]
fn unsaved_dependency_changes_typecheck_and_parse_cache_without_writing_disk() {
    let fixture = Fixture::new();
    fixture.source(
        "main.rinzu",
        "import model;fn main(){let value:Int=model.value();}",
    );
    let dependency = fixture.source(
        "model.rinzu",
        "fn value()->String{return \"old\";}export value;",
    );
    let mut cache = SourceCache::default();
    let mut disk = SourceSnapshot::new(&mut cache, BTreeMap::new());
    assert!(check_package_with_sources(&fixture.0, &mut disk).is_err());
    drop(disk);
    let overlays = BTreeMap::from([(
        dependency.clone(),
        b"fn value()->Int{return 42;}export value;".to_vec(),
    )]);
    let mut snapshot = SourceSnapshot::new(&mut cache, overlays.clone());
    let (_, artifact) = check_package_with_sources(&fixture.0, &mut snapshot).unwrap();
    assert!(
        artifact
            .checked
            .functions
            .iter()
            .any(|function| function.declaration.name.contains("value"))
    );
    assert!(snapshot.parse_hits > 0 && snapshot.parse_misses > 0);
    let ids: Vec<_> = snapshot.sources().values().map(SourceFile::id).collect();
    assert!(snapshot.disk_is_current());
    drop(snapshot);
    let mut again = SourceSnapshot::new(&mut cache, overlays);
    check_package_with_sources(&fixture.0, &mut again).unwrap();
    assert_eq!(again.parse_misses, 0);
    assert_eq!(
        again
            .sources()
            .values()
            .map(SourceFile::id)
            .collect::<Vec<_>>(),
        ids
    );
    assert_eq!(
        std::fs::read_to_string(dependency).unwrap(),
        "fn value()->String{return \"old\";}export value;"
    );
}

#[test]
fn recursive_source_sdk_uses_the_same_overlay_snapshot() {
    let fixture = Fixture::new();
    std::fs::create_dir_all(fixture.0.join("sdk/src")).unwrap();
    std::fs::write(
        fixture.0.join("sdk/rinzu.toml"),
        "[package]\nname=\"model\"\nlanguage=\"0.5\"\nkind=\"sdk\"\nentry=\"src/model.rinzu\"\n",
    )
    .unwrap();
    let sdk = fixture.0.join("sdk/src/model.rinzu");
    std::fs::write(&sdk, "fn value()->String{return \"old\";}export value;").unwrap();
    std::fs::write(
        fixture.0.join("rinzu.toml"),
        "[package]\nname=\"edit\"\nlanguage=\"0.5\"\n[dependencies]\nmodel={path=\"sdk\"}\n",
    )
    .unwrap();
    fixture.source(
        "main.rinzu",
        "import model;fn main(){let value:Int=model.value();}",
    );
    let mut cache = SourceCache::default();
    let mut snapshot = SourceSnapshot::new(
        &mut cache,
        BTreeMap::from([(
            sdk.clone(),
            b"fn value()->Int{return 7;}export value;".to_vec(),
        )]),
    );
    check_package_with_sources(&fixture.0, &mut snapshot).unwrap();
    assert!(
        snapshot
            .parsed_modules()
            .iter()
            .any(|(source, _)| source.path() == sdk)
    );
    assert_eq!(
        std::fs::read_to_string(sdk).unwrap(),
        "fn value()->String{return \"old\";}export value;"
    );
}

#[test]
fn invalid_overlay_recovers_and_virtual_imported_file_has_no_disk_artifact() {
    let fixture = Fixture::new();
    fixture.source("main.rinzu", "import fresh;fn main(){let n=fresh.value();}");
    let fresh = fixture.0.join("src/fresh.rinzu");
    let mut cache = SourceCache::default();
    let mut broken = SourceSnapshot::new(
        &mut cache,
        BTreeMap::from([(fresh.clone(), b"fn value( {".to_vec())]),
    );
    assert!(check_package_with_sources(&fixture.0, &mut broken).is_err());
    drop(broken);
    let mut repaired = SourceSnapshot::new(
        &mut cache,
        BTreeMap::from([(
            fresh.clone(),
            b"fn value()->Int{return 7;}export value;".to_vec(),
        )]),
    );
    check_package_with_sources(&fixture.0, &mut repaired).unwrap();
    assert!(!fresh.exists());
    assert!(repaired.disk_is_current());
    std::fs::write(fixture.0.join("src/main.rinzu"), "fn main(){}").unwrap();
    assert!(!repaired.disk_is_current());
}

#[test]
fn lexical_shadowing_captures_and_generic_names_navigate_to_source_tokens() {
    let text = "fn pick<T>(input:T)->T{return input;}fn value()->Int{return 99;}fn use()->Int{let value:Int=7;let closure=fn(value:Int)->Int{return value+1;};let captured=fn()->Int{return value;};return pick<Int>(closure(captured()));}// value\n";
    let source = SourceFile::new(FileId(900), "index.rinzu", text);
    let artifact = rinzu_compiler::compile_source_with_options(
        &source,
        &rinzu_compiler::CompilerOptions {
            is_library: true,
            ..Default::default()
        },
    )
    .unwrap();
    let sources = BTreeMap::from([(source.path().to_owned(), source.clone())]);
    let index = rinzu_compiler::build_editor_index(
        &sources,
        &[(source.clone(), artifact.syntax.clone())],
        &[&artifact.syntax, &artifact.checked.syntax],
    );
    let point = |byte| SourceRange::empty(source.id(), byte);
    let closure_argument = text.find("fn(value:Int)").unwrap() + 3;
    let closure_use = text.find("return value+1").unwrap() + 7;
    assert_eq!(
        index.definition_at(point(closure_use)).unwrap().start_byte as usize,
        closure_argument
    );
    let capture_use = text.find("return value;").unwrap() + 7;
    let local = text.find("let value:Int").unwrap() + 4;
    assert_eq!(
        index.definition_at(point(capture_use)).unwrap().start_byte as usize,
        local
    );
    let generic_use = text.find("pick<Int>").unwrap();
    assert_eq!(
        index.definition_at(point(generic_use)).unwrap().start_byte,
        3
    );
    let param_use = text.find("return input").unwrap() + 7;
    assert_eq!(
        index.definition_at(point(param_use)).unwrap().start_byte as usize,
        text.find("input:T").unwrap()
    );
    assert!(
        index
            .definition_at(point(text.find("// value").unwrap() + 3))
            .is_none()
    );
    assert!(
        index
            .visible_symbols(point(text.len()))
            .iter()
            .any(|s| s.name == "pick")
    );
    assert!(
        !index
            .visible_symbols(point(text.len()))
            .iter()
            .any(|s| s.name == "closure")
    );
}

#[test]
fn linked_same_named_functions_and_types_resolve_to_different_modules() {
    let fixture = Fixture::new();
    let main=fixture.source("main.rinzu","import one;import two;fn main(){let a=one.value();let b=two.value();let c:one.Box=one.Box(value:a);}");
    let one = fixture.source(
        "one.rinzu",
        "class Box{value:Int}fn value()->Int{return 1;}export value;export Box;",
    );
    let two = fixture.source("two.rinzu", "fn value()->Int{return 2;}export value;");
    let mut cache = SourceCache::default();
    let mut snapshot = SourceSnapshot::new(&mut cache, BTreeMap::new());
    let (_, a) = check_package_with_sources(&fixture.0, &mut snapshot).unwrap();
    let index = rinzu_compiler::build_editor_index(
        snapshot.sources(),
        &snapshot.parsed_modules(),
        &[&a.syntax, &a.checked.syntax],
    );
    let source = &snapshot.sources()[&main];
    for (needle, target) in [("one.value", &one), ("two.value", &two), ("one.Box=", &one)] {
        let byte = source.text().find(needle).unwrap() + 4;
        let definition = index
            .definition_at(SourceRange::empty(source.id(), byte))
            .unwrap();
        assert_eq!(definition.file_id, snapshot.sources()[target].id());
    }
}

#[test]
fn snapshot_pins_recursive_manifests_and_reports_configuration_races() {
    let fixture = Fixture::new();
    fixture.source("main.rinzu", "fn main(){}");
    let mut cache = SourceCache::default();
    let mut snapshot = SourceSnapshot::new(&mut cache, BTreeMap::new());
    check_package_with_sources(&fixture.0, &mut snapshot).unwrap();
    assert!(
        snapshot
            .input_receipts()
            .iter()
            .any(|(_, kind, _, _)| *kind == "disk-manifest")
    );
    std::fs::write(
        fixture.0.join("rinzu.toml"),
        "[package]\nname=\"changed\"\nlanguage=\"0.5\"\n",
    )
    .unwrap();
    assert!(!snapshot.disk_is_current());
    let (manifest, _) = check_package_with_sources(&fixture.0, &mut snapshot).unwrap();
    assert_eq!(manifest.name, "edit");
}
