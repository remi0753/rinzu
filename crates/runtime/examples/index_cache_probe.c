#include <assert.h>
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

static volatile int counting;
static uint64_t allocations, allocated_bytes;
static void *counted_calloc(size_t count, size_t width) {
  void *pointer = calloc(count, width);
  if (counting && pointer) {
    allocations++;
    allocated_bytes += count * width;
  }
  return pointer;
}
#define calloc counted_calloc
#include "rinzu/managed_data.h"
#undef calloc

static uint64_t nanos(void) {
  struct timespec now;
  assert(!clock_gettime(CLOCK_MONOTONIC, &now));
  return (uint64_t)now.tv_sec * UINT64_C(1000000000) + (uint64_t)now.tv_nsec;
}
static __attribute__((noinline)) void rebuild(rinzu_managed_data_t *data,
    uint64_t identity, size_t capacity) {
  rinzu_managed_index_resize(data, capacity);
  rinzu_managed_value_t *value = rinzu_managed_lookup(data, identity);
  assert(value && value->identity == identity && value->length == 1 && value->bytes[0] == 'x');
}
static __attribute__((noinline)) void cold_domain(void) {
  rinzu_managed_data_t data = {0};
  uint64_t value = rinzu_managed_string(&data, (const unsigned char *)"x", 1);
  assert(rinzu_managed_lookup(&data, value));
  rinzu_managed_data_dispose(&data);
}
static void run(int cache, int cold) {
  assert(!setenv("RINZU_MANAGED_LOOKUP_CACHE", cache ? "1" : "0", 1));
  rinzu_managed_data_t data = {0};
  uint64_t identity = rinzu_managed_string(&data, (const unsigned char *)"x", 1);
  const size_t capacities[] = {32, 64, 128, 256, 512, 1024, 512, 256, 128, 64};
  size_t rounds = 500000;
  for (size_t i = 0; i < 10000; ++i) {
    if (cold) cold_domain();
    else rebuild(&data, identity, capacities[i % 10]);
  }
  uint64_t elapsed = 0;
  for (int diagnostic = 0; diagnostic < 2; ++diagnostic) {
    allocations = allocated_bytes = 0;
    counting = diagnostic;
    uint64_t start = nanos();
    for (size_t i = 0; i < rounds; ++i) {
      if (cold) cold_domain();
      else rebuild(&data, identity, capacities[i % 10]);
    }
    if (!diagnostic) elapsed = nanos() - start;
    counting = 0;
  }
  rinzu_managed_value_t *value = rinzu_managed_lookup(&data, identity);
  uint64_t checksum = value->length + value->bytes[0];
  printf("{\"case\":\"cache-%s-%s\",\"operations\":%zu,\"nanoseconds\":%" PRIu64
      ",\"allocations\":%" PRIu64 ",\"allocated_bytes\":%" PRIu64
      ",\"checksum\":%" PRIu64 "}\n", cold ? "cold-domain" : "resize", cache ? "on" : "off",
      rounds, elapsed, allocations, allocated_bytes, checksum);
  rinzu_managed_data_dispose(&data);
}
int main(int argc, char **argv) {
  assert(argc == 2 && !strcmp(argv[1], "cache"));
  run(0, 0); run(1, 0); run(0, 1); run(1, 1);
}
