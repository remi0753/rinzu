#include "rinzu/source_async.h"
#include <assert.h>
#include <sched.h>
#include <stdbool.h>
#include <stdio.h>
#include <unistd.h>

typedef struct {
  rinzu_async_future_t *controlled, *alias, *unit;
} waiters_t;

static void initialize(rinzu_native_completion_v3_t *carrier, int64_t value) {
  rinzu_native_completion_v2_init(&carrier->completion);
  carrier->completion.base.version = 3;
  carrier->completion.base.size = sizeof(*carrier);
  carrier->result = value;
}

static rinzu_async_future_t *wait_for(rinzu_async_runtime_t *runtime,
                                      rinzu_native_completion_v3_t *carrier, unsigned kind) {
  rinzu_async_future_t *future =
      rinzu_async_new(runtime, RINZU_ASYNC_COMPLETION, kind, (rinzu_source_position_t){0});
  future->native_completion = &carrier->completion.base;
  rinzu_async_start(future);
  return future;
}

static waiters_t admit(rinzu_async_runtime_t *runtime, rinzu_native_completion_v3_t *carrier) {
  return (waiters_t){wait_for(runtime, carrier, RINZU_SCALAR_I64),
                     wait_for(runtime, carrier, RINZU_SCALAR_I64),
                     wait_for(runtime, carrier, RINZU_TRANSFER_UNIT)};
}

static void receive(rinzu_async_future_t *future) {
  while (!future->settled) {
    uint64_t epoch = rinzu_async_completion_epoch(future->runtime);
    if (!rinzu_async_try_collect(future))
      rinzu_async_wait_progress(future->runtime, epoch);
  }
}

static void collect(waiters_t waiters) {
  receive(waiters.controlled);
  receive(waiters.alias);
  receive(waiters.unit);
}

static void release(waiters_t waiters) {
  rinzu_async_release(waiters.controlled);
  rinzu_async_release(waiters.alias);
  rinzu_async_release(waiters.unit);
}

typedef struct {
  rinzu_native_completion_v3_t *carrier;
  _Atomic unsigned start;
} publication_t;

static void *publish_concurrently(void *raw) {
  publication_t *publication = raw;
  while (!atomic_load_explicit(&publication->start, memory_order_acquire))
    sched_yield();
  rinzu_native_completion_v2_publish(&publication->carrier->completion);
  return NULL;
}

static void race_publication_with_control(rinzu_async_runtime_t *runtime) {
  for (unsigned iteration = 0; iteration < 32; ++iteration) {
    rinzu_native_completion_v3_t *carrier = malloc(sizeof(*carrier));
    assert(carrier);
    initialize(carrier, (int64_t)iteration);
    waiters_t waiters = admit(runtime, carrier);
    publication_t publication = {.carrier = carrier};
    atomic_init(&publication.start, 0);
    pthread_t producer;
    assert(!pthread_create(&producer, NULL, publish_concurrently, &publication));
    atomic_store_explicit(&publication.start, 1, memory_order_release);
    bool controlled = iteration % 2 ? rinzu_async_timeout(waiters.controlled)
                                    : rinzu_async_cancel(waiters.controlled);
    assert(!rinzu_async_cancel(waiters.controlled));
    assert(!rinzu_async_timeout(waiters.controlled));
    collect(waiters);
    assert(!pthread_join(producer, NULL));
    /* Either winner is valid. The accepted Bool determines this Future's
       control result; the same SDK publication still supplies every alias. */
    assert(waiters.controlled->failure.primary == (controlled ? 2u : 0u));
    assert(atomic_load(&carrier->completion.base.cancelled) == (controlled ? 1u : 0u));
    assert(!waiters.alias->failure.primary);
    assert((int64_t)rinzu_async_materialize(waiters.alias) == (int64_t)iteration);
    free(carrier);
    assert(!rinzu_async_cancel(waiters.alias));
    assert((int64_t)rinzu_async_materialize(waiters.alias) == (int64_t)iteration);
    release(waiters);
  }
}

int main(void) {
  alarm(30);
  atomic_store(&rinzu_resource_enabled, 1);
  rinzu_async_runtime_t runtime;
  rinzu_async_init(&runtime);
  rinzu_native_completion_v3_t *carrier = malloc(sizeof(*carrier));
  assert(carrier);

  /* NLS03/NLS06: timeout wins once without publishing physical completion. */
  initialize(carrier, INT64_MAX);
  waiters_t old = admit(&runtime, carrier);
  assert(rinzu_async_timeout(old.controlled));
  assert(!rinzu_async_timeout(old.controlled));
  assert(!rinzu_async_cancel(old.controlled));
  assert(atomic_load(&carrier->completion.base.cancelled));
  assert(!rinzu_async_try_collect(old.controlled));
  assert(!atomic_load_explicit(&old.controlled->physical_done, memory_order_acquire));

  /* Split publication from its retained notify tail. Collection must copy
     every admitted result before this carrier can be reused. */
  rinzu_native_completion_wake_t *late =
      rinzu_native_completion_v2_publish_target(&carrier->completion);
  collect(old);
  assert(old.controlled->failure.primary == 2);
  assert(!old.alias->failure.primary);
  assert((int64_t)rinzu_async_materialize(old.alias) == INT64_MAX);

  /* NLS05/NLS07: reuse the address only after all old accesses have ended.
     Old Future controls and aliases must not touch the new generation. */
  initialize(carrier, INT64_MIN);
  waiters_t current = admit(&runtime, carrier);
  assert(!rinzu_async_cancel(old.alias));
  assert(!rinzu_async_timeout(old.controlled));
  assert(!atomic_load(&carrier->completion.base.cancelled));
  assert((int64_t)rinzu_async_materialize(old.alias) == INT64_MAX);

  atomic_store_explicit(&carrier->completion.base.done, 1, memory_order_release);
  if (late) {
    late->notify(late);
    late->release(late);
  }
  assert(!rinzu_native_completion_is_done(&carrier->completion.base));
  assert(!rinzu_async_try_collect(current.alias));
  assert(!atomic_load_explicit(&current.alias->physical_done, memory_order_acquire));

  rinzu_native_completion_v2_publish(&carrier->completion);
  collect(current);
  assert((int64_t)rinzu_async_materialize(current.alias) == INT64_MIN);
  assert(!current.controlled->failure.primary);
  free(carrier);

  /* Reawait and late control use retained results, including after free. */
  assert((int64_t)rinzu_async_materialize(old.alias) == INT64_MAX);
  assert((int64_t)rinzu_async_materialize(current.alias) == INT64_MIN);
  assert(!rinzu_async_timeout(current.alias));
  assert(!rinzu_async_cancel(old.controlled));
  release(old);
  release(current);
  race_publication_with_control(&runtime);
  rinzu_async_dispose(&runtime);
  for (unsigned i = 0; i < RINZU_RESOURCE_COUNT; ++i)
    assert(!rinzu_resource_counters[i].live);
  puts("native-completion-generations:timeout;reuse;late-notify;aliases;publication-control-race;"
       "resources-zero");
}
