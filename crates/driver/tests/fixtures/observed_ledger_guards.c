/* Appended to actual generated C. The guard keeps both Future/result and
   unrelated physical-return pins alive independently of parent ledgers. */
#include <assert.h>
#ifndef TEST_LEDGER_POLICY
#define TEST_LEDGER_POLICY 0
#endif
static RinzuAsyncResultRoot *ledger_pin(RinzuAsyncState *async, rinzu_managed_data_t *data,
                                        uint64_t value, uint64_t future) {
  RinzuAsyncResultRoot *pin = rinzu_failure_allocate(sizeof(*pin));
  pin->future_handle = future;
  rinzu_managed_attach_roots(data, &pin->entry, &pin->roots);
  pin->entry.future_owner = future;
  rinzu_managed_enter(&pin->roots);
  rinzu_managed_publish(&pin->roots, &value, 1);
  pin->next = async->result_roots;
  async->result_roots = pin;
  return pin;
}
static void ledger_clear_failure(RinzuContext *context) {
  rinzu_failure_release(context->pending.current);
  context->pending.current = (rinzu_failure_t){0};
}
static void ledger_case(unsigned failure, unsigned consume_after) {
  RinzuAsyncState async = {0};
  RinzuContext context = {0};
  rinzu_async_init(&async.runtime);
  rinzu_child_init(&async.children);
  async.runtime.scope = &async.children;
  context.async = &async;
  context.shared_data = &context.data;
  async.root = &context;
  rinzu_managed_data_t *data = rinzu_context_data(&context);
  rinzu_managed_attach_roots(data, &context.domain_roots, &context.roots);
  rinzu_managed_enter(&context.roots);
  data->future_context = &async;
  data->future_begin = rinzu_aot_future_begin;
  data->future_mark = rinzu_aot_future_mark;
  data->future_sweep = rinzu_aot_future_sweep;
  data->future_due = rinzu_aot_future_due;
  rinzu_async_future_t *future = rinzu_async_new(&async.runtime, RINZU_ASYNC_TASK,
                                                 failure ? RINZU_SCALAR_I64 : RINZU_ASYNC_MANAGED,
                                                 (rinzu_source_position_t){9, 10, 12, 1});
  uint64_t handle = rinzu_async_handle(future);
  uint64_t physical = rinzu_managed_string(data, (const unsigned char *)"physical-return", 15);
  RinzuAsyncResultRoot *pin = ledger_pin(&async, data, physical, 0), *owned = NULL;
  uint64_t result = 0;
  if (!failure) {
    result = rinzu_managed_string(data, (const unsigned char *)"managed-result", 14);
    owned = ledger_pin(&async, data, result, handle);
    future->gc_result = result;
  } else {
    future->failure.primary = failure == 1 ? 5 : 2;
    future->failure.primary_source = (rinzu_source_position_t){9, 10, 12, 1};
    if (failure == 2) {
      assert(!rinzu_child_expect_control(&async.children, future->identity));
      rinzu_child_append(&future->failure, 5, (rinzu_source_position_t){8, 20, 22, 1});
    }
  }
  future->result_slot = result;
  rinzu_async_materialize(future);
  assert(async.children.child_count == 1 && future->scope == &async.children);
  rinzu_managed_publish(&context.roots, &handle, 1);
  for (unsigned repetition = 0; repetition < 3; ++repetition) {
    assert((uint64_t)rinzu_baseline_async_await(&context, (int64_t)handle, 0) == result);
    unsigned retired = TEST_LEDGER_POLICY == 2 || (TEST_LEDGER_POLICY == 1 && !failure);
    assert(async.children.child_count == (retired ? 0u : 1u));
    assert((future->scope == NULL) == retired);
    assert(rinzu_async_find(&async.runtime, handle) == future && future->settled);
    /* A managed Task's logical settlement does not require this native flag.
       Ledger retirement must neither reinterpret it nor publish it early. */
    assert(!atomic_load_explicit(&future->physical_done, memory_order_acquire));
    assert(context.pending.current.primary == (failure ? (failure == 1 ? 5 : 2) : 0));
    if (failure) {
      assert(context.pending.current.primary_source.file == 9);
      assert(future->failure.primary == context.pending.current.primary);
      if (failure == 2)
        assert(context.pending.current.head && context.pending.current.head->cause == 5);
    }
    assert(async.result_roots == (owned ? owned : pin));
    if (owned)
      assert(future->gc_result == result && owned->next == pin);
    ledger_clear_failure(&context);
    rinzu_managed_collect(data, &context.roots);
    assert(rinzu_async_lookup(&async.runtime, handle) == future);
    assert(rinzu_managed_lookup(data, physical));
    if (owned)
      assert(rinzu_managed_lookup(data, result));
  }
  if (consume_after) {
    assert((uint64_t)rinzu_baseline_async_await(&context, (int64_t)handle, 1) == result);
    ledger_clear_failure(&context);
    assert(!rinzu_async_lookup(&async.runtime, handle) && !async.children.child_count);
    assert(async.result_roots == pin);
    owned = NULL;
  }
  rinzu_managed_publish(&context.roots, NULL, 0);
  rinzu_managed_collect(data, &context.roots);
  assert(!rinzu_async_lookup(&async.runtime, handle) && !async.runtime.records);
  assert(!async.children.child_count && async.result_roots == pin);
  assert(data->objects == 1 && rinzu_managed_lookup(data, physical));
  rinzu_aot_async_release_result(&async, pin);
  rinzu_managed_collect(data, &context.roots);
  assert(!data->objects && !async.result_roots);
  assert(!rinzu_child_begin_close(&async.children, NULL));
  assert(!rinzu_child_finish_close(&async.children));
  rinzu_child_dispose(&async.children);
  async.runtime.scope = NULL;
  rinzu_managed_leave(&context.roots);
  rinzu_managed_detach_roots(data, &context.domain_roots);
  rinzu_managed_dispose(&context.roots);
  rinzu_managed_data_dispose(data);
  rinzu_failure_dispose(&context.pending);
  rinzu_async_dispose(&async.runtime);
  rinzu_owner_transition_finish(rinzu_context_transitions(&context));
}
int main(void) {
  for (unsigned failure = 0; failure < 3; ++failure)
    for (unsigned consume = 0; consume < 2; ++consume)
      ledger_case(failure, consume);
  puts("OBSERVED_LEDGER_ALIAS_PINS=6:ok");
  return 0;
}
