#include <assert.h>
#include <sched.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

static _Atomic int prefix_observed, exchange_allowed;
static void publication_event(const char *kind, uint64_t id, int64_t a, int64_t b) {
  (void)id;
  (void)a;
  (void)b;
  if (!strcmp(kind, "sdk_prefix")) {
    /* This marker deliberately does not publish the SDK's payload to the Owner. */
    atomic_store_explicit(&prefix_observed, 1, memory_order_relaxed);
    while (!atomic_load_explicit(&exchange_allowed, memory_order_acquire))
      sched_yield();
  }
}
#define RINZU_NATIVE_LIFETIME_EVENT(kind, id, a, b) publication_event(kind, id, a, b)
#include "rinzu/source_async.h"

typedef struct {
  rinzu_native_completion_v3_t *carrier;
  int64_t value;
} payload_t;

static void *publish(void *argument) {
  payload_t *payload = argument;
  payload->carrier->result = payload->value;
  return rinzu_native_completion_v2_publish_target(&payload->carrier->completion);
}

int main(void) {
  rinzu_async_runtime_t runtime;
  rinzu_async_init(&runtime);
  for (int64_t n = 1; n <= 64; ++n) {
    rinzu_native_completion_v3_t carrier;
    rinzu_native_completion_v2_init(&carrier.completion);
    carrier.completion.base.version = 3;
    carrier.completion.base.size = sizeof(carrier);
    carrier.result = 0;
    atomic_store_explicit(&prefix_observed, 0, memory_order_relaxed);
    atomic_store_explicit(&exchange_allowed, 0, memory_order_relaxed);

    rinzu_async_future_t *future = rinzu_async_new(&runtime, RINZU_ASYNC_COMPLETION,
                                                   RINZU_SCALAR_I64, (rinzu_source_position_t){0});
    future->native_completion = &carrier.completion.base;
    rinzu_async_start(future);
    payload_t payload = {&carrier, INT64_C(0x46ab3907) * n + n};
    pthread_t sdk;
    assert(!pthread_create(&sdk, NULL, publish, &payload));
    while (!atomic_load_explicit(&prefix_observed, memory_order_relaxed))
      sched_yield();
    assert(!rinzu_async_try_collect(future));
    assert(!atomic_load(&future->physical_done));
    atomic_store_explicit(&exchange_allowed, 1, memory_order_release);

    while (!rinzu_async_try_collect(future))
      sched_yield();
    assert(future->settled && !future->failure.primary);
    assert(future->result_slot == (uint64_t)payload.value);
    /* Observe the payload before any producer join or SDK notification. */
    void *captured = NULL;
    assert(!pthread_join(sdk, &captured));
    rinzu_native_completion_wake_t *target = captured;
    assert(target == &runtime.native_wake->header);
    target->notify(target);
    target->release(target);
    rinzu_async_release(future);
  }
  rinzu_async_dispose(&runtime);
  puts("PUBLISHED NATIVE PAYLOAD C rounds=64");
  return 0;
}
