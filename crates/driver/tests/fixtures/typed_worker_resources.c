/* Appended to the generated runtime, so these exercise the production typed
   callbacks and compiled source bodies rather than a parallel test codec. */
static void model_corrupt_bytes(unsigned char *packet, size_t size) {
  const unsigned char payload[] = {'A', 255, 0, 0};
  for (size_t index = 8; index + sizeof payload <= size; ++index) {
    if (!memcmp(packet + index, payload, sizeof payload)) {
      assert(packet[index - 8] == 4);
      packet[index - 8] = 3;
      return;
    }
  }
  assert(0 && "Bytes(4) payload missing");
}
static void model_heap_settled(const rinzu_async_future_t *future, int body) {
  assert(future->heap_evidence && !future->worker_objects && !future->worker_roots);
  assert(future->worker_collections && future->worker_peak_frames >= (body ? 2u : 1u));
  assert(future->worker_body_entered == body);
}
static void model_credits_settled(const rinzu_async_runtime_t *runtime) {
  assert(!runtime->domains && !runtime->in_flight && !runtime->transport_bytes);
  for (const rinzu_async_credit_t *credit = runtime->credits; credit; credit = credit->next) {
    assert(credit->reserved_at && credit->released_at > credit->reserved_at);
  }
}
typedef struct {
  pthread_mutex_t gate;
  pthread_cond_t changed;
  int entered, released;
} ModelReturnGate;
static void model_late_return(rinzu_async_future_t *future) {
  MODEL_ECHO_RUN(future);
  ModelReturnGate *control = (void *)future->native_payload;
  assert(!pthread_mutex_lock(&control->gate));
  control->entered = 1;
  assert(!pthread_cond_broadcast(&control->changed));
  while (!control->released)
    assert(!pthread_cond_wait(&control->changed, &control->gate));
  assert(!pthread_mutex_unlock(&control->gate));
}
static void model_run(unsigned mode) {
  RinzuAsyncState async = {0};
  RinzuContext context = {0};
  rinzu_async_init(&async.runtime);
  async.runtime.record_evidence = 1;
  context.async = &async;
  async.root = &context;
  context.shared_data = &context.data;
  context.source = (rinzu_source_position_t){1, 17, 23, 1};
  rinzu_managed_data_t *data = rinzu_context_data(&context);
  size_t process_baseline = atomic_load(&model_native_live);
  rinzu_managed_attach_roots(data, &context.domain_roots, &context.roots);
  rinzu_managed_enter(&context.roots);
  const unsigned char text[] = {0xe6, 0x97, 0xa5, 0xe6, 0x9c, 0xac, 0xe8,
                                0xaa, 0x9e, 0,    0xf0, 0x9f, 0x99, 0x82};
  const unsigned char bytes[] = {'A', 255, 0, 0};
  uint64_t byte_slot = 0, original = 0;
  assert(!rinzu_managed_bytes(data, 4, bytes, sizeof bytes, &byte_slot));
  rinzu_managed_slot_t fields[] = {
      {rinzu_managed_string(data, text, sizeof text), 1}, {byte_slot, 1}, {UINT64_MAX, 0}};
  assert(!rinzu_managed_composite(data, MODEL_PACKET_TYPE, -1, fields, 3, &original));
  rinzu_managed_publish(&context.roots, &original, 1);
  rinzu_managed_collect(data, &context.roots);
  assert(data->objects == 3);
  if (mode == 4) {
    rinzu_async_future_t *children[8];
    for (size_t index = 0; index < 8; ++index) {
      children[index] = rinzu_aot_worker_request(&context, MODEL_ECHO, &original, 1);
      assert(rinzu_async_admit(children[index]));
      rinzu_async_execute(children[index]);
      model_heap_settled(children[index], 1);
    }
    assert(async.runtime.domains == 8 && async.runtime.in_flight == 8);
    rinzu_async_future_t *denied = rinzu_aot_worker_request(&context, MODEL_ECHO, &original, 1);
    rinzu_async_start(denied);
    assert(denied->failure.primary == 3 && !denied->admitted && !denied->started &&
           !denied->worker_body_entered && !denied->input_credit);
    assert(!rinzu_async_materialize(denied));
    rinzu_async_release(denied);
    for (size_t index = 0; index < 8; ++index) {
      assert(!rinzu_async_cancel(children[index])); /* Physical completion already won. */
      assert(rinzu_async_materialize(children[index]) && !children[index]->failure.primary);
      rinzu_async_release(children[index]);
    }
  } else {
    size_t target = mode == 2 ? MODEL_EXPANDED : mode == 3 ? MODEL_OBSERVED : MODEL_ECHO;
    if (mode == 2 || mode == 3) {
      async.runtime.process_transport_limit = mode == 2 ? 128 : 100;
      async.runtime.operation_transport_limit = 128;
    }
    rinzu_async_future_t *future = rinzu_aot_worker_request(&context, target, &original, 1);
    assert(rinzu_async_admit(future));
    if (mode == 0)
      model_corrupt_bytes(future->input, future->input_size);
    ModelReturnGate *control = NULL;
    if (mode == 5) {
      control = rinzu_failure_allocate(sizeof(*control));
      future->native_payload = (void *)control;
      assert(!pthread_mutex_init(&control->gate, NULL) &&
             !pthread_cond_init(&control->changed, NULL));
      future->run = model_late_return;
    }
    rinzu_async_start(future);
    if (control) {
      assert(!pthread_mutex_lock(&control->gate));
      while (!control->entered)
        assert(!pthread_cond_wait(&control->changed, &control->gate));
      model_heap_settled(future, 1);
      assert(!atomic_load(&future->physical_done) && future->result_credit &&
             async.runtime.domains == 1);
      assert(rinzu_async_cancel(future));
      control->released = 1;
      assert(!pthread_cond_broadcast(&control->changed));
      assert(!pthread_mutex_unlock(&control->gate));
    }
    rinzu_async_join(future);
    model_heap_settled(future, mode != 0);
    assert(!future->input && !future->input_size && !future->input_credit);
    if (mode == 1)
      model_corrupt_bytes(future->result, future->result_size);
    if (mode == 2 || mode == 3) {
      assert(future->failure.primary == 3 && future->result_rejected && !future->result &&
             !future->observed && !future->observed_packets);
      size_t results = 0;
      for (rinzu_async_credit_t *credit = async.runtime.credits; credit; credit = credit->next)
        results += credit->direction == 1;
      assert(mode == 2 ? results == 0 : results == 2);
    }
    assert(!rinzu_async_materialize(future));
    assert(future->failure.primary == (mode <= 1 ? 6 : mode == 5 ? 2 : 3));
    assert(future->failure.primary_source.file == context.source.file &&
           future->failure.primary_source.start_byte == context.source.start_byte);
    assert(!async.result_roots && (mode > 1 || data->objects == 3));
    if (control)
      assert(!pthread_cond_destroy(&control->changed) && !pthread_mutex_destroy(&control->gate));
    rinzu_async_release(future);
  }
  model_credits_settled(&async.runtime);
  assert(!async.runtime.records);
  while (async.result_roots)
    rinzu_aot_async_release_result(&async, async.result_roots);
  rinzu_managed_collect(data, &context.roots);
  assert(data->objects == 3 && !data->live_views && !data->view_allocations);
  rinzu_managed_leave(&context.roots);
  rinzu_managed_collect(data, &context.roots);
  assert(!data->objects && !context.roots.depth);
  rinzu_managed_detach_roots(data, &context.domain_roots);
  rinzu_managed_dispose(&context.roots);
  rinzu_managed_data_dispose(data);
  rinzu_failure_dispose(&context.pending);
  rinzu_async_dispose(&async.runtime);
  rinzu_owner_transition_finish(rinzu_context_transitions(&context));
  assert(atomic_load(&model_native_live) == process_baseline);
}
int main(void) {
  unsetenv("RINZU_CANCEL_WORKER_AFTER_POLLS");
  for (unsigned round = 0; round < 16; ++round)
    for (unsigned mode = 0; mode < 6; ++mode)
      model_run(mode);
  puts("TYPED_WORKER_RESOURCES=96:baseline");
  return 0;
}
