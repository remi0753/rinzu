use rinzu_runtime::scalar::{Comparison, ScalarKind, ScalarValue};

pub fn source() -> String {
    let kinds = [
        ("CInt8", ScalarKind::I8),
        ("CInt16", ScalarKind::I16),
        ("CInt32", ScalarKind::I32),
        ("Int", ScalarKind::I64),
        ("CUInt8", ScalarKind::U8),
        ("CUInt16", ScalarKind::U16),
        ("CUInt32", ScalarKind::U32),
        ("CUInt64", ScalarKind::U64),
        ("Bool", ScalarKind::Bool),
    ];
    let operators = [
        ("==", Comparison::Equal),
        ("!=", Comparison::NotEqual),
        ("<", Comparison::Less),
        ("<=", Comparison::LessEqual),
        (">", Comparison::Greater),
        (">=", Comparison::GreaterEqual),
    ];
    let mut definitions =
        String::from("fn require(condition:Bool){if !condition{panic(\"wrong\");}}\n");
    let mut body = String::from("fn main(){\n");
    let mut checks = 0;
    for (index, (name, kind)) in kinds.into_iter().enumerate() {
        let values: Vec<i128> = if kind == ScalarKind::Bool {
            vec![0, 1]
        } else if kind.signed() {
            let (low, high) = kind.bounds().unwrap();
            vec![low, -1, 0, 1, high]
        } else {
            let (_, high) = kind.bounds().unwrap();
            vec![0, 1, high / 2, high / 2 + 1, high]
        };
        let ops = &operators[..if kind == ScalarKind::Bool { 2 } else { 6 }];
        for (bit, (operator, _)) in ops.iter().enumerate() {
            definitions.push_str(&format!(
                "fn compare{index}_{bit}(a:{name},b:{name})->Bool{{return a {operator} b;}}\n"
            ));
        }
        let literal = |value: &i128| {
            if kind == ScalarKind::Bool {
                (value != &0).to_string()
            } else {
                value.to_string()
            }
        };
        for (i, a) in values.iter().enumerate() {
            // Keep each responsibility-analysis graph small without reducing
            // the Cartesian product of values or dynamic parameter coverage.
            definitions.push_str(&format!(
                "fn row{index}_{i}(){{let a:{name}={};\n",
                literal(a)
            ));
            for (j, b) in values.iter().enumerate() {
                let left = ScalarValue::from_bits(kind, *a as u64).unwrap();
                let right = ScalarValue::from_bits(kind, *b as u64).unwrap();
                definitions.push_str(&format!("let b{j}:{name}={};\n", literal(b)));
                for (bit, (_, operator)) in ops.iter().enumerate() {
                    let expected = left.compare(*operator, right).unwrap();
                    definitions.push_str(&format!(
                        "require(compare{index}_{bit}(a,b{j})=={expected});\n"
                    ));
                    checks += 1;
                }
            }
            definitions.push_str("}\n");
        }
        definitions.push_str(&format!("fn group{index}(){{\n"));
        for i in 0..values.len() {
            definitions.push_str(&format!("row{index}_{i}();\n"));
        }
        definitions.push_str("}\n");
        body.push_str(&format!("group{index}();\n"));
    }
    assert_eq!(checks, 1_208);
    format!("{definitions}{body}}}")
}
