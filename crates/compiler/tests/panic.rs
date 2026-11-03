use rinzu_compiler::{FileId, SourceFile, compile_source};

#[test]
fn panic_is_an_uncatchable_unit_intrinsic_at_each_managed_boundary() {
    for source in [
        "fn main(){panic(\"failed assertion\");}",
        "import rinzu.core;fn main(){rinzu.core.panic(\"qualified panic\");}",
        "fn main(){let result:Unit=panic(\"unit result\");}",
        "async fn main(){panic(\"async panic\");}",
        "fn work(){panic(\"worker panic\");}async fn main(){await spawn work();}",
    ] {
        compile_source(&SourceFile::new(FileId(1), "panic.rinzu", source)).unwrap();
    }
}

#[test]
fn panic_requires_one_positional_string() {
    for source in [
        "fn main(){panic();}",
        "fn main(){panic(1);}",
        "fn main(){panic(true);}",
        "fn main(){panic(\"first\",\"second\");}",
        "fn main(){panic(message:\"named\");}",
        "fn main(){let result:Int=panic(\"unit\");}",
    ] {
        assert!(
            compile_source(&SourceFile::new(FileId(1), "panic.rinzu", source)).is_err(),
            "{source}"
        );
    }
}

#[test]
fn panic_does_not_return_through_helpers_or_catch_handlers() {
    for body in [
        "panic(\"stop\");",
        "stop();",
        "try {stop();} catch errors {}",
    ] {
        let text = format!(
            "error E{{}} async fn fail()->Int{{throw E(message:\"failure\");}} fn stop(){{panic(\"stop\");}} fn main(){{{body} let unobserved=fail();}}"
        );
        compile_source(&SourceFile::new(FileId(1), "panic-flow.rinzu", text)).unwrap();
    }
    let checks = (0..8)
        .map(|index| format!("if !v{index} {{panic(\"assertion failed\");}} "))
        .collect::<String>();
    let parameters = (0..8)
        .map(|index| format!("v{index}:Bool"))
        .collect::<Vec<_>>()
        .join(",");
    let text = format!(
        "error E{{}} async fn fail()->Int{{throw E(message:\"failure\");}} fn verify({parameters}){{{checks}panic(\"stop\");}} fn main(){{verify(false,false,false,false,false,false,false,false);let unobserved=fail();}}"
    );
    compile_source(&SourceFile::new(FileId(1), "panic-assertions.rinzu", text)).unwrap();
}

#[test]
fn panic_preserves_argument_errors_and_type_checks_unreachable_code() {
    for text in [
        "error E{} fn message()->String {throw E(message:\"argument\");} fn main(){panic(message());}",
        "fn main(){panic(\"stop\");let value=unknown;}",
    ] {
        assert!(
            compile_source(&SourceFile::new(FileId(1), "panic-input.rinzu", text)).is_err(),
            "{text}"
        );
    }
}

#[test]
fn panic_runs_finally_observations_even_after_awaiting_a_forwarded_future() {
    for body in ["stop();", "await halt();", "await forward(halt());"] {
        let text = format!(
            "error E{{}} async fn fail()->Int{{throw E(message:\"failure\");}} fn stop(){{panic(\"stop\");}} async fn halt(){{stop();}} async fn forward(task:Future<Unit>){{await task;}} async fn main(){{let task=fail();try {{{body}}} finally {{try {{let observed=await task;}}catch errors{{}}}}}}"
        );
        compile_source(&SourceFile::new(FileId(1), "panic-finally.rinzu", text)).unwrap();
    }
}

#[test]
fn cleanup_errors_do_not_turn_a_panic_into_a_catchable_error() {
    for body in ["stop();", "await halt();", "await forward(halt());"] {
        let text = format!(
            "error E{{}} fn stop(){{panic(\"stop\");}} async fn halt(){{stop();}} async fn forward(task:Future<Unit>){{await task;}} async fn main(){{try {{{body}}} finally {{throw E(message:\"suppressed cleanup Error\");}}}}"
        );
        compile_source(&SourceFile::new(FileId(1), "panic-cleanup.rinzu", text)).unwrap();
    }
}

#[test]
fn fatal_panic_settles_outstanding_tasks_instead_of_discarding_them() {
    for body in ["stop();", "await halt();", "await forward(halt());"] {
        let text = format!(
            "error E{{}} async fn fail(){{throw E(message:\"failure\");}} fn stop(){{panic(\"stop\");}} async fn halt(){{stop();}} async fn forward(task:Future<Unit>){{await task;}} async fn main(){{let task=fail();{body}}}"
        );
        compile_source(&SourceFile::new(FileId(1), "panic-tasks.rinzu", text)).unwrap();
    }
}

#[test]
fn panic_unwinding_still_checks_future_capture_in_finally() {
    for body in ["stop();", "await halt();", "await forward(halt());"] {
        let text = format!(
            "error Stored{{payload:fn()->Future<Int>}} async fn safe()->Int{{return 7;}} fn stop(){{panic(\"stop\");}} async fn halt(){{stop();}} async fn forward(task:Future<Unit>){{await task;}} async fn main(){{let task=safe();let captured=fn()->Future<Int>{{return task;}};try {{{body}}} finally {{let error=Stored(message:\"invalid\",payload:captured);}}}}"
        );
        let errors =
            compile_source(&SourceFile::new(FileId(1), "panic-retention.rinzu", text)).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("Error payload cannot retain Future")),
            "{errors:#?}"
        );
    }
}
