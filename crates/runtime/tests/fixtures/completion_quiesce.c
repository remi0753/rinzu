#include "rinzu/source_async.h"
#include <assert.h>
#include <sched.h>
#include <unistd.h>

typedef struct {
  pthread_mutex_t gate;
  pthread_cond_t changed;
  int entered, released, returned, alive;
  _Atomic int clearing, cleared;
} target_t;

static void delayed_wake(void *raw) {
  target_t *target = raw;
  assert(!pthread_mutex_lock(&target->gate));
  assert(target->alive);
  target->entered = 1;
  assert(!pthread_cond_broadcast(&target->changed));
  while (!target->released)
    assert(!pthread_cond_wait(&target->changed, &target->gate));
  /* The Owner must keep the native target alive after physical publication. */
  assert(target->alive);
  target->returned = 1;
  assert(!pthread_mutex_unlock(&target->gate));
}
static void *release_wake(void *raw) {
  target_t *target = raw;
  while (!atomic_load_explicit(&target->clearing, memory_order_acquire))
    sched_yield();
  struct timespec gap = {0, 20000000};
  assert(!nanosleep(&gap, NULL));
  assert(!atomic_load_explicit(&target->cleared, memory_order_acquire));
  assert(!pthread_mutex_lock(&target->gate));
  target->released = 1;
  assert(!pthread_cond_broadcast(&target->changed));
  assert(!pthread_mutex_unlock(&target->gate));
  return NULL;
}
static void worker(rinzu_async_future_t *future) {
  future->result_slot = 1;
}
static void *publish_notified(void *raw) {
  rinzu_native_completion_v2_publish(raw);
  return NULL;
}
int main(int argc, char **argv) {
  alarm(15);
  rinzu_async_runtime_t runtime;
  rinzu_async_init(&runtime);
  /* Force the publisher thread even if the Owner observes done immediately. */
  runtime.direct_native_completion = 0;
  int notified = argc > 1 && !strcmp(argv[1], "notified");
  if (argc > 1 && !notified) {
    if (!strcmp(argv[1], "late-start")) {
      rinzu_async_clear_completion_wake(&runtime);
      rinzu_async_future_t *future = rinzu_async_new(&runtime, RINZU_ASYNC_READY,
          RINZU_TRANSFER_UNIT, (rinzu_source_position_t){0});
      rinzu_async_start(future);
    } else if (!strcmp(argv[1], "pending-helper")) {
      rinzu_async_future_t *future = rinzu_async_new(&runtime, RINZU_ASYNC_WORKER,
          RINZU_TRANSFER_UNIT, (rinzu_source_position_t){0});
      future->run = worker;
      rinzu_async_start(future);
      rinzu_async_clear_completion_wake(&runtime);
    } else {
      rinzu_native_completion_t native;
      rinzu_native_completion_init(&native);
      rinzu_async_future_t *future = rinzu_async_new(&runtime, RINZU_ASYNC_COMPLETION,
          RINZU_TRANSFER_UNIT, (rinzu_source_position_t){0});
      future->native_completion = &native;
      rinzu_async_start(future);
      rinzu_async_clear_completion_wake(&runtime);
    }
    return 99;
  }
  target_t *target = calloc(1, sizeof(*target));
  assert(target);
  assert(!pthread_mutex_init(&target->gate, NULL));
  assert(!pthread_cond_init(&target->changed, NULL));
  atomic_init(&target->clearing, 0);
  atomic_init(&target->cleared, 0);
  target->alive = 1;
  rinzu_async_set_completion_wake(&runtime, delayed_wake, target);
  rinzu_native_completion_t *native = malloc(notified ? sizeof(rinzu_native_completion_v2_t)
                                                      : sizeof(*native));
  assert(native);
  if (notified)
    rinzu_native_completion_v2_init((rinzu_native_completion_v2_t *)native);
  else
    rinzu_native_completion_init(native);
  rinzu_async_future_t *future = rinzu_async_new(&runtime, RINZU_ASYNC_COMPLETION,
      RINZU_TRANSFER_UNIT, (rinzu_source_position_t){0});
  future->native_completion = native;
  rinzu_async_start(future);
  pthread_t publisher;
  if (notified)
    assert(!pthread_create(&publisher, NULL, publish_notified, native));
  else
    atomic_store_explicit(&native->done, 1, memory_order_release);
  assert(!pthread_mutex_lock(&target->gate));
  while (!target->entered)
    assert(!pthread_cond_wait(&target->changed, &target->gate));
  assert(!pthread_mutex_unlock(&target->gate));
  (void)rinzu_async_completion_epoch(&runtime);
  assert(atomic_load_explicit(&future->physical_done, memory_order_acquire));
  assert(rinzu_async_try_collect(future));
  free(native);
  rinzu_async_release(future);
  pthread_t release;
  assert(!pthread_create(&release, NULL, release_wake, target));
  atomic_store_explicit(&target->clearing, 1, memory_order_release);
  rinzu_async_clear_completion_wake(&runtime);
  assert(target->returned && !runtime.completion_wake && !runtime.wake_context);
  atomic_store_explicit(&target->cleared, 1, memory_order_release);
  assert(!pthread_join(release, NULL));
  if (notified)
    assert(!pthread_join(publisher, NULL));
  target->alive = 0;
  assert(!pthread_cond_destroy(&target->changed));
  assert(!pthread_mutex_destroy(&target->gate));
  free(target);
  /* Disposal is idempotent with respect to the already quiesced wake target. */
  rinzu_async_dispose(&runtime);
  alarm(0);
  return 0;
}
