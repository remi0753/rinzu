use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source};

const DECLARATIONS: &str = r#"
native {record Flag {enabled:Bool, count:Int} fn change(value:Flag) symbol "change_flag";}
class Holder {const flag:Flag}
fn forward(value:Flag) {change(value);}
error Failed {}
async fn work() {throw Failed(message:"failed");}
"#;

#[test]
fn native_updates_invalidate_field_facts_through_aliases_and_managed_calls() {
    for mutation in [
        "change(flag);",
        "let alias={flag=flag};change(alias.flag);",
        "let holder=Holder(flag:flag);change(holder.flag);",
        "forward(flag);",
    ] {
        for guard in ["flag.enabled", "flag.count==1"] {
            let source = format!(
                r#"{DECLARATIONS}
            async fn main() {{
                let flag=Flag(enabled:true,count:1);
                let pending=work();
                try {{{mutation}}} catch errors {{}}
                if {guard} {{try {{await pending;}} catch errors {{}}}}
            }}"#
            );
            let Err(errors) = compile_source(&SourceFile::new(
                FileId(501),
                "native-mutation.rinzu",
                source,
            )) else {
                panic!("native update retained a stale field fact: {mutation}")
            };
            assert!(
                errors
                    .iter()
                    .any(|error| error.code == DiagnosticCode::FutureDiscarded),
                "{mutation}: {errors:#?}"
            );
        }
    }
}

#[test]
fn a_scalar_snapshot_taken_before_native_entry_remains_a_stable_value() {
    let source = format!(
        r#"{DECLARATIONS}
        async fn main() {{try {{
            let flag=Flag(enabled:true,count:1);
            let before={{enabled=flag.enabled,count=flag.count}};
            let pending=work();
            try {{forward(flag);}} catch errors {{}}
            if before.enabled && before.count==1 {{try {{await pending;}} catch errors {{}}}}
        }} catch errors {{}} }}"#
    );
    compile_source(&SourceFile::new(
        FileId(502),
        "native-snapshot.rinzu",
        source,
    ))
    .unwrap();
}

#[test]
fn native_fields_are_not_constant_facts_or_source_assignment_targets() {
    let artifact = compile_source(&SourceFile::new(
        FileId(503),
        "native-fields.rinzu",
        format!("{DECLARATIONS} fn main(){{}}"),
    ))
    .unwrap();
    assert!(artifact.ir.source_types["Flag"].const_fields.is_empty());
    let errors = compile_source(&SourceFile::new(
        FileId(504),
        "native-assignment.rinzu",
        format!(
            "{DECLARATIONS} fn main(){{let flag=Flag(enabled:true,count:1);flag.enabled=false;}}"
        ),
    ))
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("cannot be assigned")
                || error.message.contains("requires a class")),
        "{errors:?}"
    );
}
