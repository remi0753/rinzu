use rinzu_compiler::{
    AbiScalar, FileId, FrontendArtifact, OperationKind, SourceFile, StackRoot, Terminator, Type,
    compile_source, verify_module,
};

fn fixture() -> FrontendArtifact {
    compile_source(&SourceFile::new(
        FileId(1),
        "composite.rinzu",
        r#"
class Inner { const label: String, const number: CSize }
class Outer { const inner: Inner, const first: Int }
enum Choice { payload(number: CSize, text: String), empty }
fn number() -> CSize { return 18446744073709551615; }
fn text() -> String { return "日本語\u{0}"; }
fn probe() -> CSize {
    let inner = Inner(number: number(), label: text());
    let outer = Outer(first: 1, inner: inner);
    let projected = outer.inner.number;
    let choice = Choice.payload(text: text(), number: number());
    let table: Map<CSize, String> = {number(): text(), number(): text()};
    switch choice {
        case Choice.empty: { return 0; }
        case Choice.payload(n, s): {
            let strings: List<String> = [s];
            return n;
        }
    }
    return projected;
}
fn main() { let result = probe(); }
"#,
    ))
    .unwrap()
}

#[test]
fn nominal_layouts_preserve_evaluation_order_projection_and_case_roots() {
    let artifact = fixture();
    assert_eq!(artifact.ir.source_types, artifact.checked.source_types);
    assert_eq!(
        artifact.ir.source_types["Inner"].fields[0],
        ("label".into(), Type::String)
    );
    let main = artifact
        .ir
        .functions
        .iter()
        .find(|f| f.name == "probe")
        .unwrap();
    let entry = main.blocks.iter().find(|b| b.id == main.entry).unwrap();
    let calls = entry
        .operations
        .iter()
        .filter_map(|operation| {
            if let OperationKind::ManagedCall { target, .. } = &operation.kind {
                Some((target.as_str(), operation.result.as_ref().unwrap().id))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(
        calls.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
        [
            "number", "text", "text", "number", "number", "text", "number", "text"
        ]
    );
    let constructors = entry
        .operations
        .iter()
        .filter_map(|operation| {
            if let OperationKind::Construct {
                target,
                variant,
                arguments,
                ..
            } = &operation.kind
            {
                Some((target.as_str(), *variant, arguments))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(constructors[0].0, "Inner");
    assert_eq!(constructors[0].2, &[calls[1].1, calls[0].1]);
    assert_eq!(constructors[2].0, "Choice");
    assert_eq!(constructors[2].1, Some(0));
    assert_eq!(constructors[2].2, &[calls[3].1, calls[2].1]);
    assert_eq!(
        constructors[3].2,
        &calls[4..].iter().map(|(_, id)| *id).collect::<Vec<_>>()
    );
    let projections = entry
        .operations
        .iter()
        .filter_map(|operation| {
            if let OperationKind::Field {
                identity, field, ..
            } = &operation.kind
            {
                Some((
                    identity.as_str(),
                    *field,
                    &operation.result.as_ref().unwrap().ty,
                ))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(
        projections,
        [
            ("Outer", 0, &Type::Named("Inner".into())),
            ("Inner", 1, &Type::Scalar(AbiScalar::U64))
        ]
    );
    let Terminator::Switch {
        identity, cases, ..
    } = &entry.terminator
    else {
        panic!("switch")
    };
    assert_eq!(identity, "Choice");
    assert_eq!(
        cases.iter().map(|(tag, _)| *tag).collect::<Vec<_>>(),
        [1, 0]
    );
    let payload = main
        .blocks
        .iter()
        .find(|block| block.id == cases[1].1)
        .unwrap();
    let bindings = payload
        .operations
        .iter()
        .filter_map(|operation| {
            if let OperationKind::EnumPayload {
                identity,
                variant,
                field,
                ..
            } = &operation.kind
            {
                Some((
                    identity.as_str(),
                    *variant,
                    *field,
                    &operation.result.as_ref().unwrap().ty,
                ))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(
        bindings,
        [
            ("Choice", 0, 0, &Type::Scalar(AbiScalar::U64)),
            ("Choice", 0, 1, &Type::String)
        ]
    );
    assert!(payload.operations.iter().any(|operation| matches!(&operation.kind, OperationKind::GcSafepoint { stack_map, .. } if stack_map.roots.iter().any(|root| matches!(root, StackRoot::Local { name, ty: Type::String } if name == "s")))));
    assert!(
        main.scopes
            .iter()
            .any(|scope| scope.locals.contains(&"n".into()) && scope.locals.contains(&"s".into()))
    );
    assert!(!main.blocks.iter().flat_map(|block| &block.operations).any(|operation| matches!(&operation.kind, OperationKind::LoadLocal { name } if name.contains('.'))));
    verify_module(&artifact.ir).unwrap();
}

#[test]
fn verifier_rejects_wrong_nominal_fields_constructor_types_and_enum_guards() {
    for mutation in 0..8 {
        let mut artifact = fixture();
        let main = artifact
            .ir
            .functions
            .iter_mut()
            .find(|function| function.name == "probe")
            .unwrap();
        match mutation {
            0 | 1 => {
                let operation = main
                    .blocks
                    .iter_mut()
                    .flat_map(|block| &mut block.operations)
                    .find(|operation| matches!(operation.kind, OperationKind::Field { .. }))
                    .unwrap();
                if let OperationKind::Field {
                    identity, field, ..
                } = &mut operation.kind
                {
                    if mutation == 0 {
                        *identity = "Inner".into();
                    } else {
                        *field = 99;
                    }
                }
            }
            2 | 3 => {
                let operation = main.blocks.iter_mut().flat_map(|block| &mut block.operations).find(|operation| matches!(&operation.kind, OperationKind::Construct { target, .. } if target == "Choice")).unwrap();
                if let OperationKind::Construct {
                    variant, arguments, ..
                } = &mut operation.kind
                {
                    if mutation == 2 {
                        *variant = Some(1);
                    } else {
                        arguments.swap(0, 1);
                    }
                }
            }
            4 | 5 => {
                let block = main
                    .blocks
                    .iter_mut()
                    .find(|block| matches!(block.terminator, Terminator::Switch { .. }))
                    .unwrap();
                if let Terminator::Switch {
                    identity, cases, ..
                } = &mut block.terminator
                {
                    if mutation == 4 {
                        *identity = "Outer".into();
                    } else {
                        cases[0].0 = cases[1].0;
                    }
                }
            }
            6 => {
                let operation = main
                    .blocks
                    .iter_mut()
                    .flat_map(|block| &mut block.operations)
                    .find(|operation| matches!(operation.kind, OperationKind::EnumPayload { .. }))
                    .unwrap();
                if let OperationKind::EnumPayload { variant, .. } = &mut operation.kind {
                    *variant = 1;
                }
            }
            7 => {
                artifact.ir.source_types.remove("Inner");
            }
            _ => unreachable!(),
        }
        assert!(verify_module(&artifact.ir).is_err(), "mutation {mutation}");
    }
    let mut artifact = compile_source(&SourceFile::new(FileId(2), "guard.rinzu", "enum E { a(value: Int), b(value: Int) } fn read(input: E) -> Int { switch input { case E.a(a): { return a; } case E.b(b): { return b; } } return 0; } fn main() {}")).unwrap();
    let main = &mut artifact.ir.functions[0];
    let operation = main
        .blocks
        .iter_mut()
        .flat_map(|block| &mut block.operations)
        .find(|operation| {
            matches!(
                operation.kind,
                OperationKind::EnumPayload { variant: 0, .. }
            )
        })
        .unwrap();
    if let OperationKind::EnumPayload { variant, .. } = &mut operation.kind {
        *variant = 1;
    }
    assert!(
        verify_module(&artifact.ir).is_err(),
        "same-layout unguarded variant"
    );
}
