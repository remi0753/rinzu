use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source};

fn check(
    source: &str,
) -> Result<rinzu_compiler::FrontendArtifact, Vec<rinzu_compiler::Diagnostic>> {
    compile_source(&SourceFile::new(
        FileId(408),
        "data-contracts.rinzu",
        source,
    ))
}

const FLAG: &str = r#"
error Failed{}
class Flag{enabled:Bool}
fn update(first:Flag,second:Flag){first.enabled=true;second.enabled=false;}
async fn fail(){throw Failed(message:"failure");}
"#;

#[test]
fn read_only_data_summaries_keep_primitive_guards_around_future_observation() {
    for (initial, accepted) in [(false, true), (true, false)] {
        let source = format!(
            r#"{FLAG}
            fn make()->Flag{{let flag=Flag(enabled:{initial});return flag;}}
            fn read(flag:Flag)->Bool{{return flag.enabled;}}
            async fn main(){{let flag=make();let pending=fail();
                if read(flag){{return;}}try{{await pending;}}catch errors{{}}
            }}"#
        );
        let result = check(&source);
        if accepted {
            result.unwrap();
        } else {
            let errors = result.unwrap_err();
            assert!(
                errors
                    .iter()
                    .any(|error| error.code == DiagnosticCode::FutureDiscarded),
                "{errors:#?}"
            );
        }
    }
}

#[test]
fn later_tasks_keep_the_actual_order_of_aliased_primitive_writes() {
    check(&format!(
        r#"{FLAG}async fn main(){{
        let flag=Flag(enabled:true);update(flag,flag);
        let pending=fail();if flag.enabled{{return;}}
        try{{await pending;}}catch errors{{}}
    }}"#
    ))
    .unwrap();
    let errors = check(&format!(
        r#"{FLAG}async fn main(){{
        let first=Flag(enabled:true);let second=Flag(enabled:true);update(first,second);
        let pending=fail();if first.enabled{{return;}}
        try{{await pending;}}catch errors{{}}
    }}"#
    ))
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.code == DiagnosticCode::FutureDiscarded),
        "{errors:#?}"
    );
}

#[test]
fn a_nested_data_helper_keeps_its_callers_task_observation_paths() {
    check(&format!(
        r#"{FLAG}
        fn reset(flag:Flag){{update(flag,flag);}}
        async fn main(){{let flag=Flag(enabled:true);reset(flag);
            let pending=fail();if flag.enabled{{return;}}try{{await pending;}}catch errors{{}}
        }}"#
    ))
    .unwrap();
}

#[test]
fn future_fields_are_not_data_only_even_when_they_are_not_read() {
    let errors = check(
        r#"
        error Failed{} async fn fail(){throw Failed(message:"failure");}
        class Cell{pending:Future<Unit>}
        fn overwrite(cell:Cell){cell.pending=fail();}
        async fn main(){let original=fail();let cell=Cell(pending:original);
            overwrite(cell);try{await cell.pending;}catch errors{}
        }
    "#,
    )
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.code == DiagnosticCode::FutureDiscarded),
        "{errors:#?}"
    );
}

#[test]
fn data_contracts_still_propagate_normal_errors() {
    let errors = check(
        r#"
        error Failed{} class Counter{value:Int}
        fn advance(counter:Counter){counter.value=counter.value+1;throw Failed(message:"failure");}
        fn main(){let counter=Counter(value:0);advance(counter);}
    "#,
    )
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.code == DiagnosticCode::ErrorEscapesBoundary),
        "{errors:#?}"
    );
}

#[test]
fn writes_before_an_aliased_read_keep_the_callers_observation_path() {
    for (arguments, accepted) in [("first,first", true), ("first,second", false)] {
        let source = format!(
            r#"{FLAG}
            fn reset_then_read(first:Flag,second:Flag)->Bool{{
                first.enabled=false;return second.enabled;
            }}
            fn nested(first:Flag,second:Flag)->Bool{{return reset_then_read(first,second);}}
            async fn main(){{
                let first=Flag(enabled:true);let second=Flag(enabled:true);
                let pending=fail();if nested({arguments}){{return;}}
                try{{await pending;}}catch errors{{}}
            }}"#
        );
        let result = check(&source);
        if accepted {
            result.unwrap();
        } else {
            assert!(
                result
                    .unwrap_err()
                    .iter()
                    .any(|error| error.code == DiagnosticCode::FutureDiscarded)
            );
        }
    }
}

#[test]
fn contextual_branch_clones_keep_the_callers_unobserved_tasks() {
    let errors = check(&format!(
        r#"{FLAG}
        fn nested(flag:Flag)->Bool{{if flag.enabled{{flag.enabled=false;return true;}}return false;}}
        async fn main(){{
            let flag=Flag(enabled:true);let pending=fail();
            if nested(flag){{return;}}try{{await pending;}}catch errors{{}}
        }}"#
    )).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.code == DiagnosticCode::FutureDiscarded)
    );
}

#[test]
fn joining_many_data_paths_never_drops_an_observation_obligation() {
    let fields = (0..7)
        .map(|i| format!("bit{i}:Bool"))
        .collect::<Vec<_>>()
        .join(",");
    let initial = (0..7)
        .map(|i| format!("bit{i}:false"))
        .collect::<Vec<_>>()
        .join(",");
    let branches = (0..7)
        .map(|i| format!("if unknown(){{flags.bit{i}=true;}}else{{flags.bit{i}=false;}}"))
        .collect::<String>();
    for discard in [false, true] {
        let exit = if discard {
            "if flags.bit0{return;}"
        } else {
            ""
        };
        let source = format!(
            r#"
            error Failed{{}} async fn fail(){{throw Failed(message:"failed");}}
            class Flags{{{fields}}}
            fn unknown()->Bool{{return stringLength("input")==0;}}
            fn mutate(flags:Flags){{{branches}}}
            async fn main(){{let flags=Flags({initial});let pending=fail();mutate(flags);
                {exit}try{{await pending;}}catch errors{{}}
            }}"#
        );
        let result = check(&source);
        if discard {
            assert!(
                result
                    .unwrap_err()
                    .iter()
                    .any(|error| error.code == DiagnosticCode::FutureDiscarded)
            );
        } else {
            result.unwrap();
        }
    }
}

#[test]
fn many_future_bearing_paths_keep_pending_tasks_and_ordered_writes() {
    let fields = (0..7)
        .map(|i| format!("bit{i}:Bool"))
        .collect::<Vec<_>>()
        .join(",");
    let initial = (0..7)
        .map(|i| format!("bit{i}:false"))
        .collect::<Vec<_>>()
        .join(",");
    let branches = (0..7)
        .map(|i| format!("if unknown(){{flags.bit{i}=true;}}else{{flags.bit{i}=false;}}"))
        .collect::<String>();
    for discard in [false, true] {
        let exit = if discard {
            "if flags.bit0{return;}"
        } else {
            ""
        };
        let result = check(&format!(
            r#"
            error Failed{{}} async fn fail(){{throw Failed(message:"failed");}}
            class Flags{{pending:Future<Unit>,{fields}}}
            fn unknown()->Bool{{return stringLength("input")==0;}}
            fn mutate(flags:Flags){{{branches}}}
            async fn main(){{let pending=fail();let flags=Flags(pending:pending,{initial});
                mutate(flags);{exit}try{{await flags.pending;}}catch errors{{}}
            }}
        "#
        ));
        if discard {
            assert!(
                result
                    .unwrap_err()
                    .iter()
                    .any(|error| error.code == DiagnosticCode::FutureDiscarded)
            );
        } else {
            result.unwrap();
        }
    }
}

#[test]
fn joining_many_paths_does_not_observe_both_sides_of_a_future_alias() {
    let fields = (0..7)
        .map(|i| format!("bit{i}:Bool"))
        .collect::<Vec<_>>()
        .join(",");
    let initial = (0..7)
        .map(|i| format!("bit{i}:false"))
        .collect::<Vec<_>>()
        .join(",");
    let branches = (0..7)
        .map(|i| format!("if unknown(){{cell.bit{i}=true;}}else{{cell.bit{i}=false;}}"))
        .collect::<String>();
    for drain_both in [false, true] {
        let drain = if drain_both {
            "try{await first;}catch errors{} try{await second;}catch errors{}"
        } else {
            ""
        };
        let result = check(&format!(
            r#"
            error Failed{{}} async fn fail(){{throw Failed(message:"failed");}}
            class Cell{{first:Future<Unit>,second:Future<Unit>,selected:Future<Unit>,{fields}}}
            fn unknown()->Bool{{return stringLength("input")==0;}}
            fn choose(cell:Cell){{{branches}
                if unknown(){{cell.selected=cell.first;}}else{{cell.selected=cell.second;}}
            }}
            async fn main(){{let first=fail();let second=fail();
                let cell=Cell(first:first,second:second,selected:first,{initial});
                choose(cell);try{{await cell.selected;}}catch errors{{}}{drain}
            }}
        "#
        ));
        if drain_both {
            result.unwrap();
        } else {
            let errors = result.unwrap_err();
            assert!(
                errors
                    .iter()
                    .any(|error| error.code == DiagnosticCode::FutureDiscarded),
                "{errors:#?}"
            );
        }
    }
}
