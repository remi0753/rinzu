use rinzu_compiler::{Diagnostic, DiagnosticCode, FileId, SourceFile, SourceRange, compile_source};
const BASE: &str = r#"
import rinzu.list;
error A{} error B{}
async fn fail(){throw A(message:"failure");}
async fn ok(){}
"#;
fn accepted(text: &str) {
    compile_source(&SourceFile::new(
        FileId(251),
        "future-obligations.rinzu",
        format!("{BASE}{text}"),
    ))
    .unwrap();
}
fn lost(text: &str) {
    let diagnostics = compile_source(&SourceFile::new(
        FileId(251),
        "future-obligations.rinzu",
        format!("{BASE}{text}"),
    ))
    .unwrap_err();
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::FutureDiscarded),
        "{diagnostics:#?}"
    );
}

fn assert_path(text: &str, primary: &str, related: &[&str]) -> Diagnostic {
    let text = format!("{BASE}{text}");
    let diagnostics = compile_source(&SourceFile::new(
        FileId(251),
        "future-obligations.rinzu",
        &text,
    ))
    .unwrap_err();
    let [diagnostic] = diagnostics.as_slice() else {
        panic!("expected one diagnostic, got {diagnostics:#?}");
    };
    let at = |range: SourceRange| &text[range.start_byte as usize..range.end_byte as usize];
    assert_eq!(
        diagnostic.code,
        DiagnosticCode::FutureDiscarded,
        "{diagnostic:#?}"
    );
    assert_eq!(at(diagnostic.primary), primary, "{diagnostic:#?}");
    assert_eq!(
        diagnostic
            .secondary
            .iter()
            .map(|(range, _)| at(*range))
            .collect::<Vec<_>>(),
        related,
        "{diagnostic:#?}"
    );
    diagnostic.clone()
}

#[test]
fn an_await_through_an_alias_selects_the_replaced_field() {
    let diagnostic = assert_path(
        r#"
async fn main() {
    let replacement = ok();
    var state = { row = { first = ok(), second = fail() } };
    state = { row = { first = state.row.second, second = state.row.second } };
    state = { row = { first = ok(), second = replacement } };
    let alias = state.row.second;
    await alias;
}
"#,
        "state = { row = { first = ok(), second = replacement } };",
        &["fail()", "ok()", "await alias"],
    );
    assert!(diagnostic.message.contains("`state.row.second`"));
    assert_eq!(
        diagnostic.secondary[1].1,
        "`ok()` creates the replacement Future"
    );
}

#[test]
fn a_retained_alias_and_reassignment_to_the_same_future_keep_the_origin_primary() {
    let source = r#"
async fn main() {
    var state = { pending = fail() };
    let saved = { pending = state.pending };
    state = { pending = ok() };
    await state.pending;
}
"#;
    assert_path(source, "fail()", &["}"]);
    accepted(&source.replace(
        "    await state.pending;",
        "    await state.pending;\n    try { await saved.pending; } catch errors {}",
    ));
    assert_path(
        "async fn main() { var task = fail(); task = task; }",
        "fail()",
        &["}"],
    );
    accepted(
        "async fn main() { var task = fail(); task = task; try { await task; } catch errors {} }",
    );
}

#[test]
fn equal_call_spellings_and_later_replacements_keep_separate_identities() {
    let source = r#"
async fn main() {
    var task = fail();
    task = fail();
    try { await task; } catch errors {}
}
"#;
    let diagnostic = assert_path(source, "task = fail();", &["fail()", "await task"]);
    assert_eq!(
        diagnostic.secondary[0].0.start_byte as usize,
        BASE.len() + source.find("fail()").unwrap()
    );
    assert_path(
        "async fn main() { var task = fail(); task = ok(); let next = ok(); task = next; await task; }",
        "task = ok();",
        &["fail()", "}"],
    );
}

#[test]
fn a_synchronous_factory_points_to_the_expression_that_supplied_the_future() {
    let diagnostic = assert_path(
        r#"
fn make() -> Future<Unit> { return fail(); }
fn cached() -> Future<Unit> { return ok(); }
async fn main() {
    var task = make();
    let next = cached();
    task = next;
    await task;
}
"#,
        "task = next;",
        &["make()", "cached()", "await task"],
    );
    assert_eq!(
        diagnostic.secondary[0].1,
        "this expression yields the Future that can fail"
    );
    assert_eq!(
        diagnostic.secondary[1].1,
        "the replacement Future is obtained here"
    );
    assert_eq!(
        diagnostic.secondary[2].1,
        "this `await` observes the replacement Future"
    );
}

#[test]
fn a_conditional_replacement_uses_only_an_await_on_the_same_path() {
    let source = r#"
fn choose(value: Int) -> Bool { return value < 1; }
async fn main() {
    var task = fail();
    if choose(0) {
        task = ok();
    }
    try { await task; } catch errors {}
}
"#;
    assert_path(source, "task = ok();", &["fail()", "await task"]);
    assert_path(
        &source.replace("task = ok();", "task = ok(); return;"),
        "task = ok();",
        &["fail()", "return;"],
    );
    assert_path(
        &source.replace(
            "}\n    try { await task; } catch errors {}",
            "} else { try { await task; } catch errors {} }",
        ),
        "task = ok();",
        &["fail()", "}"],
    );
    assert_path(
        r#"
fn choose(value: Int) -> Bool { return value < 1; }
async fn main() {
    var task = fail();
    if choose(0) {
        task = ok();
    } else {
        task = ok();
        await task;
    }
}
"#,
        "task = ok();",
        &["fail()", "await task"],
    );
}

#[test]
fn a_failed_assignment_preserves_the_original_future_on_the_error_path() {
    let source = r#"
fn load() throws(B) -> Future<Unit> { throw B(message: "no replacement"); }
async fn refresh() throws(B) {
    var task = fail();
    task = load();
}
fn main() {}
"#;
    assert_path(source, "fail()", &["load()"]);
    accepted(&source.replace(
        "    task = load();",
        "    try { task = load(); } catch errors {}\n    try { await task; } catch errors {}",
    ));
    assert_path(
        &source.replace(
            "    task = load();",
            "    try { task = load(); } catch errors {}",
        ),
        "fail()",
        &["}"],
    );
    assert_path(
        &source.replace(
            "    task = load();",
            "    task = ok();\n    try { await load(); } catch errors {}",
        ),
        "task = ok();",
        &["fail()", "}"],
    );
}

#[test]
fn finally_can_restore_or_replace_the_exit_and_observe_the_original_future() {
    let source = r#"
async fn refresh() throws(B) {
    var task = fail();
    try {
        task = ok();
        return;
    }
    finally { throw B(message: "cleanup"); }
}
fn main() {}
"#;
    assert_path(
        source,
        "task = ok();",
        &["fail()", "throw B(message: \"cleanup\");"],
    );
    assert_path(
        &source.replace("throw B(message: \"cleanup\");", ""),
        "task = ok();",
        &["fail()", "return;"],
    );
    accepted(
        &source
            .replace("    try {\n", "    let original = task;\n    try {\n")
            .replace(
                "throw B(message: \"cleanup\");",
                "try { await original; } catch errors {}",
            ),
    );
}

#[test]
fn a_future_in_a_saved_return_is_still_held_during_cleanup() {
    assert_path(
        r#"
fn refresh() throws(B) -> Future<Unit> {
    var task = fail();
    let replacement = ok();
    try { return task; }
    finally {
        task = replacement;
        throw B(message: "cleanup");
    }
}
fn main() {}
"#,
        "fail()",
        &["throw B(message: \"cleanup\");"],
    );
}

#[test]
fn an_obligation_specialized_by_the_caller_keeps_its_call_site() {
    let source = r#"
async fn refresh(useCache: Bool) {
    var task = fail();
    if useCache { task = ok(); return; }
    try { await task; } catch errors {}
}
async fn main() { await refresh(true); }
"#;
    assert_path(source, "fail()", &["refresh(true)"]);
    accepted(&source.replace("refresh(true)", "refresh(false)"));
}

#[test]
fn heap_aliases_and_loop_generations_fall_back_to_known_locations() {
    assert_path(
        r#"
class Holder { pending: Future<Unit> }
async fn main() {
    var task = fail();
    let holder = Holder(pending: task);
    task = ok();
    await task;
}
"#,
        "fail()",
        &["}"],
    );
    assert_path(
        r#"
async fn main() {
    var task = fail();
    var index = 0;
    while index < 2 {
        task = ok();
        index = index + 1;
    }
    try { await task; } catch errors {}
}
"#,
        "fail()",
        &["}"],
    );
}

#[test]
fn a_call_can_retain_the_original_future_without_proving_observation() {
    assert_path(
        r#"
fn choose(value: Int) -> Bool { return value < 1; }
async fn maybeObserve(task: Future<Unit>, enabled: Bool) {
    if enabled { try { await task; } catch errors {} }
}
fn launch(task: Future<Unit>, enabled: Bool) {
    let observer = maybeObserve(task, enabled);
}
async fn main() {
    var task = fail();
    launch(task, choose(0));
    task = ok();
    await task;
}
"#,
        "fail()",
        &["}"],
    );
}

#[test]
fn an_unobserved_binding_is_not_an_observation() {
    lost("async fn main(){let pending=fail();}");
    accepted("fn main(){let pending=ok();}");
}
#[test]
fn aliases_share_one_task_but_two_calls_do_not() {
    accepted("async fn main(){let pending=fail();let alias=pending;try{await alias;}catch[A]{}}");
    lost("async fn main(){let first=fail();let second=fail();try{await first;}catch[A]{}}");
}
#[test]
fn early_returns_and_normal_errors_cannot_leave_an_unobserved_task() {
    lost(
        "async fn run(flag:Bool){let pending=fail();if flag{return;}try{await pending;}catch[A]{}} async fn main(){let observed=await run(true);}",
    );
    lost(
        "async fn main(){try{let pending=fail();throw B(message:\"before await\");await pending;}catch errors{}}",
    );
    accepted(
        "async fn main(){let pending=fail();try{throw B(message:\"before await\");}catch[B]{try{await pending;}catch[A]{}}}",
    );
}
#[test]
fn a_failed_first_await_does_not_observe_a_second_task() {
    lost(
        "async fn main(){let first=fail();let second=fail();try{await first;await second;}catch errors{}}",
    );
    accepted(
        "async fn main(){let first=fail();let second=fail();try all{await first;await second;}catch errors{}}",
    );
}
#[test]
fn overwrites_and_removed_collection_members_do_not_erase_responsibility() {
    lost("async fn main(){var pending=fail();pending=ok();await pending;}");
    lost("async fn main(){let pending=fail();var values=[pending];values=[];}");
    accepted(
        "async fn main(){let pending=fail();var values=[pending];values=[];try{await pending;}catch[A]{}}",
    );
}
#[test]
fn immutable_fields_and_known_collection_indices_keep_aliases() {
    accepted(
        "class Pending{const future:Future<Unit>} async fn main(){let pending=Pending(future:fail());try{await pending.future;}catch[A]{}}",
    );
    accepted(
        "async fn main(){let pending=fail();let values=[pending];switch rinzu.list.get(values,0){case Option.Some(value):{try{await value;}catch[A]{}}case Option.None:{}}}",
    );
    lost(
        "async fn main(){let first=fail();let second=fail();let values=[first,second];switch rinzu.list.get(values,0){case Option.Some(value):{try{await value;}catch[A]{}}case Option.None:{}}}",
    );
}
#[test]
fn a_sync_factory_returns_the_same_obligation_to_each_caller() {
    accepted(
        "fn make()->Future<Unit>{return fail();} async fn main(){let pending=make();try{await pending;}catch[A]{}}",
    );
    lost("fn make()->Future<Unit>{return fail();} async fn main(){let pending=make();}");
    lost(
        "fn make()->Future<Unit>{return fail();} async fn main(){let first=make();let second=make();try{await first;}catch[A]{}}",
    );
}
#[test]
fn passing_a_future_does_not_observe_it_without_a_proven_callee() {
    lost("fn ignore(value:Future<Unit>){} async fn main(){let pending=fail();ignore(pending);}");
    accepted(
        "async fn observe(value:Future<Unit>){try{await value;}catch errors{}} async fn main(){let pending=fail();let observer=observe(pending);}",
    );
    lost(
        "async fn observe(value:Future<Unit>,flag:Bool){if flag{return;}try{await value;}catch errors{}} async fn main(){let pending=fail();let observer=observe(pending,true);}",
    );
}
#[test]
fn a_return_cancelled_by_a_failing_finally_does_not_transfer_the_future() {
    lost(
        "fn make()->Future<Unit>{try{return fail();}finally{throw B(message:\"cleanup\");}} async fn main(){try{let pending=make();await pending;}catch errors{}}",
    );
    accepted(
        "fn make()->Future<Unit>{try{return fail();}finally{}} async fn main(){try{await make();}catch[A]{}}",
    );
}
#[test]
fn returning_an_already_observed_future_does_not_create_a_new_task() {
    accepted(
        "async fn observe(value:Future<Unit>)->{future:Future<Unit>}{try{await value;}catch errors{}return {future=value};} async fn main(){let pending=fail();let result=await observe(pending);}",
    );
}
#[test]
fn nested_success_values_keep_their_inner_obligations() {
    lost(
        "async fn make()->{future:Future<Unit>}{return {future=fail()};} async fn main(){let outer=make();let row=await outer;}",
    );
    accepted(
        "async fn make()->{future:Future<Unit>}{return {future=fail()};} async fn main(){let outer=make();let row=await outer;try{await row.future;}catch[A]{}}",
    );
}
#[test]
fn a_loop_does_not_deduplicate_new_tasks_from_the_same_call_site() {
    accepted(
        "async fn main(){var index=0;while index<3{try{await fail();}catch[A]{}index=index+1;}}",
    );
    lost("async fn main(){var index=0;while index<3{let pending=fail();index=index+1;}}");
}

#[test]
fn a_nonterminating_async_body_does_not_stop_its_caller_at_registration() {
    lost("async fn forever(){while true{}} fn main(){let running=forever();let lost=fail();}");
    accepted(
        "async fn forever(){while true{}} async fn main(){let running=forever();try{await fail();}catch[A]{}}",
    );
    lost(
        "async fn forever(){while true{}} async fn main(){let pending=fail();let running=forever();await running;try{await pending;}catch[A]{}}",
    );
}

#[test]
fn boolean_input_paths_and_multiple_observer_layers_preserve_responsibility() {
    accepted(
        "async fn observe(value:Future<Unit>){try{await value;}catch errors{}} async fn relay(value:Future<Unit>){let observer=observe(value);} async fn main(){let pending=fail();let observer=relay(pending);}",
    );
    accepted(
        "async fn observe(value:Future<Unit>,enabled:Bool){if enabled{try{await value;}catch errors{}}} async fn main(){let pending=fail();let observer=observe(pending,true);}",
    );
    lost(
        "async fn observe(value:Future<Unit>,enabled:Bool){if enabled{try{await value;}catch errors{}}} async fn main(){let pending=fail();let observer=observe(pending,false);}",
    );
}

#[test]
fn an_indirect_native_pointer_result_is_not_an_infallible_async_body() {
    lost(
        "native{fn load()->String symbol \"load\";} async fn run(action:fn()->String){let text=action();} fn main(){let pending=run(load);}",
    );
    accepted(
        "native{fn load()->String symbol \"load\";} async fn run(action:fn()->String){try{let text=action();}catch errors{}} fn main(){let pending=run(load);}",
    );
}

#[test]
fn mutable_input_fields_do_not_keep_stale_branch_guards_after_writes() {
    lost(
        "class Flag{enabled:Bool} async fn run(flag:Flag){if flag.enabled{flag.enabled=false;if !flag.enabled{let pending=fail();}}} fn main(){let pending=run(Flag(enabled:true));}",
    );
}

#[test]
fn a_stable_error_narrowing_preserves_a_proven_observer() {
    accepted(
        "async fn run(problem:Error){try{if problem is A{throw problem;}}catch[A]{}} fn main(){let pending=run(B(message:\"no throw\"));}",
    );
}

#[test]
fn recursive_shape_widening_retains_requirements_and_reaches_a_fixed_point() {
    let declarations = "class Node{const future:Future<Unit>,const next:Option<Node>} fn build(count:Int)->Node{if count==0{return Node(future:OP(),next:Option<Node>.None());}return Node(future:OP(),next:Option<Node>.Some(value:build(count-1)));}";
    accepted(&format!(
        "{} fn main(){{let values=build(3);}}",
        declarations.replace("OP", "ok")
    ));
    lost(&format!(
        "{} fn main(){{let values=build(3);}}",
        declarations.replace("OP", "fail")
    ));
}

#[test]
fn argument_failure_before_registration_does_not_transfer_a_previously_started_task() {
    let receive =
        "async fn receive(value:Future<Unit>,number:CInt8){try{await value;}catch errors{}}";
    lost(&format!(
        "{receive} async fn main(){{try{{let observer=receive(fail(),CInt8(300));}}catch errors{{}}}}"
    ));
    accepted(&format!(
        "{receive} async fn main(){{let pending=fail();try{{let observer=receive(pending,CInt8(300));}}catch errors{{try{{await pending;}}catch[A]{{}}}}}}"
    ));
}

#[test]
fn bounded_counter_loops_preserve_a_future_until_its_later_await() {
    accepted(
        "async fn main(){let pending=fail();var index=0;while index<300{let marker=f\"{index}\";index=index+1;}try{await pending;}catch[A]{}}",
    );
    lost(
        "async fn main(){let pending=fail();var index=0;while index<300{continue;index=index+1;}try{await pending;}catch[A]{}}",
    );
}

#[test]
fn a_global_function_with_a_scalar_name_does_not_hide_a_constructor_error() {
    lost(
        "fn CInt8(value:Int)->CInt8{return 7;} async fn run(){let value=CInt8(300);} fn main(){let pending=run();}",
    );
    accepted(
        "fn CInt8(value:Int)->CInt8{return 7;} async fn run(){let CInt8=CInt8;let value=CInt8(300);} fn main(){let pending=run();}",
    );
}

#[test]
fn a_future_created_by_a_function_stored_in_an_error_payload_still_requires_observation() {
    let declarations = "error Holder{payload:fn()->Future<Unit>}";
    lost(&format!(
        "{declarations} fn main(){{let holder=Holder(message:\"producer\",payload:fail);let pending=holder.payload();}}"
    ));
    accepted(&format!(
        "{declarations} async fn main(){{let holder=Holder(message:\"producer\",payload:fail);try{{await holder.payload();}}catch errors{{}}}}"
    ));
}

#[test]
fn mutually_recursive_calculations_do_not_grow_the_responsibility_domain_forever() {
    accepted(
        "fn left(depth:Int)->Int{if depth==0{return 0;}return right(depth-1)+1;} fn right(depth:Int)->Int{if depth==0{return 0;}return left(depth-1)+1;} fn main(){let total=left(20);}",
    );
    lost(
        "fn left(depth:Int)->Int{if depth==0{return 0;}return right(depth-1)+1;} fn right(depth:Int)->Int{if depth==0{return 0;}return left(depth-1)+1;} fn main(){let total=left(20);let pending=fail();}",
    );
}

#[test]
fn independent_pending_results_do_not_expand_all_combinations_at_registration() {
    let calls = std::iter::repeat_n("maybe(value)", 48)
        .collect::<Vec<_>>()
        .join(",");
    let declarations = format!(
        "async fn maybe(value:Int){{let converted=CInt8(value);}} fn many(value:Int)->List<Future<Unit>>{{return [{calls}];}}"
    );
    accepted(&format!("{declarations} fn main(){{}}"));
    lost(&format!(
        "{declarations} fn main(){{let pending=many(300);}}"
    ));
}

#[test]
fn many_independent_awaits_can_merge_after_their_errors_are_handled() {
    let mut body = String::new();
    for index in 0..32 {
        body.push_str(&format!("let pending{index}=maybe(value);"));
    }
    for index in 0..32 {
        body.push_str(&format!("try{{await pending{index};}}catch errors{{}}"));
    }
    accepted(&format!(
        "async fn maybe(value:Int){{let converted=CInt8(value);}} async fn run(value:Int){{{body}}} async fn main(){{await run(300);}}"
    ));
}

#[test]
fn a_failed_outer_future_does_not_create_success_only_child_obligations() {
    let declarations = "async fn make(failure:Bool)->{future:Future<Unit>}{if failure{throw B(message:\"outer\");}return {future=fail()};}";
    accepted(&format!(
        "{declarations} async fn run(flag:Bool){{let outer=make(flag);try{{let row=await outer;try{{await row.future;}}catch[A]{{}}}}catch errors{{}}}} async fn main(){{await run(true);}}"
    ));
    lost(&format!(
        "{declarations} async fn run(flag:Bool){{let outer=make(flag);try{{let row=await outer;}}catch errors{{}}}} async fn main(){{await run(false);}}"
    ));
}

#[test]
fn repeated_awaits_do_not_recreate_an_already_observed_success_child() {
    accepted(
        "async fn make(failure:Bool)->{future:Future<Unit>}{if failure{throw B(message:\"outer\");}return {future=fail()};} async fn run(flag:Bool){let outer=make(flag);try{let first=await outer;try{await first.future;}catch[A]{}let again=await outer;}catch errors{}} async fn main(){await run(false);}",
    );
}

#[test]
fn an_observer_can_observe_an_input_or_return_it_on_each_outcome() {
    let declarations = "async fn receive(value:Future<Unit>,consume:Bool)->{future:Future<Unit>}{if consume{try{await value;}catch errors{}return {future=ok()};}return {future=value};} fn make(flag:Bool)->Future<{future:Future<Unit>}>{let pending=fail();return receive(pending,flag);}";
    accepted(&format!(
        "{declarations} async fn main(){{let row=await make(false);try{{await row.future;}}catch errors{{}}}}"
    ));
    lost(&format!(
        "{declarations} async fn main(){{let row=await make(false);}}"
    ));
}

#[test]
fn a_later_outer_failure_cannot_hide_a_child_already_created_in_its_body() {
    let declarations = "async fn make(failure:Bool)->{future:Future<Unit>}{let child=fail();if failure{throw B(message:\"outer\");}return {future=child};}";
    lost(&format!(
        "{declarations} async fn main(){{try{{let row=await make(true);try{{await row.future;}}catch errors{{}}}}catch errors{{}}}}"
    ));
    accepted(&format!(
        "{declarations} async fn main(){{try{{let row=await make(false);try{{await row.future;}}catch errors{{}}}}catch errors{{}}}}"
    ));
}

#[test]
fn many_try_all_awaits_keep_bounded_error_words_without_cartesian_task_states() {
    let mut declarations = String::new();
    let mut awaits = String::new();
    for index in 0..24 {
        declarations.push_str(&format!("let pending{index}=maybe(value);"));
        awaits.push_str(&format!("await pending{index};"));
    }
    accepted(&format!(
        "async fn maybe(value:Int){{let converted=CInt8(value);}} async fn run(value:Int){{{declarations}try all{{{awaits}}}catch errors{{}}}} async fn main(){{await run(300);}}"
    ));
}

#[test]
fn merging_handled_outcomes_never_observes_a_skipped_second_task() {
    let declarations = "async fn maybe(value:Int){let converted=CInt8(value);}";
    lost(&format!(
        "{declarations} async fn run(value:Int){{let first=maybe(value);let second=maybe(value);try{{await first;await second;}}catch errors{{}}}} async fn main(){{await run(300);}}"
    ));
    accepted(&format!(
        "{declarations} async fn run(value:Int){{let first=maybe(value);let second=maybe(value);try all{{await first;await second;}}catch errors{{}}}} async fn main(){{await run(300);}}"
    ));
}

const OBSERVE_LIST: &str = r#"
async fn observeList<T>(values:List<Future<T>>) {
    var index=0;
    while index<rinzu.list.length(values) {
        switch rinzu.list.get(values,index) {
            case Option.Some(value): {try{let result=await value;}catch errors{}}
            case Option.None: {}
        }
        index=index+1;
    }
}
"#;
#[test]
fn a_dynamic_list_observer_receives_every_element_and_not_just_one() {
    accepted(&format!(
        "{OBSERVE_LIST} fn main(){{let first=fail();let second=fail();let observer=observeList<Unit>([first,second]);}}"
    ));
    lost(&format!(
        "{} fn main(){{let first=fail();let second=fail();let observer=observeList<Unit>([first,second]);}}",
        OBSERVE_LIST.replace("var index=0;", "var index=1;")
    ));
}

#[test]
fn a_local_list_can_be_fully_observed_with_an_exhaustive_loop() {
    accepted(
        "async fn main(){let values=[fail(),fail()];var index=0;while index<rinzu.list.length(values){switch rinzu.list.get(values,index){case Option.Some(value):{try{await value;}catch[A]{}}case Option.None:{}}index=index+1;}}",
    );
}

#[test]
fn break_return_stride_and_partial_observation_do_not_prove_exhaustion() {
    for replacement in [
        "break;",
        "return;",
        "if index==0{let result=await value;}else{}",
    ] {
        let modified =
            OBSERVE_LIST.replace("try{let result=await value;}catch errors{}", replacement);
        lost(&format!(
            "{modified} async fn main(){{let first=fail();let second=fail();await observeList<Unit>([first,second]);}}"
        ));
    }
    lost(&format!(
        "{} async fn main(){{let first=fail();let second=fail();await observeList<Unit>([first,second]);}}",
        OBSERVE_LIST.replace("index=index+1;", "index=index+2;")
    ));
}

#[test]
fn a_normal_error_before_an_iteration_await_does_not_observe_the_list() {
    let modified = OBSERVE_LIST.replace(
        "try{let result=await value;}catch errors{}",
        "let invalid=CInt8(300);try{let result=await value;}catch errors{}",
    );
    lost(&format!(
        "{modified} async fn main(){{let first=fail();let second=fail();try{{await observeList<Unit>([first,second]);}}catch errors{{}}}}"
    ));
}

#[test]
fn an_uncaught_iteration_await_does_not_observe_the_remaining_elements() {
    let modified = OBSERVE_LIST.replace(
        "try{let result=await value;}catch errors{}",
        "let result=await value;",
    );
    lost(&format!(
        "{modified} async fn main(){{let first=fail();let second=fail();try{{await observeList<Unit>([first,second]);}}catch errors{{}}}}"
    ));
}

#[test]
fn observing_an_earlier_list_does_not_observe_a_subsequently_appended_task() {
    lost(&format!(
        "{OBSERVE_LIST} fn main(){{let first=fail();let original=[first];let second=fail();let expanded=rinzu.list.append(original,second);let observer=observeList<Unit>(original);}}"
    ));
    accepted(&format!(
        "{OBSERVE_LIST} fn main(){{let first=fail();let original=[first];let second=fail();let expanded=rinzu.list.append(original,second);let observer=observeList<Unit>(expanded);}}"
    ));
}

#[test]
fn all_elements_can_be_observed_in_a_statement_collecting_loop() {
    accepted(
        "async fn main(){let values=[fail(),fail()];var index=0;try all{while index<rinzu.list.length(values){switch rinzu.list.get(values,index){case Option.Some(value):{await value;}case Option.None:{}}index=index+1;}}catch errors{}}",
    );
    lost(
        "async fn main(){var values=[fail(),fail()];var index=0;try all{while index<rinzu.list.length(values){values=[];switch rinzu.list.get(values,index){case Option.Some(value):{await value;}case Option.None:{}}index=index+1;}}catch errors{}}",
    );
}

#[test]
fn the_error_effect_of_a_collecting_list_loop_depends_on_its_actual_elements() {
    let waiting = OBSERVE_LIST
        .replace(
            "while index<rinzu.list.length(values)",
            "try all{while index<rinzu.list.length(values)",
        )
        .replace("index=index+1;\n    }", "index=index+1;\n    }}finally{}")
        .replace(
            "try{let result=await value;}catch errors{}",
            "let result=await value;",
        );
    accepted(&format!(
        "{waiting} fn main(){{let observer=observeList<Unit>([ok(),ok()]);}}"
    ));
    lost(&format!(
        "{waiting} fn main(){{let observer=observeList<Unit>([ok(),fail()]);}}"
    ));
}

#[test]
fn an_immutable_length_binding_can_bound_the_same_list() {
    let observer = OBSERVE_LIST
        .replace(
            "var index=0;",
            "let length=rinzu.list.length(values);var index=0;",
        )
        .replace("index<rinzu.list.length(values)", "index<length");
    accepted(&format!(
        "{observer} fn main(){{let observer=observeList<Unit>([fail(),fail()]);}}"
    ));
    lost(
        "async fn main(){var values=[fail(),fail()];let length=rinzu.list.length(values);values=[];var index=0;while index<length{switch rinzu.list.get(values,index){case Option.Some(value):{try{await value;}catch errors{}}case Option.None:{}}index=index+1;}}",
    );
}

#[test]
fn returning_a_length_never_transfers_the_tasks_used_to_compute_it() {
    lost(
        "fn length()->Int{let values=[fail()];return rinzu.list.length(values);}fn main(){let size=length();}",
    );
}

#[test]
fn exhaustive_field_observation_clears_only_that_field_in_every_row() {
    let declarations = "class Row{const first:Future<Unit>,const second:Future<Unit>} async fn observeRows(rows:List<Row>){var index=0;while index<rinzu.list.length(rows){switch rinzu.list.get(rows,index){case Option.Some(row):{try{await row.first;}catch errors{}SECOND}case Option.None:{}}index=index+1;}}";
    let main = "fn main(){let observer=observeRows([Row(first:fail(),second:fail()),Row(first:fail(),second:fail())]);}";
    lost(&format!("{}{main}", declarations.replace("SECOND", "")));
    accepted(&format!(
        "{}{main}",
        declarations.replace("SECOND", "try{await row.second;}catch errors{}")
    ));
}

#[test]
fn every_list_element_can_be_handed_to_a_proven_independent_observer() {
    let observer =
        "async fn observeItem<T>(value:Future<T>){try{let result=await value;}catch errors{}}";
    let list = OBSERVE_LIST.replace(
        "try{let result=await value;}catch errors{}",
        "let observing=observeItem<T>(value);",
    );
    accepted(&format!(
        "{observer}{list} fn main(){{let observing=observeList<Unit>([fail(),fail()]);}}"
    ));
    lost(&format!(
        "{}{list} fn main(){{let observing=observeList<Unit>([fail(),fail()]);}}",
        observer.replace("try{let result=await value;}catch errors{}", "")
    ));
}

#[test]
fn different_list_elements_may_fail_with_different_error_types() {
    let gathering = OBSERVE_LIST
        .replace(
            "while index<rinzu.list.length(values)",
            "try all{while index<rinzu.list.length(values)",
        )
        .replace("index=index+1;\n    }", "index=index+1;\n    }}finally{}")
        .replace(
            "try{let result=await value;}catch errors{}",
            "let result=await value;",
        );
    let source = format!(
        "{BASE}{gathering} async fn other(){{throw B(message:\"second\");}} async fn main(){{let first=fail();let second=other();try{{await observeList<Unit>([first,second]);}}catch[A]{{}}catch[B]{{}}catch[A,A,...rest]{{}}catch[B,B,...rest]{{}}}}"
    );
    let diagnostics = compile_source(&SourceFile::new(
        FileId(255),
        "independent-iteration-errors.rinzu",
        source,
    ))
    .unwrap_err();
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagnosticCode::NonExhaustiveCatch),
        "{diagnostics:#?}"
    );
}

#[test]
fn separate_collection_reads_do_not_share_one_failure_word() {
    let source = format!(
        "{BASE} async fn other(){{throw B(message:\"second\");}} async fn gather(values:List<Future<Unit>>){{try all{{switch rinzu.list.get(values,0){{case Option.Some(value):{{await value;}}case Option.None:{{}}}}switch rinzu.list.get(values,1){{case Option.Some(value):{{await value;}}case Option.None:{{}}}}}}finally{{}}}} async fn main(){{let first=fail();let second=other();try{{await gather([first,second]);}}catch[A]{{}}catch[B]{{}}catch[A,A]{{}}catch[B,B]{{}}}}"
    );
    let diagnostics = compile_source(&SourceFile::new(
        FileId(256),
        "independent-read-errors.rinzu",
        source,
    ))
    .unwrap_err();
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagnosticCode::NonExhaustiveCatch),
        "{diagnostics:#?}"
    );
}

#[test]
fn one_element_succeeding_cannot_remove_another_elements_failure_exit() {
    let source = r#"
async fn run(values:List<Future<Unit>>) {
    let third=fail();
    switch rinzu.list.get(values,0) {
        case Option.Some(first): {
            try {await first;}
            catch firstErrors {try{await third;}catch thirdErrors{}return;}
            switch rinzu.list.get(values,1) {
                case Option.Some(second): {try {await second;await third;}catch errors{}}
                case Option.None: {try{await third;}catch errors{}}
            }
        }
        case Option.None: {try{await third;}catch errors{}}
    }
}
async fn main(){let second=fail();try{await run([ok(),second]);}catch runErrors{}try{await second;}catch secondErrors{}}
"#;
    lost(source);
    accepted(&source.replace(
        "try {await second;await third;}",
        "try all {await second;await third;}",
    ));
}
