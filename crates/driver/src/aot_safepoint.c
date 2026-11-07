/* Private generated-code helpers. Keep normal execution short without changing
   source positions, root liveness, GC pacing or cancellation poll frequency. */
static inline int rinzu_aot_source_range_valid(int64_t file, int64_t start, int64_t end) {
  return ((uint64_t)file | (uint64_t)start | (uint64_t)end) <= UINT32_MAX &&
         (uint64_t)start <= (uint64_t)end;
}
void rinzu_baseline_set_source(void *raw, int64_t file, int64_t start, int64_t end) {
  if (!rinzu_aot_source_range_valid(file, start, end))
    rinzu_failure_fatal();
  ((RinzuContext *)raw)->source =
      (rinzu_source_position_t){(uint64_t)file, (uint32_t)start, (uint32_t)end, 1};
  rinzu_error_trace_stack_source(&((RinzuContext *)raw)->error_trace_stack,
                                 ((RinzuContext *)raw)->source);
}
/* Compiler-owned constants only: emission checks file and start <= end.
   Packing the two 32-bit offsets removes runtime validation and rebuilding
   without loading a separate literal or retaining a pointer into JIT data.
   Always restore the value after a callee or native callback changed it. */
void rinzu_baseline_set_source_packed(void *raw, uint64_t file, uint64_t span) {
  RinzuContext *context = raw;
  context->source.file = file;
  context->source.start_byte = (uint32_t)span;
  context->source.end_byte = (uint32_t)(span >> 32);
  context->source.present = 1;
  rinzu_error_trace_stack_source(&context->error_trace_stack, context->source);
}

void rinzu_baseline_observe_source(void *raw) {
  RinzuContext *context = raw;
  rinzu_error_trace_stack_source(&context->error_trace_stack, context->source);
}

/* Keep the unchanged control suffix out of ordinary root publication. */
static __attribute__((noinline, cold)) int64_t rinzu_aot_poll_control(RinzuContext *context,
                                                                      uint64_t *polls) {
  if (context->shutdown_after && *polls >= context->shutdown_after)
    context->shutdown_requested = 1;
  if (rinzu_aot_async_cancelled(context) ||
      (context->cancel_after && *polls >= context->cancel_after))
    context->cancel_requested = 1;
  if (context->shutdown_requested || context->cancel_requested)
    rinzu_owner_request_shutdown(rinzu_context_transitions(context));
  if (context->pending.depth || context->cancel_masked)
    return 0;
  if (context->callback_depth)
    return context->pending.current.primary != 0;
  if (context->shutdown_requested)
    rinzu_failure_fail(&context->pending, 4, context->source);
  if (context->cancel_requested)
    rinzu_failure_fail(&context->pending, 2, context->source);
  return context->pending.current.primary != 0;
}
static inline int64_t rinzu_aot_poll_context(RinzuContext *context) {
  uint64_t *polls = context->domain_polls ? context->domain_polls : &context->polls;
  if (*polls == UINT64_MAX)
    rinzu_failure_fatal();
  *polls += 1;
  if (rinzu_aot_async_root_shutdown(context))
    context->shutdown_requested = 1;
  /* No Worker observation or configured control event can fire here. Keep
     the original masked/deferred result, even with a pending primary. */
  if (!context->worker && !(context->cancel_after | context->shutdown_after |
                            (uint64_t)(unsigned)context->cancel_requested |
                            (uint64_t)(unsigned)context->shutdown_requested)) {
    if (context->pending.depth || context->cancel_masked)
      return 0;
    return context->pending.current.primary != 0;
  }
  return rinzu_aot_poll_control(context, polls);
}

int64_t rinzu_baseline_poll(void *raw, int64_t site) {
  (void)site;
  return rinzu_aot_poll_context(raw);
}
int64_t rinzu_baseline_has_failure(void *raw) {
  RinzuContext *context = raw;
  return context->pending.current.primary != 0;
}
int64_t rinzu_baseline_function_entry(void *raw, int64_t function) {
  RinzuContext *context = raw;
  rinzu_error_trace_stack_init(&context->error_trace_stack);
  context->pending.trace_stack = &context->error_trace_stack;
  if (context->error_trace_stack.enabled) {
    const char *name = rinzu_error_trace_function_name(function);
    rinzu_error_trace_stack_enter(&context->error_trace_stack, name, strlen(name));
  }
  /* managed initializes and checks the selected Owner itself. The general
     context getter also initializes it, duplicating that work on every call. */
  rinzu_owner_transition_managed(context->shared_transitions ? context->shared_transitions
                                                             : &context->transitions);
  rinzu_managed_enter(&((RinzuContext *)raw)->roots);
  /* Private direct-store contract. The region is owned by this live context;
     tracing mode is initialized once and cannot change during a body. */
  _Static_assert(__BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__, "source span byte order");
  _Static_assert(sizeof(uintptr_t) == 8, "source descriptor pointer width");
  _Static_assert(offsetof(rinzu_source_position_t, file) == 0, "source file offset");
  _Static_assert(offsetof(rinzu_source_position_t, start_byte) == 8, "source start offset");
  _Static_assert(offsetof(rinzu_source_position_t, end_byte) == 12, "source end offset");
  _Static_assert(offsetof(rinzu_source_position_t, present) == 16, "source present offset");
  _Static_assert(sizeof(context->source.present) == 4, "source present width");
  _Static_assert(_Alignof(rinzu_source_position_t) >= 2, "source descriptor tag alignment");
  return (int64_t)((uintptr_t)&context->source | (context->error_trace_stack.enabled ? 1 : 0));
}

int64_t rinzu_baseline_publish_roots(void *raw, const uint64_t *roots, int64_t count) {
  RinzuContext *context = raw;
  rinzu_managed_publish(&context->roots, roots, count);
  rinzu_managed_safepoint(rinzu_context_data(context), &context->roots);
  return 0;
}
uint64_t *rinzu_baseline_prepare_roots(void *raw, int64_t count) {
  return rinzu_managed_prepare_roots(&((RinzuContext *)raw)->roots, count);
}
int64_t rinzu_baseline_commit_roots(void *raw, int64_t count) {
  RinzuContext *context = raw;
  rinzu_managed_commit_roots(&context->roots, count);
  rinzu_managed_safepoint(rinzu_context_data(context), &context->roots);
  return 0;
}
/* These ABI entries preserve publication -> safepoint -> poll -> failure
   status, including a poll's masked/deferred return differing from failure. */
int64_t rinzu_baseline_publish_roots_poll(void *raw, const uint64_t *roots, int64_t count) {
  RinzuContext *context = raw;
  rinzu_managed_publish(&context->roots, roots, count);
  rinzu_managed_safepoint(rinzu_context_data(context), &context->roots);
  (void)rinzu_aot_poll_context(context);
  return context->pending.current.primary != 0;
}
int64_t rinzu_baseline_commit_roots_poll(void *raw, int64_t count) {
  RinzuContext *context = raw;
  rinzu_managed_commit_roots(&context->roots, count);
  rinzu_managed_safepoint(rinzu_context_data(context), &context->roots);
  (void)rinzu_aot_poll_context(context);
  return context->pending.current.primary != 0;
}

/* Private compiler proof: the complete current vector contains only words
   validated by a retained caller publication, audited owned producers or a
   closed synchronous return summary. Every CFG predecessor carries the fact.
   No dynamic cache/stamp is involved. Every other safepoint uses the checked
   entry above. Keep fresh Owner/borrow/writing, complete GC and poll order. */
int64_t rinzu_baseline_commit_proven_roots_poll(void *raw, int64_t count) {
  RinzuContext *context = raw;
  rinzu_managed_commit_roots(&context->roots, count);
  rinzu_managed_data_t *data = rinzu_context_data(context);
  rinzu_managed_owner(data);
  if (data->live_views || data->view_allocations)
    rinzu_failure_fatal();
  if (context->roots.current && context->roots.current->writing)
    rinzu_failure_fatal();
  if (rinzu_gc_due(&data->gc) || (data->future_due && data->future_due(data->future_context)))
    rinzu_managed_collect(data, &context->roots);
  (void)rinzu_aot_poll_context(context);
  return context->pending.current.primary != 0;
}

/* Owner only is compiler-proven; validate every published root as before. */
static inline void rinzu_aot_safepoint_owner_proven(RinzuContext *context) {
  rinzu_managed_data_t *data = rinzu_context_data(context);
  if (data->live_views || data->view_allocations)
    rinzu_failure_fatal();
  if (context->roots.current && context->roots.current->writing)
    rinzu_failure_fatal();
  if (context->roots.current && context->roots.current->count)
    rinzu_managed_validate_frame_owned(data, context->roots.current);
  if (rinzu_gc_due(&data->gc) || (data->future_due && data->future_due(data->future_context)))
    rinzu_managed_collect(data, &context->roots);
}
int64_t rinzu_baseline_commit_roots_poll_owner_proven(void *raw, int64_t count) {
  RinzuContext *context = raw;
  rinzu_managed_commit_roots(&context->roots, count);
  rinzu_aot_safepoint_owner_proven(context);
  (void)rinzu_aot_poll_context(context);
  return context->pending.current.primary != 0;
}
int64_t rinzu_baseline_publish_roots_poll_owner_proven(void *raw, const uint64_t *roots,
                                                       int64_t count) {
  RinzuContext *context = raw;
  rinzu_managed_publish(&context->roots, roots, count);
  rinzu_aot_safepoint_owner_proven(context);
  (void)rinzu_aot_poll_context(context);
  return context->pending.current.primary != 0;
}

/* Independent compiler facts: C06U proves complete root membership, C14 proves
   current same-Data Owner authority. Keep every other check/collection/poll. */
int64_t rinzu_baseline_commit_proven_roots_poll_owner_proven(void *raw, int64_t count) {
  RinzuContext *context = raw;
  rinzu_managed_commit_roots(&context->roots, count);
  rinzu_managed_data_t *data = rinzu_context_data(context);
  if (data->live_views || data->view_allocations)
    rinzu_failure_fatal();
  if (context->roots.current && context->roots.current->writing)
    rinzu_failure_fatal();
  if (rinzu_gc_due(&data->gc) || (data->future_due && data->future_due(data->future_context)))
    rinzu_managed_collect(data, &context->roots);
  (void)rinzu_aot_poll_context(context);
  return context->pending.current.primary != 0;
}

/* Immutable compiler selector, independent of C14's Owner authority. The
   complete vector remains published/traced. No prior-publication stamp,
   retirement epoch, cached pointer or mutable proof state is consulted. */
static inline int64_t rinzu_aot_commit_partial_roots_poll(void *raw, int64_t count, uint64_t known,
                                                          int owner_proven) {
  RinzuContext *context = raw;
  rinzu_managed_commit_roots(&context->roots, count);
  rinzu_managed_data_t *data = rinzu_context_data(context);
  if (!owner_proven)
    rinzu_managed_owner(data);
  if (data->live_views || data->view_allocations || !context->roots.current ||
      context->roots.current->writing || count < 1 || count > 64)
    rinzu_failure_fatal();
  uint64_t all = count == 64 ? UINT64_MAX : (UINT64_C(1) << count) - 1;
  if (!known || (known & ~all))
    rinzu_failure_fatal();
  uint64_t unchecked = all & ~known;
  const uint64_t *roots = context->roots.current->roots;
  rinzu_managed_lookup_entry_t *cache = data->lookup_cache;
  while (unchecked) {
    unsigned index = (unsigned)__builtin_ctzll(unchecked);
    unchecked &= unchecked - 1;
    uint64_t identity = roots[index];
    if (identity > RINZU_MANAGED_HANDLE_BASE && identity <= INT64_MAX) {
      if (cache &&
          cache[(size_t)identity & (RINZU_MANAGED_LOOKUP_CACHE_CAPACITY - 1)].identity == identity)
        continue;
      if (!rinzu_managed_lookup_uncached_owned(data, identity))
        rinzu_failure_fatal();
    }
  }
  if (rinzu_gc_due(&data->gc) || (data->future_due && data->future_due(data->future_context)))
    rinzu_managed_collect(data, &context->roots);
  (void)rinzu_aot_poll_context(context);
  return context->pending.current.primary != 0;
}
int64_t rinzu_baseline_commit_partial_roots_poll(void *raw, int64_t count, uint64_t known) {
  return rinzu_aot_commit_partial_roots_poll(raw, count, known, 0);
}
int64_t rinzu_baseline_commit_partial_roots_poll_owner_proven(void *raw, int64_t count,
                                                              uint64_t known) {
  return rinzu_aot_commit_partial_roots_poll(raw, count, known, 1);
}
