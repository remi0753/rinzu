#include "rinzu/source_async.h"
#include <assert.h>
#include <sched.h>
#include <stdio.h>
#include <unistd.h>
static pthread_mutex_t gate = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t changed = PTHREAD_COND_INITIALIZER;
static int entered, released;
static _Atomic size_t wakes;
static size_t settled_waiters;
static void owner_settled(rinzu_async_ready_waiter_t *waiter) {
  rinzu_async_future_t *future = waiter->future;
  assert(pthread_equal(future->runtime->owner, pthread_self()));
  assert(future->settled && future->failure.primary == 2 && !future->result_slot);
  assert(waiter->readiness->published && !waiter->readiness->received);
  assert(!pthread_mutex_trylock(&future->runtime->gate));
  assert(!pthread_mutex_unlock(&future->runtime->gate));
  ++settled_waiters;
}
static void wake(void *raw) {
  rinzu_async_runtime_t *runtime = raw;
  /* Native notification runs outside the resource/packet gates. */
  assert(!pthread_mutex_trylock(&runtime->gate));
  assert(!pthread_mutex_unlock(&runtime->gate));
  atomic_fetch_add(&wakes, 1);
}
static void slow(rinzu_async_future_t *future) {
  assert(!pthread_equal(future->runtime->owner, pthread_self()));
  assert(!pthread_mutex_lock(&gate));
  entered = 1;
  assert(!pthread_cond_broadcast(&changed));
  while (!released)
    assert(!pthread_cond_wait(&changed, &gate));
  assert(!pthread_mutex_unlock(&gate));
  future->result_slot = 41;
}
static void *foreign_poll(void *raw) {
  (void)rinzu_async_try_collect(raw);
  abort();
}
int main(int argc, char **argv) {
  (void)argc;
  alarm(20);
  rinzu_async_runtime_t runtime;
  rinzu_async_init(&runtime);
  rinzu_async_set_completion_wake(&runtime, wake, &runtime);
  rinzu_async_future_t *future = rinzu_async_worker(&runtime, NULL, NULL, 0, RINZU_SCALAR_I32, slow,
                                                    (rinzu_source_position_t){0});
  if (!strcmp(argv[1], "late-config")) {
    rinzu_async_set_completion_wake(&runtime, wake, &runtime);
    abort();
  }
  if (!strcmp(argv[1], "foreign-poll")) {
    pthread_t thread;
    assert(!pthread_create(&thread, NULL, foreign_poll, future));
    assert(!pthread_join(thread, NULL));
    abort();
  }
  rinzu_activity_board_t *board = rinzu_activity_new();
  rinzu_async_set_readiness(future,
                            rinzu_ready_prepare(board, "physical.worker", 17, 31, 49, 41, 2));
  rinzu_async_ready_waiter_t waiting = {0}, abandoned = {0};
  waiting.on_settled = abandoned.on_settled = owner_settled;
  rinzu_async_waiter_register(future, &waiting,
                              rinzu_ready_prepare(board, "ready.main", 17, 51, 79, 41, 2));
  rinzu_async_waiter_register(future, &abandoned,
                              rinzu_ready_prepare(board, "cancelled.main", 17, 81, 99, 41, 2));
  rinzu_async_waiter_receive(&abandoned);
  assert(!abandoned.on_settled && !settled_waiters);
  assert(!rinzu_async_has_completion(&runtime));
  assert(!rinzu_async_try_collect(future));
  assert(rinzu_async_admit(future));
  rinzu_async_start(future);
  assert(!pthread_mutex_lock(&gate));
  while (!entered)
    assert(!pthread_cond_wait(&changed, &gate));
  assert(!pthread_mutex_unlock(&gate));
  assert(rinzu_async_cancel(future));
  assert(!rinzu_async_has_completion(&runtime));
  assert(!rinzu_async_try_collect(future));
  assert(runtime.domains == 1 && !future->settled && !atomic_load(&wakes));
  assert(!settled_waiters); /* Cancellation is not materialized readiness. */
  assert(!rinzu_activity_stats(board).active);
  assert(!pthread_mutex_lock(&gate));
  released = 1;
  assert(!pthread_cond_broadcast(&changed));
  assert(!pthread_mutex_unlock(&gate));
  while (!atomic_load(&wakes))
    sched_yield();
  assert(rinzu_async_has_completion(&runtime));
  assert(runtime.domains == 1 && runtime.transport_bytes == 4 && !future->joined &&
         !future->settled);
  assert(!settled_waiters); /* The physical publisher never runs Owner hooks. */
  assert(rinzu_activity_stats(board).active == 1);
  assert(rinzu_async_try_collect(future));
  assert(future->joined && future->settled);
  assert(future->failure.primary == 2 && !future->result_slot);
  assert(settled_waiters == 1);
  assert(!rinzu_async_materialize(future) && settled_waiters == 1);
  rinzu_async_ready_waiter_t late = {.on_settled = owner_settled};
  rinzu_async_waiter_register(future, &late,
                              rinzu_ready_prepare(board, "late.main", 17, 101, 119, 41, 2));
  assert(settled_waiters == 2);
  rinzu_async_waiter_receive(&late);
  assert(!late.on_settled);
  assert(!rinzu_async_try_collect(future) && !rinzu_async_has_completion(&runtime));
  assert(!runtime.domains && !runtime.transport_bytes && !runtime.in_flight);
  rinzu_activity_stats_t stats = rinzu_activity_stats(board);
  /* Main is ready after the real packet was received. A cancellation-resumed
     earlier waiter is unlinked and cannot be published again. */
  assert(stats.active == 1 && stats.ready_live == 2 && stats.prepared == stats.ended + 1);
  rinzu_async_waiter_receive(&waiting);
  stats = rinzu_activity_stats(board);
  assert(!stats.active && stats.ready_live == 1 && stats.prepared == stats.ended);
  rinzu_async_release(future);
  rinzu_async_dispose(&runtime);
  assert(!rinzu_activity_stats(board).ready_live);
  rinzu_activity_dispose(board);
  assert(!pthread_cond_destroy(&changed));
  assert(!pthread_mutex_destroy(&gate));
  puts("PASS physical completion wake and Owner-only nonblocking collection");
}
