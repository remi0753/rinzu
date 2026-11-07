/* Appended to the production generated runtime. A physical native return
   owns a strong pin independently of any Future until its Owner settles it. */
#include <assert.h>

static int pin_identity_case(int consume, unsigned identity) {
  RinzuAsyncState async = {0};
  RinzuContext context = {0};
  rinzu_async_init(&async.runtime);
  context.async = &async;
  context.shared_data = &context.data;
  async.root = &context;
  rinzu_managed_data_t *data = rinzu_context_data(&context);
  rinzu_managed_attach_roots(data, &context.domain_roots, &context.roots);
  rinzu_managed_enter(&context.roots);
  if (identity) {
    rinzu_async_future_t *first = rinzu_async_new(&async.runtime, RINZU_ASYNC_READY,
                                                  RINZU_SCALAR_BOOL, (rinzu_source_position_t){0});
    rinzu_async_materialize(first);
    rinzu_async_release(first);
  }
  rinzu_async_future_t *future = rinzu_async_new(&async.runtime, RINZU_ASYNC_READY,
                                                 RINZU_SCALAR_BOOL, (rinzu_source_position_t){0});
  assert(future->identity == identity);
  future->result_slot = 1;
  rinzu_async_materialize(future);
  uint64_t handle = rinzu_async_handle(future);
  uint64_t value = rinzu_managed_string(data, (const unsigned char *)"physical-return", 15);
  RinzuAsyncResultRoot *pin = rinzu_failure_allocate(sizeof(*pin));
  rinzu_managed_attach_roots(data, &pin->entry, &pin->roots);
  rinzu_managed_enter(&pin->roots);
  rinzu_managed_publish(&pin->roots, &value, 1);
  assert(!pin->entry.future_owner);
  async.result_roots = pin;
  if (consume) {
    assert(rinzu_baseline_async_await(&context, (int64_t)handle, 1) == 1);
  } else {
    rinzu_aot_future_sweep(&async);
  }
  if (async.result_roots != pin) {
    fprintf(stderr, "UNRELATED_PIN_LOST:future=%u:path=%s\n", identity, consume ? "consume" : "gc");
    return 17;
  }
  assert(!rinzu_async_lookup(&async.runtime, handle));
  rinzu_managed_collect(data, &context.roots);
  assert(data->objects == 1);
  rinzu_aot_async_release_result(&async, pin);
  rinzu_managed_collect(data, &context.roots);
  assert(!data->objects && !async.result_roots && !async.runtime.records);
  rinzu_managed_leave(&context.roots);
  rinzu_managed_detach_roots(data, &context.domain_roots);
  rinzu_managed_dispose(&context.roots);
  rinzu_managed_data_dispose(data);
  rinzu_failure_dispose(&context.pending);
  rinzu_async_dispose(&async.runtime);
  rinzu_owner_transition_finish(rinzu_context_transitions(&context));
  return 0;
}
int main(void) {
  for (unsigned identity = 0; identity < 2; ++identity)
    for (int consume = 0; consume < 2; ++consume) {
      int status = pin_identity_case(consume, identity);
      if (status)
        return status;
    }
  puts("ASYNC_RESULT_PIN_IDENTITY=4:baseline");
  return 0;
}
