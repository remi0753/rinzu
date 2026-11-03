/* Each parked Main activation shares the Domain machine; CPU Workers own one. */
static rinzu_owner_transition_t *rinzu_context_transitions(RinzuContext *context) {
  rinzu_owner_transition_t *owner =
      context->shared_transitions ? context->shared_transitions : &context->transitions;
  rinzu_owner_transition_init(owner);
  return owner;
}
static size_t rinzu_context_native_roots(RinzuContext *context) {
  return rinzu_managed_native_root_count(rinzu_context_data(context), &context->roots);
}
