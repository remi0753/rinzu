use rinzu_runtime::scalar::{
    Arithmetic as A, Comparison as C, ScalarError as E, ScalarKind as K, ScalarValue as V,
};

#[test]
fn integer_widths_and_full_unsigned_range_use_checked_value_arithmetic() {
    for kind in [K::I8, K::I16, K::I32, K::I64, K::U8, K::U16, K::U32, K::U64] {
        let (minimum, maximum) = kind.bounds().unwrap();
        let min = V::from_integer(kind, minimum).unwrap();
        let max = V::from_integer(kind, maximum).unwrap();
        let one = V::from_integer(kind, 1).unwrap();
        let zero = V::from_integer(kind, 0).unwrap();
        assert_eq!(max.arithmetic(A::Add, one), Err(E::Arithmetic), "{kind:?}");
        assert_eq!(
            min.arithmetic(A::Subtract, one),
            Err(E::Arithmetic),
            "{kind:?}"
        );
        assert_eq!(
            max.arithmetic(A::Multiply, max),
            Err(E::Arithmetic),
            "{kind:?}"
        );
        assert_eq!(max.arithmetic(A::Divide, zero), Err(E::Arithmetic));
        assert_eq!(max.arithmetic(A::Remainder, zero), Err(E::Arithmetic));
        assert!(max.compare(C::Greater, min).unwrap());
        assert_eq!(max.bits(), maximum as u64);
        assert_eq!(
            max.arithmetic(A::Subtract, one).unwrap().integer(),
            Some(maximum - 1)
        );
        if kind.signed() {
            let minus_one = V::from_integer(kind, -1).unwrap();
            assert_eq!(min.arithmetic(A::Divide, minus_one), Err(E::Arithmetic));
            assert_eq!(min.arithmetic(A::Remainder, minus_one), Err(E::Arithmetic));
            assert_eq!(min.arithmetic(A::Negate, zero), Err(E::Arithmetic));
            assert_eq!(
                V::from_integer(kind, -7)
                    .unwrap()
                    .arithmetic(A::Divide, V::from_integer(kind, 3).unwrap())
                    .unwrap()
                    .integer(),
                Some(-2)
            );
            assert_eq!(
                V::from_physical_bits(kind, u64::MAX).unwrap().integer(),
                Some(-1)
            );
        } else {
            assert_eq!(one.arithmetic(A::Negate, zero), Err(E::TypeMismatch));
        }
        assert!(max.key().is_some());
    }
    assert!(V::from_bits(K::I8, 255).is_err());
    assert!(V::from_bits(K::U8, 256).is_err());
    assert_eq!(
        V::from_integer(K::U64, u64::MAX as i128).unwrap().bits(),
        u64::MAX
    );
}

#[test]
fn floats_use_target_precision_ieee_results_and_comparisons() {
    let base = V::from_f32(16_777_216.0);
    let rounded = base.arithmetic(A::Add, V::from_f32(1.0)).unwrap();
    assert_eq!(
        rounded.arithmetic(A::Subtract, base).unwrap().float(),
        Some(0.0)
    );
    for (zero, negative_zero, one, nan) in [
        (
            V::from_f32(0.0),
            V::from_f32(-0.0),
            V::from_f32(1.0),
            V::from_f32(f32::NAN),
        ),
        (
            V::from_f64(0.0),
            V::from_f64(-0.0),
            V::from_f64(1.0),
            V::from_f64(f64::NAN),
        ),
    ] {
        assert!(zero.compare(C::Equal, negative_zero).unwrap());
        assert!(!nan.compare(C::Equal, nan).unwrap());
        assert!(nan.compare(C::NotEqual, one).unwrap());
        assert!(!nan.compare(C::Less, one).unwrap());
        assert!(
            one.arithmetic(A::Divide, zero)
                .unwrap()
                .float()
                .unwrap()
                .is_infinite()
        );
        assert!(
            zero.arithmetic(A::Divide, zero)
                .unwrap()
                .float()
                .unwrap()
                .is_nan()
        );
        assert_eq!(one.arithmetic(A::Remainder, one), Err(E::TypeMismatch));
        assert!(nan.key().is_none());
        assert!(negative_zero.key().is_none());
    }
    assert!(V::from_bool(true).key().is_none());
}

#[test]
fn conversions_reject_fraction_nonfinite_and_exact_upper_bound_without_saturation() {
    for target in [K::I8, K::I16, K::I32, K::I64, K::U8, K::U16, K::U32, K::U64] {
        let (minimum, _) = target.bounds().unwrap();
        assert_eq!(
            V::from_f64(minimum as f64)
                .convert(target)
                .unwrap()
                .integer(),
            Some(minimum)
        );
        let upper = 2_f64.powi(target.width() as i32 - i32::from(target.signed()));
        for value in [upper, f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.5] {
            assert_eq!(
                V::from_f64(value).convert(target),
                Err(E::Conversion),
                "{target:?}: {value}"
            );
        }
        assert_eq!(
            V::from_f64(-0.0).convert(target).unwrap().integer(),
            Some(0)
        );
    }
    // Direct integer -> binary32 must not round through binary64 at this midpoint.
    let integer = (1_i128 << 60) + (1_i128 << 36) + 1;
    assert_eq!(
        V::from_integer(K::U64, integer)
            .unwrap()
            .convert(K::F32)
            .unwrap()
            .bits(),
        0x5d80_0001
    );
    assert_eq!(((integer as f64) as f32).to_bits(), 0x5d80_0000);
    assert_eq!(
        V::from_integer(K::I8, -1).unwrap().convert(K::U64),
        Err(E::Conversion)
    );
    assert_eq!(V::from_bool(true).convert(K::I8), Err(E::Conversion));
    assert_eq!(
        V::from_integer(K::I8, 1).unwrap().convert(K::Bool),
        Err(E::Conversion)
    );
    assert_eq!(
        V::from_f64(-0.0).convert(K::F32).unwrap().bits(),
        u64::from((-0.0_f32).to_bits())
    );
    assert_eq!(V::from_f64(f64::MAX).convert(K::F32), Err(E::Conversion));
    assert!(
        V::from_f64(f64::INFINITY)
            .convert(K::F32)
            .unwrap()
            .float()
            .unwrap()
            .is_infinite()
    );
    assert_eq!(
        V::from_f64(-f64::MIN_POSITIVE)
            .convert(K::F32)
            .unwrap()
            .bits(),
        u64::from((-0.0_f32).to_bits())
    );
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn aot_c_integer_arithmetic_matches_runtime_with_and_without_sanitizers() {
    use std::fmt::Write;
    let mut program = String::from(
        "#include <assert.h>\n#include \"rinzu/scalar.h\"\n\
         static const struct { unsigned kind, operation; uint64_t left, right, bits; int status; } cases[] = {\n",
    );
    let mut random = 0x5eed_1234_5678_9abc_u64;
    for kind in [K::I8, K::I16, K::I32, K::I64, K::U8, K::U16, K::U32, K::U64] {
        let (minimum, maximum) = kind.bounds().unwrap();
        let values: Vec<_> = [
            minimum,
            minimum + 1,
            minimum / 2,
            -2,
            -1,
            0,
            1,
            2,
            maximum / 2,
            maximum - 1,
            maximum,
        ]
        .into_iter()
        .filter(|value| *value >= minimum && *value <= maximum)
        .map(|value| V::from_integer(kind, value).unwrap())
        .collect();
        let mut pairs: Vec<_> = values
            .iter()
            .flat_map(|left| values.iter().map(move |right| (*left, *right)))
            .collect();
        for _ in 0..128 {
            let mut operands = [V::from_integer(kind, 0).unwrap(); 2];
            for operand in &mut operands {
                random = random
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                *operand = V::from_physical_bits(kind, random).unwrap();
            }
            pairs.push((operands[0], operands[1]));
        }
        for (left, right) in pairs {
            for (operation, arithmetic) in [
                A::Negate,
                A::Add,
                A::Subtract,
                A::Multiply,
                A::Divide,
                A::Remainder,
            ]
            .into_iter()
            .enumerate()
            {
                let (status, bits) = match left.arithmetic(arithmetic, right) {
                    Ok(value) => (0, value.bits()),
                    Err(E::Arithmetic) => (1, 0),
                    Err(E::TypeMismatch) => (2, 0),
                    unexpected => panic!("{kind:?}: {unexpected:?}"),
                };
                writeln!(
                    program,
                    "{{{}, {}, UINT64_C({}), UINT64_C({}), UINT64_C({}), {}}},",
                    kind as u32,
                    operation,
                    left.bits(),
                    right.bits(),
                    bits,
                    status
                )
                .unwrap();
            }
        }
        if kind != K::I64 && kind != K::U64 {
            let invalid = maximum as u64 + 1;
            for operation in 0..=6 {
                writeln!(
                    program,
                    "{{{}, {}, UINT64_C({}), UINT64_C(0), UINT64_C(0), 2}},",
                    kind as u32, operation, invalid
                )
                .unwrap();
                writeln!(
                    program,
                    "{{{}, {}, UINT64_C(0), UINT64_C({}), UINT64_C(0), 2}},",
                    kind as u32, operation, invalid
                )
                .unwrap();
            }
        }
        writeln!(
            program,
            "{{{}, 6, UINT64_C(0), UINT64_C(0), UINT64_C(0), 2}},",
            kind as u32
        )
        .unwrap();
    }
    program.push_str(
        "};\nint main(void) {\n\
         for (size_t i = 0; i < sizeof(cases) / sizeof(cases[0]); ++i) {\n\
           rinzu_scalar_result_t result = rinzu_scalar_arithmetic(cases[i].kind, cases[i].operation, cases[i].left, cases[i].right);\n\
           assert(result.status == cases[i].status);\n\
           assert(result.bits == cases[i].bits);\n\
         }\nreturn 0;\n}\n",
    );
    let root = std::env::temp_dir().join(format!("rinzu-scalar-arithmetic-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("test.c"), program).unwrap();
    let include = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include");
    for flags in [
        vec!["-O2"],
        vec![
            "-O1",
            "-g",
            "-fsanitize=address,undefined",
            "-fno-sanitize-recover=all",
        ],
    ] {
        let output = std::process::Command::new("clang")
            .args(flags)
            .args(["-Wall", "-Wextra", "-Werror", "-I"])
            .arg(&include)
            .arg(root.join("test.c"))
            .arg("-o")
            .arg(root.join("test"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = std::process::Command::new(root.join("test"))
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .env("UBSAN_OPTIONS", "halt_on_error=1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn aot_c_scalar_conversions_match_runtime_boundaries_and_rounding() {
    use std::fmt::Write;
    let kinds = [
        K::I8,
        K::I16,
        K::I32,
        K::I64,
        K::U8,
        K::U16,
        K::U32,
        K::U64,
        K::F32,
        K::F64,
        K::Bool,
    ];
    let mut values = Vec::new();
    for kind in &kinds {
        if let Some((minimum, maximum)) = kind.bounds() {
            for integer in [minimum, maximum, 0, 1] {
                values.push(V::from_integer(*kind, integer).unwrap());
            }
        }
    }
    values.push(V::from_integer(K::U64, (1_i128 << 60) + (1_i128 << 36) + 1).unwrap());
    for value in [
        0.0,
        -0.0,
        0.5,
        -0.5,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::MAX,
        f64::MIN_POSITIVE,
        2_f64.powi(63),
        2_f64.powi(64),
        -2_f64.powi(63),
        f32::MAX as f64,
        1e-50,
    ] {
        values.push(V::from_f64(value));
        values.push(V::from_f32(value as f32));
    }
    values.push(V::from_bool(true));
    let mut program = String::from(
        "#include <assert.h>\n#include \"rinzu/scalar.h\"\nint main(void) { rinzu_scalar_result_t result;\n",
    );
    for value in values {
        for target in kinds {
            writeln!(
                program,
                "result = rinzu_scalar_convert({}, {}, UINT64_C({}));",
                value.kind() as u32,
                target as u32,
                value.bits()
            )
            .unwrap();
            match value.convert(target) {
                Ok(expected) => writeln!(
                    program,
                    "assert(result.status == 0 && result.bits == UINT64_C({}));",
                    expected.bits()
                )
                .unwrap(),
                Err(_) => writeln!(program, "assert(result.status == 3);").unwrap(),
            }
        }
    }
    program.push_str("return 0; }\n");
    let root = std::env::temp_dir().join(format!("rinzu-scalar-conversion-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("test.c"), program).unwrap();
    let include = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include");
    let output = std::process::Command::new("clang")
        .arg("-O2")
        .arg("-Wall")
        .arg("-Wextra")
        .arg("-Werror")
        .arg("-I")
        .arg(include)
        .arg(root.join("test.c"))
        .arg("-o")
        .arg(root.join("test"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = std::process::Command::new(root.join("test"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_dir_all(root).unwrap();
}
