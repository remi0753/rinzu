#[path = "../test-support/library.rs"]
mod test_library;
use rinzu_codegen::{SourceAsyncOptions, run_source_async_report};
use rinzu_compiler::{FileId, SourceFile};
use test_library::compile_library;

fn run(text: &str, expected: &[u8]) {
    let artifact =
        compile_library(&SourceFile::new(FileId(171), "error-model.rinzu", text)).unwrap();
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    if !text.contains("async fn work") {
        let sync = text.replace("async fn main()", "fn main()");
        let artifact =
            compile_library(&SourceFile::new(FileId(175), "error-jit.rinzu", sync)).unwrap();
        for collect_every_allocation in [false, true] {
            let report = rinzu_codegen::run_baseline_jit_with_options(
                &artifact.ir,
                "main",
                rinzu_codegen::BaselineOptions {
                    capture_stdout: true,
                    collect_every_allocation,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(report.result, Ok(0), "{report:#?}");
            assert_eq!(report.stdout, expected);
            assert_eq!(report.heap_objects, 0);
            assert!(
                report
                    .caught_errors
                    .iter()
                    .all(|failure| failure.managed.is_empty())
            );
        }
    }
    for gc_stress in [false, true] {
        let report = run_source_async_report(
            &artifact.ir,
            "main",
            SourceAsyncOptions {
                capture_stdout: true,
                gc_stress,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(report.failure_details.is_none(), "{report:#?}");
        assert_eq!(report.stdout, expected);
        assert_eq!((report.heap.objects, report.heap.roots), (0, 0));
        assert!(
            report
                .caught_errors
                .iter()
                .all(|failure| failure.managed.is_empty())
        );
    }
}

#[test]
fn throw_and_list_catch_preserve_nominal_payload_and_call_propagation() {
    run(
        r#"import rinzu.list;

        import rinzu.stdio;
        error Missing { payload: Int }
        fn fail() -> Int { throw Missing(message: "missing", payload: 37); }
        fn intermediate() -> Int { return fail(); }
        async fn main() {
            try { let value = intermediate(); rinzu.stdio.writeLine(value); }
            catch [problem: Missing, ...rest] {
                rinzu.stdio.writeLine(problem.name);
                rinzu.stdio.writeLine(problem.message);
                rinzu.stdio.writeLine(problem.payload);
                rinzu.stdio.writeLine(rinzu.list.length(rest));
            }
            catch errors { rinzu.stdio.writeLine("wrong catch"); }
        }
    "#,
        b"Missing\nmissing\n37\n0\n",
    );
}

#[test]
fn inferred_exhaustive_patterns_execute_without_a_whole_list_fallback() {
    run(
        r#"
        import rinzu.stdio;
        error A {payload:Int} error C {payload:Int}
        fn singleton(problem:Error){throw problem;}
        fn combined(){try{singleton(A(message:"a",payload:17));}finally{throw C(message:"c",payload:29);}}
        async fn main(){
            try{singleton(A(message:"single",payload:11));}catch[a:A]{rinzu.stdio.writeLine(a.payload);}
            try{combined();}catch[a:A,c:C]{rinzu.stdio.writeLine(a.payload);rinzu.stdio.writeLine(c.payload);}
        }
    "#,
        b"11\n17\n29\n",
    );
}

#[test]
fn whole_list_bindings_preserve_individual_errors_and_can_outlive_the_catch() {
    run(
        r#"import rinzu.list;

        import rinzu.stdio;
        error A{payload:Int} error B{payload:Int}
        fn collect()->List<Error>{
            try{try{throw A(message:"first",payload:13);}finally{throw B(message:"second",payload:31);}}
            catch errors{return errors;}
            return [];
        }
        async fn main(){
            let saved=collect();
            rinzu.stdio.writeLine(rinzu.list.length(saved));
            switch rinzu.list.get(saved,0){case Option.Some(first):{rinzu.stdio.writeLine(first.name);rinzu.stdio.writeLine(first.message);if first is A{rinzu.stdio.writeLine(first.payload);}}case Option.None:{}}
            switch rinzu.list.get(saved,1){case Option.Some(second):{rinzu.stdio.writeLine(second.name);rinzu.stdio.writeLine(second.message);if second is B{rinzu.stdio.writeLine(second.payload);}}case Option.None:{}}
            try{throw A(message:"single",payload:7);}catch errors{rinzu.stdio.writeLine(rinzu.list.length(errors));}
        }
    "#,
        b"2\nA\nfirst\n13\nB\nsecond\n31\n1\n",
    );
}

#[test]
fn statement_all_continues_collects_in_order_and_keeps_finally_outside_its_catch() {
    run(
        r#"import rinzu.list;

        import rinzu.stdio;
        error A{} error B{}
        async fn main(){
            var visited=0;
            try all {
                throw A(message:"a");
                visited=visited+1;
                throw B(message:"b");
                visited=visited+1;
            } catch errors {
                rinzu.stdio.writeLine(visited);
                rinzu.stdio.writeLine(rinzu.list.length(errors));
                switch rinzu.list.get(errors,0){case Option.Some(first):{rinzu.stdio.writeLine(first.name);}case Option.None:{}}
                switch rinzu.list.get(errors,1){case Option.Some(second):{rinzu.stdio.writeLine(second.name);}case Option.None:{}}
            } finally {rinzu.stdio.writeLine("finally");}
        }
    "#,
        b"2\n2\nA\nB\nfinally\n",
    );
}

#[test]
fn statement_all_preserves_saved_control_and_handler_return_overrides_it() {
    run(
        r#"import rinzu.list;

        import rinzu.stdio;
        error A{}
        fn choose()->String{
            try all {throw A(message:"a");return "held";}
            catch errors {rinzu.stdio.writeLine(rinzu.list.length(errors));}
            finally {let churn=["moving","roots"];rinzu.stdio.writeLine("choose-finally");}
            return "wrong";
        }
        fn replacement()->Int{
            try all {throw A(message:"a");return 5;}
            catch errors {return 6;}
            finally {rinzu.stdio.writeLine("replacement-finally");}
            return 0;
        }
        async fn main(){
            rinzu.stdio.writeLine(choose());
            rinzu.stdio.writeLine(replacement());
            var counter=0;
            while counter<3 {
                try all {throw A(message:"loop");counter=counter+1;if counter==1{continue;}break;}
                catch errors {rinzu.stdio.writeLine(counter);}
                finally {rinzu.stdio.writeLine("loop-finally");}
            }
            rinzu.stdio.writeLine(counter);
        }
    "#,
        b"1\nchoose-finally\nheld\nreplacement-finally\n6\n1\nloop-finally\n2\nloop-finally\n2\n",
    );
}

#[test]
fn statement_all_without_catch_propagates_after_finally_and_handler_failure_stays_outward() {
    run(
        r#"import rinzu.list;

        import rinzu.stdio;
        error A{} error B{} error C{}
        fn missing(){try all {throw A(message:"a");throw B(message:"b");}finally {rinzu.stdio.writeLine("before propagation");}}
        fn handled(){try all {throw A(message:"a");}catch errors {throw B(message:"handler");}finally {throw C(message:"cleanup");}}
        async fn main(){
            try {missing();}catch errors {rinzu.stdio.writeLine(rinzu.list.length(errors));}
            try {handled();}catch errors {rinzu.stdio.writeLine(rinzu.list.length(errors));switch rinzu.list.get(errors,0){case Option.Some(first):{rinzu.stdio.writeLine(first.name);}case Option.None:{}}}
            try all {rinzu.stdio.writeLine("no failure");}catch errors {rinzu.stdio.writeLine("wrong");}finally {rinzu.stdio.writeLine("empty-finally");}
        }
    "#,
        b"before propagation\n2\n2\nB\nno failure\nempty-finally\n",
    );
}

#[test]
fn statement_all_observes_awaits_and_deduplicates_one_future_per_group() {
    run(
        r#"import rinzu.list;

        import rinzu.stdio;
        error A{}
        async fn work(){throw A(message:"a");}
        async fn main(){
            let pending=work();
            try all {await pending;await pending;}catch errors {rinzu.stdio.writeLine(rinzu.list.length(errors));}
            var round=0;
            try all {while round<2{await pending;round=round+1;}}catch errors {rinzu.stdio.writeLine(rinzu.list.length(errors));}
            rinzu.stdio.writeLine(round);
        }
    "#,
        b"1\n1\n2\n",
    );
}

#[test]
fn statement_all_explicit_cleanup_await_preserves_return_roots_and_propagates_outward() {
    run(
        r#"
        import rinzu.stdio;
        error A{}
        async fn work(){throw A(message:"cleanup");}
        async fn ready(){}
        async fn held()->String{try all{return "held";}finally{await ready();let churn=["moving","return"]; }return "wrong";}
        async fn main(){
            rinzu.stdio.writeLine(await held());
            try {try all {throw A(message:"body");}catch errors{rinzu.stdio.writeLine("body catch");}finally{await work();}}
            catch errors {rinzu.stdio.writeLine("cleanup outside");}
        }
    "#,
        b"held\nbody catch\ncleanup outside\n",
    );
}

#[test]
fn statement_all_rejects_an_abandoned_failure_and_does_not_adopt_a_safe_child() {
    let invalid = r#"
        import rinzu.stdio;
        error A{}
        async fn work(){throw A(message:"a");}
        async fn parent(){try all{let unobserved=work();}catch errors{rinzu.stdio.writeLine("wrong all catch");}}
        async fn main(){try{await parent();}catch errors{rinzu.stdio.writeLine("outside");}}
    "#;
    let diagnostics = compile_library(&SourceFile::new(
        FileId(171),
        "unobserved-all.rinzu",
        invalid,
    ))
    .unwrap_err();
    assert!(
        diagnostics
            .iter()
            .any(|error| error.code == rinzu_compiler::DiagnosticCode::FutureDiscarded)
    );
    run(
        r#"
import rinzu.stdio;
error A{} error B{}
async fn work(){try{throw A(message:"handled inside child");}catch[A]{}}
async fn parent(){try all{let unobserved=work();throw B(message:"body");}catch[B]{rinzu.stdio.writeLine("body only");}}
async fn main(){await parent();}
"#,
        b"body only\n",
    );
}

#[test]
fn constructor_optional_generic_payload_and_common_error_upcast_work_without_throw() {
    run(
        r#"
        import rinzu.stdio;
        error Invalid<T> { payload?: T }
        fn message(problem: Error) -> String { return problem.message; }
        async fn main() {
            let absent = Invalid<Int>(message: "absent");
            let present = Invalid<Int>(message: "present", payload: 23);
            rinzu.stdio.writeLine(absent.name);
            rinzu.stdio.writeLine(absent.payload);
            rinzu.stdio.writeLine(present.payload);
            rinzu.stdio.writeLine(message(present));
        }
    "#,
        b"Invalid\nOption<Int>.None\nOption<Int>.Some(value: 23)\npresent\n",
    );
}

#[test]
fn rethrow_preserves_shared_class_payload_after_callee_frame_is_gone() {
    run(
        r#"
        import rinzu.stdio;
        class Document { value: Int }
        error SaveFailed { payload: Document }
        fn createFailure(document: Document) {
            throw SaveFailed(message: "save", payload: document);
        }
        fn relay(document: Document) {
            try { createFailure(document); }
            catch [problem: SaveFailed] { document.value = 19; throw problem; }
            catch errors { throw SaveFailed(message: "unexpected", payload: document); }
        }
        async fn main() {
            let document = Document(value: 7);
            try { relay(document); }
            catch [problem: SaveFailed] {
                document.value = 31;
                rinzu.stdio.writeLine(problem.payload.value);
                problem.payload.value = 43;
                rinzu.stdio.writeLine(document.value);
            }
            catch errors { rinzu.stdio.writeLine("wrong catch"); }
        }
    "#,
        b"31\n43\n",
    );
}

#[test]
fn repeated_await_observes_the_same_failure_and_handler_errors_propagate_outward() {
    run(
        r#"
        import rinzu.stdio;
        error Failed { payload: Int }
        error HandlerFailed {}
        async fn work() -> Int { throw Failed(message: "work", payload: 11); }
        async fn main() {
            let pending = work();
            try { let first = await pending; }
            catch [problem: Failed] { rinzu.stdio.writeLine(problem.payload); }
            catch errors { rinzu.stdio.writeLine("wrong catch"); }
            try {
                try { let again = await pending; }
                catch [problem: Failed] { rinzu.stdio.writeLine(problem.payload); throw HandlerFailed(message: "handler"); }
                catch errors { rinzu.stdio.writeLine("same try must not catch handler failure"); }
            }
            catch [problem: HandlerFailed] { rinzu.stdio.writeLine(problem.message); }
            catch errors { rinzu.stdio.writeLine("wrong catch"); }
        }
    "#,
        b"11\n11\nhandler\n",
    );
}

#[test]
fn full_list_matching_preserves_finally_order_and_selects_one_handler() {
    run(
        r#"import rinzu.list;

        import rinzu.stdio;
        error A { payload: Int }
        error B { payload: Int }
        async fn main() {
            try {
                try { throw A(message: "a", payload: 3); }
                finally { throw B(message: "b", payload: 7); }
            }
            catch [single: A] { rinzu.stdio.writeLine("wrong length"); }
            catch [first: A, second: B, ...rest] {
                rinzu.stdio.writeLine(first.payload);
                rinzu.stdio.writeLine(second.payload);
                rinzu.stdio.writeLine(rinzu.list.length(rest));
            }
            catch errors { rinzu.stdio.writeLine("wrong order"); }
        }
    "#,
        b"3\n7\n0\n",
    );
}

#[test]
fn generic_identity_and_explicit_cause_survive_common_error_projection() {
    run(
        r#"
        import rinzu.stdio;
        error Invalid<T> { payload: T }
        error Wrapped { payload: Int }
        fn inspect(problem: Error) {
            rinzu.stdio.writeLine(problem.name);
            let cause = problem.cause;
            switch cause {
                case Option.Some(value): { rinzu.stdio.writeLine(value.message); }
                case Option.None: { rinzu.stdio.writeLine("missing cause"); }
            }
        }
        async fn main() {
            let original = Invalid<String>(message: "original", payload: "text");
            let wrapped = Wrapped(message: "wrapped", payload: 19, cause: original);
            inspect(wrapped);
            try { throw original; }
            catch [problem: Invalid<Int>] { rinzu.stdio.writeLine("wrong generic type"); }
            catch [problem: Invalid<String>] { rinzu.stdio.writeLine(problem.payload); }
            catch errors { rinzu.stdio.writeLine("wrong catch"); }
        }
    "#,
        b"Wrapped\noriginal\ntext\n",
    );
}

#[test]
fn is_refines_same_identity_without_copying_the_error_or_class_payload() {
    run(
        r#"
        import rinzu.stdio;
        class Document { value:Int }
        class Holder { error:Error }
        error Invalid<T> { payload:T }
        error Other {}
        fn inspect(problem:Error, document:Document) {
            rinzu.stdio.writeLine(problem is Invalid<Int>);
            if problem is Invalid<Document> && problem.payload.value>0 {
                document.value=19;
                rinzu.stdio.writeLine(problem.payload.value);
                problem.payload.value=23;
            }
            if !(problem is Invalid<Document>) {rinzu.stdio.writeLine("wrong type");}
            else {rinzu.stdio.writeLine(problem.payload.value);}
        }
        fn makeReader(problem:Error)->fn()->Int {
            if problem is Invalid<Document> {return fn()->Int{return problem.payload.value;};}
            return fn()->Int{return -1;};
        }
        async fn main() {
            let document=Document(value:7);
            let original=Invalid<Document>(message:"x",payload:document);
            inspect(original,document);
            let reader=makeReader(original);
            document.value=31;
            rinzu.stdio.writeLine(reader());
            let holder=Holder(error:original);
            let saved=holder.error;
            if saved is Invalid<Document> {
                holder.error=Other(message:"other");
                rinzu.stdio.writeLine(saved.payload.value);
                rinzu.stdio.writeLine(holder.error is Other);
            }
        }
    "#,
        b"false\n19\n23\n31\n31\ntrue\n",
    );
}

#[test]
fn error_identity_tests_are_local_to_each_worker_domain() {
    let artifact=compile_library(&SourceFile::new(FileId(177),"worker-is.rinzu",r#"
        import rinzu.stdio;
        error E{payload:Int}
        fn check()->Bool{let error:Error=E(message:"x",payload:7);if error is E{return error.payload==7;}return false;}
        async fn main(){let pending=spawn check();rinzu.stdio.writeLine(await pending);}
    "#)).unwrap();
    let report = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(report.failure_details.is_none(), "{report:#?}");
    assert_eq!(report.stdout, b"true\n");
    assert_eq!(report.worker_heaps.len(), 1);
    assert!(
        report
            .worker_heaps
            .iter()
            .all(|worker| worker.heap.objects == 0 && worker.heap.roots == 0)
    );
}

#[test]
fn caught_diagnostic_evidence_does_not_retain_payloads_between_iterations() {
    let artifact = compile_library(&SourceFile::new(
        FileId(172),
        "error-roots.rinzu",
        r#"
        error Failed { payload: String }
        fn fail() { throw Failed(message: "temporary", payload: "temporary payload"); }
        async fn main() {
            var index = 0;
            while index < 128 {
                try { fail(); }
                catch [problem: Failed] { let payload = problem.payload; }
                catch errors {}
                index = index + 1;
            }
        }
    "#,
    ))
    .unwrap();
    let report = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(report.failure_details.is_none(), "{report:#?}");
    assert_eq!(report.caught_errors.len(), 128);
    assert!(report.heap.peak_objects < 32, "{report:#?}");
    assert_eq!((report.heap.objects, report.heap.roots), (0, 0));
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn jit_and_aot_object_accept_the_connected_nominal_error_schema() {
    let artifact = compile_library(&SourceFile::new(
        FileId(173),
        "error-backend.rinzu",
        "error E {} fn main() { let value = E(message: \"x\"); }",
    ))
    .unwrap();
    let run = rinzu_codegen::run_baseline_jit(&artifact.ir, "main").unwrap();
    assert_eq!(run.result, Ok(0));
    let aot = rinzu_codegen::emit_baseline_aot_object(&artifact.ir, "main").unwrap();
    assert!(aot.data_schema_c.contains("RINZU_VALUE_ERROR_RECORD"));
}
