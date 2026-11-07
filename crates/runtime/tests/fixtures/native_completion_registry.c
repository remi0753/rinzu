#include "rinzu/source_async.h"
#include <assert.h>
#include <stdio.h>
#include <time.h>
static uint64_t nanos(clockid_t id) {
  struct timespec value;
  assert(!clock_gettime(id, &value));
  return (uint64_t)value.tv_sec * 1000000000 + (uint64_t)value.tv_nsec;
}
int main(void) {
  alarm(20);
  enum { COUNT = 4096, ROUNDS = 2000 };
  rinzu_async_runtime_t runtime;
  rinzu_async_init(&runtime);
  rinzu_async_future_t *held[COUNT];
  uint64_t start = nanos(CLOCK_MONOTONIC);
  for (size_t i = 0; i < COUNT; ++i) {
    rinzu_native_completion_t *native = malloc(sizeof(*native));
    assert(native);
    rinzu_native_completion_init(native);
    atomic_store_explicit(&native->done, 1, memory_order_release);
    held[i] = rinzu_async_new(&runtime, RINZU_ASYNC_COMPLETION, RINZU_TRANSFER_UNIT,
                              (rinzu_source_position_t){0});
    held[i]->native_completion = native;
    rinzu_async_start(held[i]);
    assert(rinzu_async_try_collect(held[i]));
    free(native);
  }
  uint64_t registration = nanos(CLOCK_MONOTONIC) - start;
  rinzu_native_completion_t pending;
  rinzu_native_completion_init(&pending);
  rinzu_async_future_t *wait = rinzu_async_new(&runtime, RINZU_ASYNC_COMPLETION,
                                               RINZU_TRANSFER_UNIT, (rinzu_source_position_t){0});
  wait->native_completion = &pending;
  rinzu_async_start(wait);
  rinzu_async_lock(&runtime);
  size_t registered = 0;
  for (rinzu_async_future_t *p = runtime.native_completions; p; p = p->completion_next)
    ++registered;
  rinzu_async_unlock(&runtime);
  const char *active = getenv("RINZU_NATIVE_COMPLETION_ACTIVE_ONLY");
  assert(registered == (active && !strcmp(active, "0") ? COUNT + 1 : 1));
  uint64_t sum = 0, cpu = nanos(CLOCK_PROCESS_CPUTIME_ID);
  start = nanos(CLOCK_MONOTONIC);
  for (size_t i = 0; i < ROUNDS; ++i)
    sum += rinzu_async_completion_epoch(&runtime);
  uint64_t probe = nanos(CLOCK_MONOTONIC) - start;
  cpu = nanos(CLOCK_PROCESS_CPUTIME_ID) - cpu;
  assert(sum == (uint64_t)COUNT * ROUNDS);
  atomic_store_explicit(&pending.done, 1, memory_order_release);
  while (!wait->settled) {
    uint64_t observed = rinzu_async_completion_epoch(&runtime);
    if (!rinzu_async_try_collect(wait))
      rinzu_async_wait_progress(&runtime, observed);
  }
  rinzu_async_release(wait);
  for (size_t i = COUNT; i > 0; --i)
    rinzu_async_release(held[i - 1]);
  rinzu_async_dispose(&runtime);
  assert(!runtime.records && !runtime.pending_operations && !runtime.in_flight);
  printf("{\"held\":%u,\"registered\":%zu,\"rounds\":%u,\"registrationNs\":%llu,\"probeNs\":%llu,"
         "\"cpuNs\":%llu}\n",
         COUNT, registered, ROUNDS, (unsigned long long)registration, (unsigned long long)probe,
         (unsigned long long)cpu);
}
