#include "api.h"
#include <CoreFoundation/CoreFoundation.h>
#include <assert.h>
#include <errno.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdio.h>
#include <time.h>
#include <unistd.h>
/* One synchronous producer prevents a new foreign arrival while the timing
   handler briefly enters the bounded native clock function. */
enum { THREADS = 1, REPEATS = 512 };
static uint64_t (*handler)(size_t, uint64_t);
static void (*main_ready)(void);
static _Atomic size_t finished;
static _Atomic int active;
static pthread_t threads[THREADS];
static int started, handoff_started;
static uint64_t origin_ns[THREADS], receipt_ns[THREADS];
uint64_t perf_clock(void) {
  struct timespec now;
  assert(!clock_gettime(CLOCK_MONOTONIC, &now));
  return (uint64_t)now.tv_sec * UINT64_C(1000000000) + (uint64_t)now.tv_nsec;
}
int64_t perf_ping(int64_t value) {
  assert(pthread_main_np());
  return value + 1;
}
void perf_report(int32_t kind, uint64_t elapsed, size_t count) {
  assert(pthread_main_np() && count && elapsed);
  if (kind == 6) {
    assert(atomic_load(&active) && atomic_load(&finished) == THREADS && handoff_started);
    fputs("RINZU_OWNER_MANAGED_WAKE=1\n", stderr);
  }
  fprintf(stderr, "RINZU_PERF=%d,%llu,%zu\n", kind, (unsigned long long)elapsed, count);
}
void perf_same(int64_t (*callback)(int64_t), size_t count, int32_t kind) {
  assert(pthread_main_np() && callback && count);
  uint64_t start = perf_clock();
  for (size_t index = 0; index < count; index++)
    assert(callback((int64_t)index) == (int64_t)index + 1);
  perf_report(kind, perf_clock() - start, count);
}
static void *invoke(void *raw) {
  size_t index = (size_t)raw;
  assert(!pthread_main_np());
  for (size_t repeat = 0; repeat < REPEATS; repeat++) {
    uint64_t start = perf_clock(), entered = handler(index * REPEATS + repeat, start),
             received = perf_clock();
    assert(start <= entered && entered <= received);
    origin_ns[index] += entered - start;
    receipt_ns[index] += received - start;
  }
  atomic_fetch_add(&finished, 1);
  return NULL;
}
static void tick(CFRunLoopTimerRef timer, void *context) {
  (void)timer;
  (void)context;
  assert(pthread_main_np());
  if (!started) {
    started = 1;
    for (size_t index = 0; index < THREADS; index++)
      assert(!pthread_create(&threads[index], NULL, invoke, (void *)index));
  }
  if (atomic_load(&finished) == THREADS && !handoff_started) {
    handoff_started = 1;
    main_ready();
  }
}
void perf_stop(void) {
  assert(pthread_main_np() && atomic_load(&active) && atomic_load(&finished) == THREADS);
  CFRunLoopStop(CFRunLoopGetCurrent());
}
int32_t perf_foreign(uint64_t (*callback)(size_t, uint64_t), void (*ready)(void)) {
  assert(pthread_main_np() && callback && ready);
  alarm(20);
  handler = callback;
  main_ready = ready;
  atomic_store(&active, 1);
  CFRunLoopTimerContext context = {0};
  CFRunLoopTimerRef timer =
      CFRunLoopTimerCreate(NULL, CFAbsoluteTimeGetCurrent() + 0.001, 0.001, 0, 0, tick, &context);
  assert(timer);
  CFRunLoopAddTimer(CFRunLoopGetCurrent(), timer, kCFRunLoopDefaultMode);
  (void)CFRunLoopRunInMode(kCFRunLoopDefaultMode, 10, 0);
  atomic_store(&active, 0);
  assert(atomic_load(&finished) == THREADS && handoff_started);
  uint64_t origin = 0, receipt = 0;
  for (size_t index = 0; index < THREADS; index++) {
    assert(!pthread_join(threads[index], NULL));
    origin += origin_ns[index];
    receipt += receipt_ns[index];
  }
  CFRunLoopTimerInvalidate(timer);
  CFRelease(timer);
  alarm(0);
  perf_report(4, origin, THREADS * REPEATS);
  perf_report(5, receipt, THREADS * REPEATS);
  return 512;
}
uint64_t perf_worker_stamp(void) {
  assert(!pthread_main_np());
  struct timespec delay = {0, 10000000};
  while (nanosleep(&delay, &delay))
    assert(errno == EINTR);
  return perf_clock();
}
