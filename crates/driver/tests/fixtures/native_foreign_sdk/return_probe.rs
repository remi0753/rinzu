pub fn instrument(source: String, count: usize) -> String {
    let target = "state->dispatcher = rinzu_owner_dispatcher_new(state->foreign_queue, NULL, NULL, NULL, NULL);";
    assert_eq!(source.matches(target).count(), 1);
    source.replace(target, &format!("{target}\nrinzu_owner_dispatcher_test_return_after_published(state->dispatcher,{count});"))
}
