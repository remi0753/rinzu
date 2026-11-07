/* Isolate stack admission/execution/disposal with identical native bodies. */
#include <assert.h>
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include "rinzu/continuation.h"

typedef struct {
  rinzu_continuation_t continuation;
  uint64_t value, *sum;
} Task;
typedef struct {
  uint64_t checksum, maps, reuses;
  size_t peak_bytes;
} Stats;
static void body(void *raw) {
  Task *task = raw;
  *task->sum += task->value;
}
static uint64_t tick(clockid_t clock) {
  struct timespec now;
  assert(!clock_gettime(clock, &now));
  return (uint64_t)now.tv_sec * UINT64_C(1000000000) + (uint64_t)now.tv_nsec;
}
static __attribute__((noinline)) Stats burst(size_t count, int execute) {
  rinzu_continuation_t root;
  rinzu_continuation_init_root(&root);
  rinzu_continuation_accounting_t accounting = {0};
  Task *tasks = calloc(count, sizeof(*tasks));
  assert(tasks);
  uint64_t sum = 0;
  for (size_t i = 0; i < count; ++i) {
    tasks[i].value = i + 1;
    tasks[i].sum = &sum;
    rinzu_continuation_init(&tasks[i].continuation, 256 * 1024, body, &tasks[i], &accounting);
  }
  assert(accounting.live == count);
#ifdef RINZU_EXPECT_DEFERRED_STACK
  assert(!accounting.mapped_bytes && !accounting.allocations);
#endif
  for (size_t i = 0; i < count; ++i) {
    if (execute) {
      rinzu_continuation_resume(&root, &tasks[i].continuation);
      assert(tasks[i].continuation.finished);
    }
    rinzu_continuation_dispose(&tasks[i].continuation);
  }
  assert(sum == (execute ? count * (count + 1) / 2 : 0));
  assert(!accounting.live && accounting.peak_live == count);
  assert(accounting.mapped_bytes == accounting.cached_bytes);
  Stats result = {sum, accounting.allocations, accounting.reuses, accounting.peak_mapped_bytes};
  rinzu_continuation_cache_dispose(&accounting);
  assert(!accounting.mapped_bytes && !accounting.cached_bytes && !accounting.cached);
  free(tasks);
  return result;
}
static void run(size_t count, size_t rounds, int execute, int cache) {
  assert(!setenv("RINZU_CONTINUATION_CACHE_BYTES", cache ? "8388608" : "0", 1));
  (void)burst(8, execute);
  uint64_t checksum = 0, maps = 0, reuses = 0;
  size_t peak = 0;
  uint64_t cpu_start = tick(CLOCK_PROCESS_CPUTIME_ID), start = tick(CLOCK_MONOTONIC);
  for (size_t i = 0; i < rounds; ++i) {
    Stats stats = burst(count, execute);
    checksum += stats.checksum;
    maps += stats.maps;
    reuses += stats.reuses;
    if (stats.peak_bytes > peak)
      peak = stats.peak_bytes;
  }
  uint64_t elapsed = tick(CLOCK_MONOTONIC) - start;
  uint64_t cpu = tick(CLOCK_PROCESS_CPUTIME_ID) - cpu_start;
  printf("{\"case\":\"%s-%zu-cache-%s\",\"operations\":%zu,\"nanoseconds\":%" PRIu64
         ",\"cpu_nanoseconds\":%" PRIu64 ",\"allocations\":%zu,\"allocated_bytes\":%zu"
         ",\"mmap_calls\":%" PRIu64 ",\"stack_reuses\":%" PRIu64
         ",\"peak_mapped_bytes\":%zu,\"checksum\":%" PRIu64 "}\n",
         execute ? "queued" : "unstarted", count, cache ? "on" : "off", count * rounds, elapsed,
         cpu, rounds, count * rounds * sizeof(Task), maps, reuses, peak, checksum);
}
int main(int argc, char **argv) {
  assert(argc == 2);
  if (!strcmp(argv[1], "verify")) {
    for (int cache = 0; cache < 2; ++cache) {
      assert(!setenv("RINZU_CONTINUATION_CACHE_BYTES", cache ? "8388608" : "0", 1));
      for (int execute = 0; execute < 2; ++execute) {
        Stats stats = burst(128, execute);
#ifdef RINZU_EXPECT_DEFERRED_STACK
        assert(stats.maps == (execute ? (cache ? 1 : 128) : 0));
        assert(stats.reuses == (execute && cache ? 127 : 0));
#else
        (void)stats;
#endif
      }
    }
  } else {
    assert(!strcmp(argv[1], "stack"));
    const size_t counts[] = {64, 1024, 8192};
    for (size_t i = 0; i < 3; ++i)
      for (int execute = 0; execute < 2; ++execute)
        for (int cache = 0; cache < 2; ++cache)
          run(counts[i], i == 0 ? 50 : (i == 1 ? 10 : 3), execute, cache);
  }
}
