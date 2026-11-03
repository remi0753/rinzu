use rinzu_runtime::key::{KeyHasher, string_hash};
use rinzu_runtime::scalar::{ScalarError, ScalarKind, ScalarValue};
use std::process::Command;

#[test]
fn integer_and_string_key_hashes_match_c_and_remain_stable_through_moving_gc() {
    let root = std::env::temp_dir().join(format!("rinzu-key-c-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let mut program = String::from(
        "#include <assert.h>\n#include <stdio.h>\n#include \"rinzu/managed_data.h\"\nint main(void) { rinzu_managed_data_t data = {0}; rinzu_managed_roots_t roots = {0}; rinzu_managed_enter(&roots);\n",
    );
    let mut expected = Vec::new();
    for kind in [
        ScalarKind::I8,
        ScalarKind::I16,
        ScalarKind::I32,
        ScalarKind::I64,
        ScalarKind::U8,
        ScalarKind::U16,
        ScalarKind::U32,
        ScalarKind::U64,
    ] {
        let (minimum, maximum) = kind.bounds().unwrap();
        for integer in [minimum, 0, 1, maximum] {
            let value = ScalarValue::from_integer(kind, integer).unwrap();
            let hash = value.key().unwrap().stable_hash();
            expected.push(hash);
            assert_eq!(
                ScalarValue::from_physical_bits(kind, value.bits())
                    .unwrap()
                    .key()
                    .unwrap()
                    .stable_hash(),
                hash
            );
            program.push_str(&format!("{{ rinzu_key_hasher_t hash = rinzu_key_init(); assert(!rinzu_key_integer(&hash, {}, UINT64_C({}))); printf(\"%llu\\n\", (unsigned long long)hash.hash); }}\n", kind as u32, value.bits()));
        }
    }
    for value in [
        ScalarValue::from_f32(-0.0),
        ScalarValue::from_f64(f64::NAN),
        ScalarValue::from_bits(ScalarKind::Bool, 1).unwrap(),
    ] {
        assert!(value.key().is_none());
        let mut hash = KeyHasher::default();
        let unchanged = hash.finish();
        assert_eq!(hash.integer(value), Err(ScalarError::TypeMismatch));
        assert_eq!(hash.finish(), unchanged);
        program.push_str(&format!("{{ rinzu_key_hasher_t hash = rinzu_key_init(); uint64_t unchanged = hash.hash; assert(rinzu_key_integer(&hash, {}, UINT64_C({}))); assert(hash.hash == unchanged); }}\n", value.kind() as u32, value.bits()));
    }
    for (kind, bits) in [(ScalarKind::I8, 255), (ScalarKind::U8, 256)] {
        assert!(ScalarValue::from_bits(kind, bits).is_err());
        program.push_str(&format!("{{ rinzu_key_hasher_t hash = rinzu_key_init(); uint64_t unchanged = hash.hash; assert(rinzu_key_integer(&hash, {}, UINT64_C({bits}))); assert(hash.hash == unchanged); }}\n", kind as u32));
    }
    for text in [
        "",
        "12345678",
        "123456789",
        "日本語🙂\0tail",
        "e\u{301}",
        "é",
    ] {
        expected.push(string_hash(text));
        let bytes = if text.is_empty() {
            "0".to_owned()
        } else {
            text.as_bytes()
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(",")
        };
        program.push_str(&format!("{{ const unsigned char bytes[] = {{{bytes}}}; uint64_t one = rinzu_managed_string(&data, bytes, {}), two = rinzu_managed_string(&data, bytes, {}); assert(one != two && rinzu_managed_string_equal(&data, one, two)); uint64_t hash = rinzu_managed_string_hash(&data, one); assert(hash == rinzu_managed_string_hash(&data, two)); uint64_t live[] = {{one, two}}; rinzu_managed_publish(&roots, live, 2); for (int i = 0; i < 9; ++i) {{ rinzu_managed_collect(&data, &roots); assert(rinzu_managed_string_equal(&data, one, two)); assert(hash == rinzu_managed_string_hash(&data, one)); assert(hash == rinzu_managed_string_hash(&data, two)); }} printf(\"%llu\\n\", (unsigned long long)hash); }}\n", text.len(), text.len()));
    }
    assert_ne!(string_hash("e\u{301}"), string_hash("é"));
    assert_ne!(
        ScalarValue::from_integer(ScalarKind::U64, 1)
            .unwrap()
            .key()
            .unwrap()
            .stable_hash(),
        ScalarValue::from_integer(ScalarKind::I64, 1)
            .unwrap()
            .key()
            .unwrap()
            .stable_hash()
    );
    program.push_str("rinzu_managed_publish(&roots, NULL, 0); rinzu_managed_collect(&data, &roots); assert(!data.objects); rinzu_managed_leave(&roots); rinzu_managed_dispose(&roots); rinzu_managed_data_dispose(&data); return 0; }\n");
    std::fs::write(root.join("test.c"), program).unwrap();
    let compile = Command::new("clang")
        .args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-I"])
        .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
        .arg(root.join("test.c"))
        .arg("-o")
        .arg(root.join("test"))
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(root.join("test")).output().unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let actual = String::from_utf8(run.stdout)
        .unwrap()
        .lines()
        .map(|line| line.parse::<u64>().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
    std::fs::remove_dir_all(root).unwrap();
}
