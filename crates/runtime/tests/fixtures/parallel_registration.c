#include <assert.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

static void event(const char *, uint64_t, int64_t, int64_t);
#define RINZU_NATIVE_LIFETIME_EVENT(k, id, a, b) event(k, id, a, b)
#define RINZU_NATIVE_WAKE_BIND_EVENT(f, target)                                                    \
  event("wake_bind", (f)->identity, (intptr_t)(target), 0)
#define RINZU_NATIVE_WAKE_FREE_EVENT(target) event("target_identity_free", 0, (intptr_t)(target), 0)
#include "rinzu/source_async.h"

enum schedule { EARLY, BETWEEN, FOREIGN, FOREIGN_THEN_PUBLISH };
enum point { RETAINED, FOREIGN_CAS, REGISTERED, COLLECT };
static enum schedule schedule;
static unsigned first, version;
static _Thread_local unsigned domain;
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t changed = PTHREAD_COND_INITIALIZER;
static int reached[2][4], proceed[2][4];
static rinzu_async_runtime_t runtimes[2];
static rinzu_native_completion_v2_t *carrier;
static _Atomic unsigned wakes[2];

static void row(const char *kind, uint64_t id, int64_t a, int64_t b) {
  flockfile(stdout);
  printf("%u,%s,%" PRIu64 ",%" PRId64 ",%" PRId64 "\n", domain, kind, id, a, b);
  funlockfile(stdout);
}

static void pause_at(enum point point) {
  assert(!pthread_mutex_lock(&lock));
  reached[domain][point] = 1;
  assert(!pthread_cond_broadcast(&changed));
  while (!proceed[domain][point])
    assert(!pthread_cond_wait(&changed, &lock));
  assert(!pthread_mutex_unlock(&lock));
}

static void wait_at(unsigned who, enum point point) {
  assert(!pthread_mutex_lock(&lock));
  while (!reached[who][point])
    assert(!pthread_cond_wait(&changed, &lock));
  assert(!pthread_mutex_unlock(&lock));
}

static void release(unsigned who, enum point point) {
  assert(!pthread_mutex_lock(&lock));
  proceed[who][point] = 1;
  assert(!pthread_cond_broadcast(&changed));
  assert(!pthread_mutex_unlock(&lock));
}

static size_t refs(rinzu_async_native_wake_t *target) {
  return atomic_load_explicit(&target->references, memory_order_acquire);
}

static void event(const char *kind, uint64_t id, int64_t a, int64_t b) {
  if (!strcmp(kind, "register_drop")) {
    rinzu_async_native_wake_t *target = runtimes[domain].native_wake;
    row("rollback_refs", id, (intptr_t)&target->header, refs(target));
  }
  row(kind, id, a, b);
  if (!strcmp(kind, "register_retain"))
    pause_at(RETAINED);
  if (schedule == FOREIGN_THEN_PUBLISH && domain != first && !strcmp(kind, "register_cas"))
    pause_at(FOREIGN_CAS);
  if (!strcmp(kind, "registered"))
    pause_at(REGISTERED);
}

static void wake(void *raw) {
  _Atomic unsigned *count = raw;
  atomic_fetch_add_explicit(count, 1, memory_order_relaxed);
}

static void *owner(void *argument) {
  domain = (unsigned)(uintptr_t)argument;
  rinzu_async_runtime_t *runtime = &runtimes[domain];
  rinzu_async_init(runtime);
  rinzu_async_set_completion_wake(runtime, wake, &wakes[domain]);
  rinzu_async_future_t *future = rinzu_async_new(
      runtime, RINZU_ASYNC_COMPLETION, version == 3 ? RINZU_SCALAR_I64 : RINZU_TRANSFER_UNIT,
      (rinzu_source_position_t){0});
  future->native_completion = &carrier->base;
  rinzu_async_start(future);
  pause_at(COLLECT);
  rinzu_async_native_progress(runtime);
  assert(rinzu_async_try_collect(future));
  assert(future->settled && !future->failure.primary &&
         future->result_slot == (version == 3 ? 7183 : 0));
  rinzu_async_release(future);
  rinzu_async_dispose(runtime);
  return NULL;
}

static rinzu_native_completion_wake_t *publish(void) {
  rinzu_native_completion_wake_t *target = rinzu_native_completion_v2_publish_target(carrier);
  assert(atomic_load(&carrier->notification) == RINZU_NATIVE_COMPLETION_DONE);
  row("capture_target", 0, (intptr_t)target, 0);
  return target;
}

int main(int argc, char **argv) {
  assert(argc == 4);
  setvbuf(stdout, NULL, _IOLBF, 0);
  version = (unsigned)atoi(argv[1]);
  first = (unsigned)atoi(argv[2]);
  schedule = (enum schedule)atoi(argv[3]);
  assert((version == 2 || version == 3) && first < 2 && schedule <= FOREIGN_THEN_PUBLISH);
  unsigned other = 1 - first;
  rinzu_native_completion_v3_t data;
  carrier = &data.completion;
  rinzu_native_completion_v2_init(carrier);
  if (version == 3) {
    carrier->base.version = 3;
    carrier->base.size = sizeof(data);
    data.result = 7183;
  }
  pthread_t threads[2];
  for (unsigned d = 0; d < 2; ++d)
    assert(!pthread_create(&threads[d], NULL, owner, (void *)(uintptr_t)d));
  for (unsigned d = 0; d < 2; ++d)
    wait_at(d, RETAINED);
  assert(!atomic_load(&carrier->notification));
  assert(runtimes[0].native_wake != runtimes[1].native_wake);
  for (unsigned d = 0; d < 2; ++d)
    assert(refs(runtimes[d].native_wake) == 2);

  if (schedule == EARLY)
    assert(!publish());
  release(first, RETAINED);
  wait_at(first, REGISTERED);
  release(first, REGISTERED);
  wait_at(first, COLLECT);
  rinzu_native_completion_wake_t *target = NULL;
  if (schedule == BETWEEN) {
    target = publish();
    assert(target == &runtimes[first].native_wake->header);
  }
  release(other, RETAINED);
  if (schedule == FOREIGN_THEN_PUBLISH) {
    wait_at(other, FOREIGN_CAS);
    target = publish();
    assert(target == &runtimes[first].native_wake->header);
    release(other, FOREIGN_CAS);
  }
  if (schedule >= FOREIGN) {
    /* The rejected Owner terminates the child after returning its candidate. */
    assert(!pthread_join(threads[other], NULL));
    return 99;
  }
  wait_at(other, REGISTERED);
  release(other, REGISTERED);
  wait_at(other, COLLECT);
  if (target) {
    domain = first;
    assert(refs((rinzu_async_native_wake_t *)target) == 2);
    row("producer_refs", 0, (intptr_t)target, refs((rinzu_async_native_wake_t *)target));
    target->notify(target);
    assert(atomic_load(&wakes[first]) == 1 && atomic_load(&wakes[other]) == 0);
    target->release(target);
    assert(refs(runtimes[first].native_wake) == 1);
    row("producer_released", 0, 0, 0);
  }
  for (unsigned d = 0; d < 2; ++d) {
    release(d, COLLECT);
    assert(!pthread_join(threads[d], NULL));
  }
  return 0;
}
