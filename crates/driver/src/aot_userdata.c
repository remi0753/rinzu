/* Active and parked activations share their actual Owner's userdata registry. */
static rinzu_userdata_registry_t *rinzu_context_userdata(RinzuContext *context) {
  rinzu_userdata_registry_t *registry =
      context->shared_userdata ? context->shared_userdata : &context->userdata;
  rinzu_userdata_owner(registry, rinzu_context_data(context));
  return registry;
}
static void rinzu_aot_userdata_finish(RinzuContext *context) {
  rinzu_userdata_finish(rinzu_context_userdata(context), rinzu_context_data(context));
}
static void rinzu_aot_userdata_evidence(RinzuContext *context) {
  rinzu_aot_userdata_finish(context);
  rinzu_userdata_registry_t *registry = rinzu_context_userdata(context);
  printf("RINZU_NATIVE_USERDATA=%zu,%zu,%zu,%zu,%zu,%zu\n", registry->live_scopes, registry->tokens,
         registry->roots, registry->in_flight, registry->created, registry->released);
  rinzu_userdata_process_stats_t process = rinzu_userdata_process_stats();
  printf("RINZU_USERDATA_PROCESS=%zu,%zu,%zu\n", process.tokens, process.closed,
         process.token_bytes);
}
