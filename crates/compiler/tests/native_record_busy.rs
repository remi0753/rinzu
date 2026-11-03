use rinzu_compiler::{
    DiagnosticCode, FileId, OperationKind as O, SourceFile, compile_source, verify_module,
};

const BASE: &str = r#"
native {
    record Flag {count:Int}
    fn take(value:Flag)->Int symbol "take_flag";
    fn invoke(handler:fn()->Int)->Int symbol "invoke";
}
class Plain {value:Int}
type Context={flag:Flag};
fn read(value:Flag)->Int{return value.count;}
"#;

#[test]
fn native_content_reads_and_reloans_need_callback_error_handling() {
    for body in [
        "return value.count;",
        "return take(value);",
        "return read(value);",
        "let held={flag=value};return held.flag.count;",
        "let text=f\"{value}\";return 1;",
        "let held={flag=value};let text=f\"{held}\";return 1;",
        "let held=[value];let text=f\"{held}\";return 1;",
        "let held={1:value};let text=f\"{held}\";return 1;",
    ] {
        for protection in [0, 1, 2] {
            let (fallback, body) = match protection {
                0 => ("", body.to_owned()),
                1 => ("fallback -1", body.to_owned()),
                _ => ("", format!("try{{{body}}}catch errors{{return -1;}}")),
            };
            let source = format!(
                "{BASE} fn install(value:Flag)->Int{{return invoke(fn()->Int {fallback} {{{body}}});}} fn main(){{try{{let value=Flag(count:1);let result=install(value);}}catch errors{{}}}}"
            );
            let result = compile_source(&SourceFile::new(
                FileId(511),
                "native-busy-boundary.rinzu",
                source,
            ));
            if protection == 0 {
                let Err(errors) = result else {
                    panic!("unchecked Busy accepted: {body}")
                };
                assert!(
                    errors
                        .iter()
                        .any(|error| error.code == DiagnosticCode::ErrorEscapesBoundary),
                    "{errors:#?}"
                );
            } else {
                result.unwrap();
            }
        }
    }
}

#[test]
fn holding_native_references_and_reading_managed_fields_adds_no_busy_effect() {
    let source = format!(
        r#"{BASE}
        fn install(value:Flag, plain:Plain)->Int {{
            return invoke(fn()->Int {{
                let held:Context={{flag=value}};
                let reference=held.flag;
                return plain.value;
            }});
        }}
        fn main() {{let value=Flag(count:1);let plain=Plain(value:3);let result=install(value,plain);}}
    "#
    );
    compile_source(&SourceFile::new(
        FileId(512),
        "native-holding.rinzu",
        source,
    ))
    .unwrap();
}

#[test]
fn native_field_ir_requires_a_catch_edge_and_reference_projection_does_not() {
    let source = format!(
        r#"{BASE}
        fn caught(value:Flag)->Int {{try{{return value.count;}}catch errors{{return -1;}}}}
        fn hold(value:Context)->Flag {{return value.flag;}}
        fn main() {{}}
    "#
    );
    let artifact = compile_source(&SourceFile::new(
        FileId(513),
        "native-busy-ir.rinzu",
        source,
    ))
    .unwrap();
    let function = artifact
        .ir
        .functions
        .iter()
        .find(|function| function.name == "caught")
        .unwrap();
    let field = function
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .find(|operation| matches!(operation.kind, O::Field { .. }))
        .unwrap();
    let O::Field {
        error: Some(edge), ..
    } = &field.kind
    else {
        panic!("missing Busy edge")
    };
    assert_ne!(edge.target, function.failure);
    assert!(field.unwind.is_some());
    let hold = artifact
        .ir
        .functions
        .iter()
        .find(|function| function.name == "hold")
        .unwrap();
    assert!(
        hold.blocks
            .iter()
            .flat_map(|block| &block.operations)
            .any(|operation| matches!(operation.kind, O::Field { error: None, .. }))
    );
    let mut module = artifact.ir.clone();
    for operation in module
        .functions
        .iter_mut()
        .find(|function| function.name == "caught")
        .unwrap()
        .blocks
        .iter_mut()
        .flat_map(|block| &mut block.operations)
    {
        if let O::Field { error, .. } = &mut operation.kind {
            *error = None;
        }
    }
    assert!(verify_module(&module).is_err());
}

#[test]
fn formatting_a_class_marker_does_not_read_its_native_record_fields() {
    let source = format!(
        r#"{BASE}
        class Marker {{value:Flag, changes:Int}}
        fn install(value:Flag)->Int {{
            let marker=Marker(value:value,changes:0);
            return invoke(fn()->Int {{let text=f"{{marker}}";return 1;}});
        }}
        fn main() {{let value=Flag(count:1);let result=install(value);}}
    "#
    );
    compile_source(&SourceFile::new(FileId(514), "native-marker.rinzu", source)).unwrap();
}
