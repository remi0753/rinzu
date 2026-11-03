#include "rinzu/source_async.h"
#include <assert.h>
#include <sched.h>
#include <unistd.h>

static _Atomic size_t wakes;
static void wake(void *context) {
  assert(*(unsigned *)context == 41);
  atomic_fetch_add_explicit(&wakes, 1, memory_order_relaxed);
}
static rinzu_async_future_t *waiting(rinzu_async_runtime_t *runtime,
                                     rinzu_native_completion_t *carrier) {
  rinzu_async_future_t *future = rinzu_async_new(runtime, RINZU_ASYNC_COMPLETION,
      RINZU_TRANSFER_UNIT, (rinzu_source_position_t){0});
  future->native_completion = carrier;
  return future;
}
static void receive(rinzu_async_future_t *future) {
  while (!future->settled) {
    uint64_t observed = rinzu_async_completion_epoch(future->runtime);
    if (!rinzu_async_try_collect(future))
      rinzu_async_wait_progress(future->runtime, observed);
  }
}
typedef struct {
  rinzu_native_completion_v2_t *carrier;
  _Atomic int begin;
  uint64_t *payload, expected;
} publication_t;
static void *publish(void *raw) {
  publication_t *publication = raw;
  while (!atomic_load_explicit(&publication->begin, memory_order_acquire))
    sched_yield();
  *publication->payload = publication->expected;
  rinzu_native_completion_v2_publish(publication->carrier);
  /* No carrier access here: the Owner may already have freed/reused it. */
  return NULL;
}
static void registration_and_shared_waiters(void) {
  rinzu_async_runtime_t runtime;
  rinzu_async_init(&runtime);
  rinzu_async_future_t *retained[256];
  for (size_t i = 0; i < 128; ++i) {
    rinzu_native_completion_v2_t *carrier = malloc(sizeof(*carrier));
    assert(carrier);
    rinzu_native_completion_v2_init(carrier);
    retained[2 * i] = waiting(&runtime, &carrier->base);
    retained[2 * i + 1] = waiting(&runtime, &carrier->base);
    assert(rinzu_async_cancel(retained[2 * i]));
    uint64_t payload = 0;
    publication_t publication = {.carrier = carrier, .payload = &payload, .expected = i + 1};
    atomic_init(&publication.begin, 0);
    pthread_t producer;
    assert(!pthread_create(&producer, NULL, publish, &publication));
    if (i % 2)
      atomic_store_explicit(&publication.begin, 1, memory_order_release);
    rinzu_async_start(retained[2 * i]);
    rinzu_async_start(retained[2 * i + 1]);
    atomic_store_explicit(&publication.begin, 1, memory_order_release);
    receive(retained[2 * i + 1]);
    assert(atomic_load_explicit(&retained[2 * i]->physical_done, memory_order_acquire));
    assert(payload == i + 1);
    /* Both admitted waits publish before the shared SDK carrier can retire. */
    free(carrier);
    receive(retained[2 * i]);
    assert(retained[2 * i]->failure.primary == 2);
    assert(!retained[2 * i + 1]->failure.primary);
    assert(!pthread_join(producer, NULL));
    (void)rinzu_async_completion_epoch(&runtime);
  }
  assert(runtime.reactor_started == !runtime.native_notifications);
  for (size_t i = 0; i < 256; ++i)
    rinzu_async_release(retained[i]);
  rinzu_async_dispose(&runtime);
}
static void already_done_and_prefix_not_done(void) {
  rinzu_async_runtime_t runtime;
  rinzu_async_init(&runtime);
  for (unsigned i = 0; i < 32; ++i) {
    rinzu_native_completion_v2_t carrier;
    rinzu_native_completion_v2_init(&carrier);
    if (i % 2)
      rinzu_native_completion_v2_publish(&carrier);
    else
      atomic_store_explicit(&carrier.base.done, 1, memory_order_release);
    rinzu_async_future_t *future = waiting(&runtime, &carrier.base);
    rinzu_async_start(future);
    if (!(i % 2)) {
      assert(!rinzu_native_completion_is_done(&carrier.base));
      assert(!rinzu_async_try_collect(future));
      rinzu_native_completion_v2_publish(&carrier);
    }
    receive(future);
    rinzu_async_release(future);
  }
  assert(runtime.reactor_started == !runtime.native_notifications);
  rinzu_async_dispose(&runtime);
}
static void mixed_legacy_and_notified(void) {
  rinzu_async_runtime_t runtime;
  rinzu_async_init(&runtime);
  rinzu_native_completion_t legacy;
  rinzu_native_completion_v2_t notified;
  rinzu_native_completion_init(&legacy);
  rinzu_native_completion_v2_init(&notified);
  rinzu_async_future_t *a = waiting(&runtime, &legacy), *b = waiting(&runtime, &notified.base);
  rinzu_async_start(a);
  rinzu_async_start(b);
  assert(runtime.reactor_started);
  rinzu_native_completion_v2_publish(&notified);
  receive(b);
  assert(!atomic_load_explicit(&a->physical_done, memory_order_acquire));
  atomic_store_explicit(&legacy.done, 1, memory_order_release);
  receive(a);
  rinzu_async_release(a);
  rinzu_async_release(b);
  rinzu_async_dispose(&runtime);
}
static void deferred_notification_after_dispose(void) {
  rinzu_async_runtime_t *runtime = malloc(sizeof(*runtime));
  rinzu_native_completion_v2_t *carrier = malloc(sizeof(*carrier));
  unsigned *context = malloc(sizeof(*context));
  assert(runtime && carrier && context);
  *context = 41;
  rinzu_async_init(runtime);
  rinzu_async_set_completion_wake(runtime, wake, context);
  rinzu_native_completion_v2_init(carrier);
  rinzu_async_future_t *future = waiting(runtime, &carrier->base);
  rinzu_async_start(future);
  /* Pause the producer after its LAST carrier access, before its retained
     target is invoked. This is the exact tail of v2_publish split by a pause. */
  atomic_store_explicit(&carrier->base.done, 1, memory_order_release);
  uintptr_t target = atomic_exchange_explicit(&carrier->notification,
      RINZU_NATIVE_COMPLETION_DONE, memory_order_acq_rel);
  receive(future);
  free(carrier);
  rinzu_async_release(future);
  rinzu_async_dispose(runtime);
  free(runtime);
  free(context);
  size_t before = atomic_load_explicit(&wakes, memory_order_relaxed);
  if (target > RINZU_NATIVE_COMPLETION_DONE) {
    rinzu_native_completion_wake_t *retained = (rinzu_native_completion_wake_t *)target;
    retained->notify(retained);
    retained->release(retained);
  }
  assert(atomic_load_explicit(&wakes, memory_order_relaxed) == before);
}
int main(void) {
  alarm(30);
  atomic_store(&rinzu_resource_enabled, 1);
  assert(sizeof(rinzu_native_completion_t) == 16);
  assert(sizeof(rinzu_native_completion_v2_t) == 24);
  {
    rinzu_native_completion_v2_t header;
    rinzu_native_completion_v2_init(&header);
    assert(rinzu_native_completion_valid(&header.base));
    header.base.size = 16;
    assert(!rinzu_native_completion_valid(&header.base));
    header.base.size = sizeof(header);
    header.base.version = 3;
    assert(!rinzu_native_completion_valid(&header.base));
    rinzu_native_completion_init(&header.base);
    assert(rinzu_native_completion_valid(&header.base));
    header.base.size = sizeof(header);
    assert(!rinzu_native_completion_valid(&header.base));
  }
  registration_and_shared_waiters();
  already_done_and_prefix_not_done();
  mixed_legacy_and_notified();
  deferred_notification_after_dispose();
  for (unsigned i = 0; i < RINZU_RESOURCE_COUNT; ++i)
    assert(!rinzu_resource_counters[i].live);
  return 0;
}
