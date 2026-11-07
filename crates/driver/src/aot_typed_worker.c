/* Source Worker transport. The context, typed arguments, same-thread native
   callbacks and GC frames belong to this physical Worker's Owner heap. */
#ifdef RINZU_AOT_HAS_CPU_WORKERS
typedef struct {
  size_t count;
  rinzu_managed_snapshot_t *views;
  rinzu_value_t *values;
} RinzuTypedViews;
static void rinzu_aot_typed_views_dispose(RinzuTypedViews *views) {
  for (size_t index = 0; index < views->count; ++index)
    rinzu_managed_snapshot_dispose(&views->views[index]);
  free(views->views);
  free(views->values);
  *views = (RinzuTypedViews){0};
}
static RinzuTypedViews rinzu_aot_typed_views(rinzu_managed_data_t *data, const size_t *types,
                                             const uint64_t *slots, size_t count) {
  if (rinzu_typed_schema(data->schema, types, count) || (count && !slots) ||
      count > SIZE_MAX / sizeof(rinzu_managed_snapshot_t) ||
      count > SIZE_MAX / sizeof(rinzu_value_t))
    rinzu_failure_fatal();
  RinzuTypedViews views = {count,
                           count ? rinzu_failure_allocate(count * sizeof(*views.views)) : NULL,
                           count ? rinzu_failure_allocate(count * sizeof(*views.values)) : NULL};
  for (size_t index = 0; index < count; ++index) {
    rinzu_managed_slot_t slot = {slots[index], rinzu_program_slot_references[types[index]]};
    if (rinzu_managed_snapshot(data, types[index], slot, &views.views[index]))
      rinzu_failure_fatal();
    views.values[index] = views.views[index].value;
  }
  return views;
}
static void rinzu_aot_typed_encode_input(rinzu_async_future_t *future) {
  rinzu_async_owner(future->runtime);
  RinzuContext *owner = future->transport_owner;
  RinzuTypedViews views = rinzu_aot_typed_views(rinzu_context_data(owner), future->parameter_types,
                                                future->arguments, future->parameter_count);
  if (rinzu_typed_encode(future->value_schema, future->parameter_types, future->parameter_count,
                         views.values, future->input, future->input_size))
    rinzu_failure_fatal();
  rinzu_aot_typed_views_dispose(&views);
}
static uint64_t rinzu_aot_typed_materialize_result(rinzu_async_future_t *future) {
  rinzu_async_owner(future->runtime);
  RinzuContext *owner = future->transport_owner;
  rinzu_managed_data_t *data = rinzu_context_data(owner);
  rinzu_typed_packet_t packet = {0};
  rinzu_managed_slot_t slot = {0};
  int status = rinzu_typed_decode(future->value_schema, &future->result_type, 1, future->result,
                                  future->result_size, &packet);
  if (!status)
    status = rinzu_managed_materialize_values(data, &future->result_type, packet.values, 1, &slot);
  rinzu_typed_packet_dispose(&packet);
  if (status) {
    future->failure.primary = 6;
    future->failure.primary_source = future->source;
    return 0;
  }
  unsigned terminal = atomic_load_explicit(&future->terminal, memory_order_acquire);
  if (slot.reference && terminal != RINZU_ASYNC_CANCELLED && terminal != RINZU_ASYNC_TIMED_OUT) {
    RinzuAsyncResultRoot *root = rinzu_failure_allocate(sizeof(*root));
    root->future_handle = rinzu_async_handle(future);
    rinzu_managed_attach_roots(data, &root->entry, &root->roots);
    root->entry.future_owner = rinzu_async_handle(future);
    future->gc_result = slot.bits;
    rinzu_managed_enter(&root->roots);
    rinzu_managed_publish(&root->roots, &slot.bits, 1);
    root->next = owner->async->result_roots;
    owner->async->result_roots = root;
  }
  return slot.bits;
}
static rinzu_async_future_t *rinzu_aot_typed_worker(RinzuContext *context, const size_t *types,
                                                    const uint64_t *arguments, size_t count,
                                                    unsigned result_kind, size_t result_type,
                                                    void (*run)(rinzu_async_future_t *),
                                                    const char *label) {
  RinzuContext *owner = context->async->root;
  rinzu_managed_data_t *data = rinzu_context_data(owner);
  if (!run || rinzu_typed_schema(data->schema, &result_type, 1) ||
      count > SIZE_MAX / sizeof(uint64_t))
    rinzu_failure_fatal();
  RinzuTypedViews views = rinzu_aot_typed_views(data, types, arguments, count);
  size_t size = 0;
  if (rinzu_typed_size(data->schema, types, count, views.values, &size))
    rinzu_failure_fatal();
  rinzu_aot_typed_views_dispose(&views);
  rinzu_async_future_t *future = rinzu_async_new(&context->async->runtime, RINZU_ASYNC_WORKER,
                                                 RINZU_TRANSFER_UNIT, context->source);
  future->result_kind = result_kind;
  future->result_type = result_type;
  future->value_schema = data->schema;
  future->parameter_types = types;
  future->parameter_count = count;
  future->input_size = size;
  future->run = run;
  future->transport_owner = owner;
  future->encode_input = rinzu_aot_typed_encode_input;
  future->materialize_result = rinzu_aot_typed_materialize_result;
  if (count) {
    future->arguments = rinzu_failure_allocate(count * sizeof(uint64_t));
    memcpy(future->arguments, arguments, count * sizeof(uint64_t));
  }
  rinzu_async_set_readiness(future, rinzu_owner_ready_prepare(rinzu_context_transitions(context),
                                                              label, future->source.file,
                                                              future->source.start_byte,
                                                              future->source.end_byte));
  return future;
}
static int rinzu_aot_typed_prepare_worker(RinzuContext *context, rinzu_async_future_t *future) {
  rinzu_managed_data_t *data = rinzu_context_data(context);
  rinzu_managed_attach_roots(data, &context->domain_roots, &context->roots);
  rinzu_managed_enter(&context->roots);
  rinzu_typed_packet_t packet = {0};
  size_t count = future->parameter_count;
  rinzu_managed_slot_t *slots = count ? rinzu_failure_allocate(count * sizeof(*slots)) : NULL;
  int status = rinzu_typed_decode(future->value_schema, future->parameter_types, count,
                                  future->input, future->input_size, &packet);
  if (!status)
    status = rinzu_managed_materialize_values(data, future->parameter_types, packet.values, count,
                                              slots);
  rinzu_typed_packet_dispose(&packet);
  if (!status) {
    future->arguments = count ? rinzu_failure_allocate(count * sizeof(uint64_t)) : NULL;
    uint64_t *roots = count ? rinzu_failure_allocate(count * sizeof(uint64_t)) : NULL;
    size_t root_count = 0;
    for (size_t index = 0; index < count; ++index) {
      future->arguments[index] = slots[index].bits;
      if (slots[index].reference)
        roots[root_count++] = slots[index].bits;
    }
    rinzu_managed_publish(&context->roots, roots, root_count);
    free(roots);
    /* Moving collection proves preparation roots while input credit is held. */
    rinzu_managed_collect(data, &context->roots);
  } else {
    future->failure.primary = 6;
    future->failure.primary_source = future->source;
    future->result_rejected = 1;
  }
  free(slots);
  rinzu_async_settle_input(future);
  return !status;
}
static void rinzu_aot_typed_encode_result(RinzuContext *context, rinzu_async_future_t *future) {
  RinzuTypedViews views = rinzu_aot_typed_views(rinzu_context_data(context), &future->result_type,
                                                &future->result_slot, 1);
  size_t size = 0;
  if (rinzu_typed_size(future->value_schema, &future->result_type, 1, views.values, &size))
    rinzu_failure_fatal();
  rinzu_async_lock(future->runtime);
  int reserved = rinzu_async_reserve_credit(future, 1, size, &future->result_credit);
  rinzu_async_unlock(future->runtime);
  if (reserved) {
    future->result_size = size;
    future->result = size ? rinzu_failure_allocate(size) : NULL;
    if (rinzu_typed_encode(future->value_schema, &future->result_type, 1, views.values,
                           future->result, size))
      rinzu_failure_fatal();
    future->result_slot = 0;
  }
  rinzu_aot_typed_views_dispose(&views);
  if (!reserved)
    rinzu_async_result_overloaded(future);
}
static void rinzu_aot_typed_finish_worker(RinzuContext *context, rinzu_async_future_t *future) {
  rinzu_managed_leave(&context->roots);
  rinzu_managed_collect(&context->data, &context->roots);
  future->heap_evidence = 1;
  future->worker_objects = context->data.objects;
  future->worker_roots = context->roots.depth;
  future->worker_collections = context->data.collections;
  future->worker_peak_objects = context->data.peak_objects;
  future->worker_peak_frames = context->roots.peak_depth;
  rinzu_aot_userdata_finish(context);
  rinzu_userdata_registry_t *userdata = rinzu_context_userdata(context);
  future->worker_userdata_scopes = userdata->live_scopes;
  future->worker_userdata_tokens = userdata->tokens;
  future->worker_userdata_roots = userdata->roots;
  future->worker_userdata_in_flight = userdata->in_flight;
  future->worker_userdata_created = userdata->created;
  future->worker_userdata_released = userdata->released;
  if (context->return_roots || context->native_calls || context->data.objects ||
      context->roots.depth || context->data.live_views || context->data.view_allocations)
    rinzu_failure_fatal();
  rinzu_async_detach_native_board(future);
  rinzu_owner_transition_finish(rinzu_context_transitions(context));
  rinzu_managed_detach_roots(&context->data, &context->domain_roots);
  rinzu_failure_dispose(&context->pending);
  rinzu_managed_dispose(&context->roots);
  rinzu_managed_data_dispose(&context->data);
}
#endif
static void rinzu_aot_typed_heap_evidence(RinzuAsyncState *state) {
  printf("RINZU_WORKER_HEAPS=");
  const char *separator = "";
  for (unsigned group = 0; group < 2; ++group)
    for (rinzu_async_future_t *future = group ? state->retired_evidence : state->runtime.futures;
         future; future = future->next)
      if (future->heap_evidence) {
        printf("%s%llu:%zu:%zu:%llu:%zu:%zu:%d", separator, (unsigned long long)future->identity,
               future->worker_objects, future->worker_roots,
               (unsigned long long)future->worker_collections, future->worker_peak_objects,
               future->worker_peak_frames, future->worker_body_entered);
        separator = ";";
      }
  printf("\n");
  printf("RINZU_WORKER_USERDATA=");
  separator = "";
  for (unsigned group = 0; group < 2; ++group)
    for (rinzu_async_future_t *future = group ? state->retired_evidence : state->runtime.futures;
         future; future = future->next)
      if (future->heap_evidence) {
        printf("%s%llu:%zu:%zu:%zu:%zu:%zu:%zu", separator, (unsigned long long)future->identity,
               future->worker_userdata_scopes, future->worker_userdata_tokens,
               future->worker_userdata_roots, future->worker_userdata_in_flight,
               future->worker_userdata_created, future->worker_userdata_released);
        separator = ";";
      }
  printf("\n");
  while (state->retired_evidence) {
    rinzu_async_future_t *next = state->retired_evidence->next;
    free(state->retired_evidence);
    state->retired_evidence = next;
  }
}
