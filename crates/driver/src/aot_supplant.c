/* Private Main reservation records. Every physical call keeps its exact SDK
   ABI; the internal handle never becomes a source-visible Future. */
struct RinzuSupplant {
  rinzu_async_future_t *future;
  RinzuContext *owner;
  RinzuContext capsule;
  RinzuAsyncResultRoot *result_root;
  uint64_t generation;
  int64_t plan;
  int queued, capsule_live;
  struct RinzuSupplant *next;
};
static RinzuSupplant *rinzu_aot_supplant_find(RinzuAsyncState *state,
                                              rinzu_async_future_t *future) {
  for (RinzuSupplant *call = state->supplants; call; call = call->next)
    if (call->future == future)
      return call;
  _Exit(86);
}
static void rinzu_aot_supplant_dispose_capsule(RinzuSupplant *call) {
  if (!call->capsule_live)
    return;
  RinzuContext *capsule = &call->capsule;
  if (capsule->roots.depth || capsule->native_calls || capsule->return_roots ||
      capsule->pending.current.primary)
    _Exit(86);
  rinzu_managed_detach_roots(capsule->shared_data, &capsule->domain_roots);
  rinzu_managed_dispose(&capsule->roots);
  rinzu_failure_dispose(&capsule->pending);
  call->capsule_live = 0;
}
static unsigned rinzu_aot_supplant_result_kind(const RinzuNativePlan *plan) {
  if (plan->result >= RINZU_NATIVE_RESULT_OPAQUE)
    return RINZU_ASYNC_MANAGED;
  return plan->result ? plan->result - 1 : RINZU_TRANSFER_UNIT;
}
int64_t rinzu_baseline_supplant_reserve(void *raw, int64_t plan_index, const uint64_t *arguments) {
  RinzuContext *context = raw;
  RinzuAsyncState *state = rinzu_aot_async_state(context);
  if (plan_index < 0 || (uint64_t)plan_index >= rinzu_native_plan_count ||
      !rinzu_native_plans[plan_index].supplant || !context->roots.depth || context->worker)
    _Exit(86);
  const RinzuNativePlan *plan = &rinzu_native_plans[plan_index];
  RinzuSupplant *call = rinzu_failure_allocate(sizeof(*call));
  call->owner = context;
  call->plan = plan_index;
  call->future = rinzu_async_new(&state->runtime, RINZU_ASYNC_TASK,
                                 rinzu_aot_supplant_result_kind(plan), context->source);
  call->next = state->supplants;
  state->supplants = call;
  if (rinzu_supplant_reserve(&state->supplant_gate, &call->generation)) {
    call->future->failure.primary = 13;
    call->future->failure.primary_source = context->source;
    rinzu_async_materialize(call->future);
    return (int64_t)rinzu_async_handle(call->future);
  }
  if (state->active_supplant)
    _Exit(86);
  state->active_supplant = call;
  RinzuContext *capsule = &call->capsule;
  capsule->async = state;
  capsule->shared_data = rinzu_context_data(context);
  capsule->shared_userdata = rinzu_context_userdata(context);
  capsule->shared_transitions = rinzu_context_transitions(context);
  capsule->foreign_queue = state->foreign_queue;
  capsule->domain_polls = &state->polls;
  capsule->cancel_after = context->cancel_after;
  capsule->shutdown_after = context->shutdown_after;
  capsule->source = context->source;
  rinzu_managed_attach_roots(capsule->shared_data, &capsule->domain_roots, &capsule->roots);
  call->capsule_live = 1;
  rinzu_baseline_source_native_prepare(context, plan_index, arguments);
  if (context->pending.current.primary) {
    call->future->failure = context->pending.current;
    context->pending.current = (rinzu_failure_t){0};
    rinzu_aot_supplant_dispose_capsule(call);
    (void)rinzu_supplant_fail_preparation(&state->supplant_gate, call->generation);
    (void)rinzu_supplant_finish_settlement(&state->supplant_gate, call->generation);
    state->active_supplant = NULL;
    rinzu_async_materialize(call->future);
  } else {
    RinzuNativeCall *native = rinzu_native_call(context, plan_index);
    /* The parked task's stack and continuation are never resumed by callback
       child prefixes. Their caller is the runtime's active root stack. */
    for (size_t index = 0; index < native->callback_count; ++index) {
      RinzuNativeCallback *callback = &native->callbacks[index];
      if (callback->managed)
        callback->managed->owner = capsule;
      rinzu_callback_entry_bind_foreign(callback->control, state->foreign_queue, call->generation,
                                        callback->foreign_eligible);
    }
  }
  return (int64_t)rinzu_async_handle(call->future);
}
void rinzu_baseline_supplant_start(void *raw, int64_t handle) {
  RinzuAsyncState *state = rinzu_aot_async_state(raw);
  RinzuSupplant *call =
      rinzu_aot_supplant_find(state, rinzu_async_find(&state->runtime, (uint64_t)handle));
  if (call->queued)
    _Exit(86);
  call->queued = 1;
  if (!call->future->settled)
    (void)rinzu_supplant_queue(&state->supplant_gate, call->generation);
}
static int rinzu_aot_supplant_cancel(RinzuContext *context, rinzu_async_future_t *future) {
  RinzuAsyncState *state = rinzu_aot_async_state(context);
  RinzuSupplant *call = state->active_supplant;
  if (!call || call->future != future)
    return 0;
  unsigned winner =
      context->shutdown_requested ? RINZU_SUPPLANT_SHUTDOWN : RINZU_SUPPLANT_CANCELLED;
  (void)rinzu_supplant_cancel(&state->supplant_gate, call->generation, winner);
  rinzu_owner_request_shutdown(rinzu_context_transitions(context));
  return 1;
}
static void rinzu_aot_supplant_hold_result(RinzuAsyncState *state, RinzuSupplant *call,
                                           uint64_t slot) {
  if (call->future->result_kind != RINZU_ASYNC_MANAGED)
    return;
  rinzu_managed_data_t *data = rinzu_context_data(call->owner);
  if (rinzu_managed_slot_valid(data, rinzu_native_plans[call->plan].result_type,
                               (rinzu_managed_slot_t){slot, 1}))
    _Exit(86);
  RinzuAsyncResultRoot *root = rinzu_failure_allocate(sizeof(*root));
  rinzu_managed_attach_roots(data, &root->entry, &root->roots);
  rinzu_managed_enter(&root->roots);
  rinzu_managed_publish(&root->roots, &slot, 1);
  root->next = state->result_roots;
  state->result_roots = root;
  call->result_root = root;
}
static int rinzu_aot_supplant_drive(RinzuContext *root) {
  RinzuAsyncState *state = rinzu_aot_async_state(root);
  if (root != state->root || !state->root_continuation.running || root->callback_depth)
    _Exit(86);
  RinzuSupplant *call = state->active_supplant;
  if (!call || !call->queued)
    return 0;
  RinzuContext *context = call->owner;
  if (context->cancel_requested || context->shutdown_requested)
    (void)rinzu_aot_supplant_cancel(context, call->future);
  if (rinzu_supplant_snapshot(&state->supplant_gate).winner == RINZU_SUPPLANT_NO_WINNER) {
    RinzuNativeCall *native = rinzu_native_call(context, call->plan);
    int status = rinzu_native_prepare_records(context, native);
    if (status) {
      if (status != RINZU_VALUE_BUSY)
        _Exit(86);
      if (rinzu_supplant_fail_preparation(&state->supplant_gate, call->generation)) {
        rinzu_source_position_t previous = context->source;
        context->source = call->future->source;
        (void)rinzu_aot_record_busy(context);
        context->source = previous;
        call->future->failure = context->pending.current;
        context->pending.current = (rinzu_failure_t){0};
      }
    } else
      atomic_fetch_add(&rinzu_native_evidence.live_buffers, native->record_count);
  }
  uint64_t slot = 0;
  if (rinzu_supplant_begin_entry(&state->supplant_gate, call->generation)) {
    rinzu_supplant_entered(&state->supplant_gate, call->generation);
    rinzu_foreign_loop_begin(state->foreign_queue, call->generation);
    ASYNC_EVENT(context, RINZU_EVENT_NATIVE_ENTER, 0);
    uint64_t address = rinzu_baseline_source_native_address(context, call->plan);
    const uint64_t *words = rinzu_baseline_source_native_words(context, call->plan);
    uint64_t result = rinzu_native_plans[call->plan].supplant(address, words);
    rinzu_foreign_loop_end(state->foreign_queue, call->generation);
    rinzu_baseline_source_native_returned(context, call->plan, result);
    context->cancel_requested |= call->capsule.cancel_requested;
    context->shutdown_requested |= call->capsule.shutdown_requested;
    if (context->cancel_requested || context->shutdown_requested)
      (void)rinzu_aot_supplant_cancel(context, call->future);
    rinzu_supplant_returned(&state->supplant_gate, call->generation);
    ASYNC_EVENT(context, RINZU_EVENT_NATIVE_LEAVE, 0);
    rinzu_baseline_source_native_close(context, call->plan);
    for (;;) {
      RinzuNativeCall *native = rinzu_native_call(context, call->plan);
      int settled = 1;
      for (size_t index = 0; index < native->callback_count; index++)
        if (!rinzu_callback_entry_settled(native->callbacks[index].control))
          settled = 0;
      rinzu_foreign_stats_t records = rinzu_foreign_stats(state->foreign_queue);
      if (settled && !records.live && !records.wake_pins)
        break;
      rinzu_owner_dispatcher_pump(state->dispatcher);
      sched_yield();
    }
    rinzu_baseline_source_native_settle(context, call->plan);
    unsigned winner = rinzu_supplant_snapshot(&state->supplant_gate).winner;
    if (winner == RINZU_SUPPLANT_RETURNED) {
      rinzu_source_position_t previous = context->source;
      context->source = call->future->source;
      slot = rinzu_baseline_source_native_read(context, call->plan);
      context->source = previous;
      if (context->pending.current.primary) {
        call->future->failure = context->pending.current;
        context->pending.current = (rinzu_failure_t){0};
      } else
        rinzu_aot_supplant_hold_result(state, call, slot);
    } else {
      RinzuNativeCall *native = rinzu_native_call(context, call->plan);
      if (native->phase != 3)
        _Exit(86);
      /* Discard the raw result without conversion or a successful binding. */
      if (native->opaque_result.identity) {
        uint64_t identity = native->opaque_result.identity;
        rinzu_managed_discard_opaque(rinzu_context_data(context), &native->opaque_result);
        rinzu_managed_frame_t *frame = native->retained.current;
        if (!frame || !frame->count || frame->roots[frame->count - 1] != identity)
          _Exit(86);
        frame->count--;
      }
      native->phase = 4;
      atomic_fetch_add(&rinzu_native_evidence.read, 1);
      call->future->failure.primary = winner == RINZU_SUPPLANT_SHUTDOWN ? 4 : 2;
      call->future->failure.primary_source = call->future->source;
    }
    rinzu_baseline_source_native_release(context, call->plan);
  } else {
    unsigned winner = rinzu_supplant_snapshot(&state->supplant_gate).winner;
    if (winner != RINZU_SUPPLANT_CANCELLED && winner != RINZU_SUPPLANT_SHUTDOWN &&
        winner != RINZU_SUPPLANT_FAILED)
      _Exit(86);
    rinzu_native_abandon_prepared(context, call->plan);
    if (winner != RINZU_SUPPLANT_FAILED) {
      call->future->failure.primary = winner == RINZU_SUPPLANT_SHUTDOWN ? 4 : 2;
      call->future->failure.primary_source = call->future->source;
    }
  }
  rinzu_aot_supplant_dispose_capsule(call);
  (void)rinzu_supplant_finish_settlement(&state->supplant_gate, call->generation);
  state->active_supplant = NULL;
  call->future->result_slot = slot;
  rinzu_async_materialize(call->future);
  return 1;
}
int64_t rinzu_baseline_supplant_settle(void *raw, int64_t handle) {
  RinzuContext *context = raw;
  RinzuAsyncState *state = rinzu_aot_async_state(context);
  rinzu_async_future_t *future = rinzu_async_find(&state->runtime, (uint64_t)handle);
  RinzuSupplant *call = rinzu_aot_supplant_find(state, future);
  if (call->owner != context || !call->queued)
    _Exit(86);
  int pending = !future->settled;
  if (pending)
    ASYNC_EVENT(context, RINZU_EVENT_AWAIT_SUPPLANT, future->identity);
  /* A cancellation cannot resume this await before native's real return. */
  while (!future->settled) {
    if (context->cancel_requested || context->shutdown_requested)
      (void)rinzu_aot_supplant_cancel(context, future);
    rinzu_aot_async_wait(context, future);
  }
  uint64_t slot = (uint64_t)rinzu_baseline_async_await(context, handle, 0);
  if (call->result_root) {
    /* Transfer to the active frame before dropping the completion pin. The
       next compiled stack-map publication replaces this temporary frame root. */
    rinzu_managed_frame_t *frame = context->roots.current;
    if (context->pending.current.primary || !frame || frame->count >= SIZE_MAX / sizeof(uint64_t))
      _Exit(86);
    uint64_t *roots = rinzu_managed_reserve_roots(&context->roots, frame->count + 1);
    roots[frame->count] = slot;
    rinzu_managed_set_count(&context->roots, frame->count + 1);
    context->roots.publications++;
    rinzu_aot_async_release_result(state, call->result_root);
    call->result_root = NULL;
  }
  RinzuSupplant **cursor = &state->supplants;
  while (*cursor && *cursor != call)
    cursor = &(*cursor)->next;
  if (!*cursor || state->active_supplant == call || call->capsule_live)
    _Exit(86);
  *cursor = call->next;
  free(call);
  return (int64_t)slot;
}
static void rinzu_aot_supplant_init(RinzuAsyncState *state) {
  rinzu_supplant_init(&state->supplant_gate);
  state->foreign_queue = rinzu_foreign_new(NULL, NULL);
  rinzu_foreign_attach_main(state->foreign_queue);
  state->dispatcher = rinzu_owner_dispatcher_new(state->foreign_queue, NULL, NULL, NULL, NULL);
  rinzu_owner_dispatcher_set_hooks(state->dispatcher, rinzu_aot_dispatcher_step,
                                   rinzu_aot_dispatcher_ready, rinzu_aot_dispatcher_can_enter,
                                   state);
  rinzu_async_set_completion_wake(&state->runtime, rinzu_aot_completion_wake, state->foreign_queue);
  state->root->foreign_queue = state->foreign_queue;
}
static void rinzu_aot_supplant_finish(RinzuAsyncState *state) {
  if (state->supplants || state->active_supplant)
    _Exit(86);
  for (rinzu_async_future_t *future = state->runtime.futures; future; future = future->next)
    if (future->started && !future->joined)
      _Exit(86);
  rinzu_async_clear_completion_wake(&state->runtime);
  rinzu_supplant_dispose(&state->supplant_gate);
  rinzu_foreign_stats_t records = rinzu_foreign_stats(state->foreign_queue);
  rinzu_resource_foreign_end(records.live, records.bytes, records.created, records.retired);
  printf("RINZU_FOREIGN=%llu,%llu,%llu,%llu,%zu,%zu,%zu,%zu\n", (unsigned long long)records.created,
         (unsigned long long)records.published, (unsigned long long)records.received,
         (unsigned long long)records.retired, records.live, records.bytes, records.queued,
         records.wake_pins);
  rinzu_owner_dispatcher_dispose(state->dispatcher);
  rinzu_foreign_dispose(state->foreign_queue);
  state->dispatcher = NULL;
  state->foreign_queue = NULL;
  state->root->foreign_queue = NULL;
}
