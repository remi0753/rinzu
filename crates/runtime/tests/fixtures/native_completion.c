#include "rinzu/source_async.h"
#include <assert.h>
#include <stddef.h>
#include <sched.h>
static void timer(rinzu_async_future_t *future) {
  struct timespec duration = {0, 1000000};
  nanosleep(&duration, NULL);
  future->result_slot = 0;
}
static void foreign_wake_only(void *raw) {
  rinzu_async_runtime_t *runtime = raw;
  assert(!pthread_equal(runtime->owner, pthread_self()));
  assert(!pthread_mutex_trylock(&runtime->gate));
  assert(!pthread_mutex_unlock(&runtime->gate));
}
static void immediate_completion(void) {
  rinzu_async_runtime_t runtime;
  rinzu_async_init(&runtime);
  if (!runtime.direct_native_completion) {
    rinzu_async_dispose(&runtime);
    return;
  }
  rinzu_async_set_completion_wake(&runtime, foreign_wake_only, &runtime);
  rinzu_activity_board_t *board = rinzu_activity_new();
  for (size_t index = 0; index < 2048; ++index) {
    rinzu_native_completion_t *native = malloc(sizeof(*native));
    assert(native);
    rinzu_native_completion_init(native);
    atomic_store_explicit(&native->done, 1, memory_order_release);
    rinzu_async_future_t *future = rinzu_async_new(&runtime, RINZU_ASYNC_COMPLETION,
                                                  RINZU_TRANSFER_UNIT,
                                                  (rinzu_source_position_t){0});
    future->native_completion = native;
    rinzu_async_set_readiness(future, rinzu_ready_prepare(board, "native.immediate", 1, 2, 3, 0, 0));
    if (index % 2) {
      assert(rinzu_async_cancel(future));
      assert(!atomic_load(&future->physical_done));
    }
    rinzu_async_start(future);
    assert(atomic_load(&future->physical_done) && !runtime.reactor_started);
    assert(rinzu_async_has_completion(&runtime));
    assert(rinzu_async_try_collect(future));
    assert(future->failure.primary == (index % 2 ? 2 : 0));
    free(native);
    /* Collection may outlive its SDK carrier. Subsequent scans must only
       inspect physical_done, even before the Future itself is reclaimed. */
    assert(!rinzu_async_has_completion(&runtime));
    assert(rinzu_async_completion_epoch(&runtime) == index + 1);
    assert(!rinzu_async_try_collect(future));
    rinzu_async_release(future);
  }
  assert(!rinzu_activity_stats(board).ready_live);
  rinzu_async_dispose(&runtime);
  rinzu_activity_dispose(board);
}
static void owner_progress_without_reactor(void) {
  for (unsigned mode = 0; mode < 4; ++mode) {
    rinzu_async_runtime_t runtime;
    rinzu_async_init(&runtime);
    if (!runtime.direct_native_completion) {
      rinzu_async_dispose(&runtime);
      return;
    }
    rinzu_native_completion_t native;
    rinzu_native_completion_init(&native);
    rinzu_async_future_t *future = rinzu_async_new(&runtime, RINZU_ASYNC_COMPLETION,
                                                  RINZU_TRANSFER_UNIT,
                                                  (rinzu_source_position_t){0});
    future->native_completion = &native;
    /* Simulate completion in the registration/Owner-progress gap, without a
       helper thread that could accidentally make a lost-wake test pass. */
    future->executed = 1;
    runtime.native_completions = future;
    assert(rinzu_async_cancel(future));
    assert(!rinzu_async_try_collect(future));
    atomic_store_explicit(&native.done, 1, memory_order_release);
    if (mode == 0) assert(rinzu_async_has_completion(&runtime));
    if (mode == 1) assert(rinzu_async_try_collect(future));
    if (mode == 2) assert(rinzu_async_completion_epoch(&runtime) == 1);
    if (mode == 3) rinzu_async_wait_progress(&runtime, 0);
    assert(atomic_load(&future->physical_done));
    assert(runtime.completion_epoch == 1 && !runtime.reactor_started);
    if (!future->settled) assert(rinzu_async_try_collect(future));
    assert(future->failure.primary == 2);
    rinzu_async_release(future);
    rinzu_async_dispose(&runtime);
  }
}
static void shared_carrier_cleanup(void) {
  rinzu_async_runtime_t runtime;
  rinzu_async_init(&runtime);
  rinzu_native_completion_t *native = malloc(sizeof(*native));
  assert(native);
  rinzu_native_completion_init(native);
  rinzu_async_future_t *first = rinzu_async_new(&runtime, RINZU_ASYNC_COMPLETION,
                                               RINZU_TRANSFER_UNIT, (rinzu_source_position_t){0});
  first->native_completion = native;
  if (runtime.direct_native_completion) {
    first->executed = 1;
    runtime.native_completions = first;
  } else {
    rinzu_async_start(first);
  }
  assert(rinzu_async_cancel(first));
  atomic_store_explicit(&native->done, 1, memory_order_release);
  rinzu_async_future_t *cleanup = rinzu_async_new(&runtime, RINZU_ASYNC_COMPLETION,
                                                 RINZU_TRANSFER_UNIT, (rinzu_source_position_t){0});
  cleanup->native_completion = native;
  rinzu_async_start(cleanup);
  while (!cleanup->settled) {
    uint64_t observed = rinzu_async_completion_epoch(&runtime);
    if (!rinzu_async_try_collect(cleanup))
      rinzu_async_wait_progress(&runtime, observed);
  }
  assert(atomic_load(&first->physical_done));
  free(native);
  assert(rinzu_async_completion_epoch(&runtime) == 2);
  assert(rinzu_async_try_collect(first));
  assert(first->failure.primary == 2);
  rinzu_async_release(first);
  rinzu_async_release(cleanup);
  rinzu_async_dispose(&runtime);
}
typedef struct {
  rinzu_async_runtime_t *runtime;
  rinzu_async_future_t *wanted;
} publication_t;
static void *interrupted_publication(void *raw) {
  publication_t *publication = raw;
  rinzu_async_lock(publication->runtime);
  assert(rinzu_async_publish_native_locked(publication->wanted));
  /* Model an OS preemption after the first record's release-store, before
     the rest of a shared-carrier publication/retirement batch finishes. */
  struct timespec gap = {0, 5000000};
  nanosleep(&gap, NULL);
  (void)rinzu_async_scan_native_locked(publication->runtime);
  rinzu_async_unlock(publication->runtime);
  return NULL;
}
static void collection_joins_publication(void) {
  rinzu_async_runtime_t runtime;
  rinzu_async_init(&runtime);
  rinzu_native_completion_t *native = malloc(sizeof(*native));
  assert(native);
  rinzu_native_completion_init(native);
  atomic_store_explicit(&native->done, 1, memory_order_release);
  rinzu_async_future_t *futures[2];
  for (size_t index = 0; index < 2; ++index) {
    futures[index] = rinzu_async_new(&runtime, RINZU_ASYNC_COMPLETION, RINZU_TRANSFER_UNIT,
                                      (rinzu_source_position_t){0});
    futures[index]->native_completion = native;
    futures[index]->executed = 1;
    futures[index]->completion_next = runtime.native_completions;
    runtime.native_completions = futures[index];
  }
  publication_t publication = {&runtime, futures[1]};
  pthread_t producer;
  assert(!pthread_create(&producer, NULL, interrupted_publication, &publication));
  while (!atomic_load_explicit(&futures[1]->physical_done, memory_order_acquire))
    sched_yield();
  assert(rinzu_async_try_collect(futures[1]));
  assert(atomic_load_explicit(&futures[0]->physical_done, memory_order_acquire));
  free(native);
  assert(!pthread_join(producer, NULL));
  assert(rinzu_async_try_collect(futures[0]));
  rinzu_async_release(futures[0]);
  rinzu_async_release(futures[1]);
  rinzu_async_dispose(&runtime);
}
int main(void) {
  alarm(10); /* A lost Owner wake must fail rather than hang the test runner. */
  assert(sizeof(rinzu_native_completion_t) == 16);
  assert(offsetof(rinzu_native_completion_t, done) == 8);
  assert(offsetof(rinzu_native_completion_t, cancelled) == 12);
  immediate_completion();
  owner_progress_without_reactor();
  shared_carrier_cleanup();
  collection_joins_publication();
  rinzu_async_runtime_t runtime;
  rinzu_async_init(&runtime);
  rinzu_native_completion_t native[64];
  rinzu_async_future_t *futures[64];
  for (size_t index = 0; index < 64; index++) {
    rinzu_native_completion_init(&native[index]);
    futures[index] = rinzu_async_new(&runtime, RINZU_ASYNC_COMPLETION, RINZU_TRANSFER_UNIT,
                                     (rinzu_source_position_t){0});
    futures[index]->native_completion = &native[index];
    rinzu_async_start(futures[index]);
    assert(!futures[index]->started); /* No per-wait OS thread. */
    if (index % 2 == 0) {
      assert(rinzu_async_cancel(futures[index]));
      assert(atomic_load(&native[index].cancelled));
    }
    assert(!rinzu_async_try_collect(futures[index]));
  }
  assert(runtime.reactor_started);
  /* Both the reactor and the Owner wait on completion_changed. A timer
     publication must wake the Owner even if the reactor was first asleep. */
  for (size_t index = 0; index < 64; index++) {
    rinzu_async_future_t *tick = rinzu_async_new(&runtime, RINZU_ASYNC_TIMER, RINZU_TRANSFER_UNIT,
                                                 (rinzu_source_position_t){0});
    tick->run = timer;
    rinzu_async_start(tick);
    for (;;) {
      uint64_t observed = rinzu_async_completion_epoch(&runtime);
      if (rinzu_async_try_collect(tick))
        break;
      rinzu_async_wait_progress(&runtime, observed);
    }
    rinzu_async_release(tick);
  }
  for (size_t index = 0; index < 64; index++)
    atomic_store_explicit(&native[index].done, 1, memory_order_release);
  for (size_t index = 0; index < 64; index++) {
    for (;;) {
      uint64_t observed = rinzu_async_completion_epoch(&runtime);
      if (rinzu_async_try_collect(futures[index]))
        break;
      rinzu_async_wait_progress(&runtime, observed);
    }
    assert(futures[index]->settled);
    assert(futures[index]->failure.primary == (index % 2 == 0 ? 2 : 0));
    rinzu_async_release(futures[index]);
  }
  rinzu_async_dispose(&runtime);
  assert(!runtime.records && !runtime.domains && !runtime.in_flight);
  alarm(0);
  return 0;
}
