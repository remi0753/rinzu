#define _POSIX_C_SOURCE 200809L
#include <assert.h>
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include "rinzu/byte_scan.h"

/* Previous AOT algorithm, retained only as the comparison kernel. */
static inline size_t previous(const unsigned char *source, size_t length,
                              const unsigned char *set, size_t count, size_t start, int in_set) {
  uint64_t members[4] = {0};
  for (size_t i = 0; i < count; ++i)
    members[set[i] >> 6] |= UINT64_C(1) << (set[i] & 63);
  for (size_t i = start; i < length; ++i) {
    int member = !!(members[source[i] >> 6] & (UINT64_C(1) << (source[i] & 63)));
    if (member == in_set)
      return i;
  }
  return SIZE_MAX;
}
#if defined(RINZU_PROBE_PAIR_REJECTED)
static inline size_t pair_candidate(const unsigned char *source, size_t length,
                                    const unsigned char *set, size_t count, size_t start, int in_set) {
  if (count == 2) {
    for (size_t i = start; i < length; ++i)
      if ((source[i] == set[0] || source[i] == set[1]) == in_set)
        return i;
    return SIZE_MAX;
  }
  return rinzu_byte_find_set(source, length, set, count, start, in_set);
}
#define scan pair_candidate
#elif defined(RINZU_PROBE_BASELINE)
#define scan previous
#else
#define scan rinzu_byte_find_set
#endif
static void verify(void) {
  unsigned char input[256], set[256];
  for (size_t i = 0; i < 256; ++i) input[i] = set[i] = (unsigned char)i;
  const size_t starts[] = {0, 1, 7, 8, 254, 255, 256, 257, SIZE_MAX};
  for (size_t byte = 0; byte < 256; ++byte) {
    const unsigned char one[] = {(unsigned char)byte};
    const unsigned char pair[] = {(unsigned char)byte, (unsigned char)(255 - byte)};
    const unsigned char duplicate[] = {(unsigned char)byte, (unsigned char)byte};
    const unsigned char *sets[] = {set, one, pair, duplicate, set, set};
    const size_t counts[] = {0, 1, 2, 2, 32, 256};
    for (size_t i = 0; i < 6; ++i)
      for (size_t j = 0; j < sizeof starts / sizeof *starts; ++j)
        for (int in = 0; in < 2; ++in) {
          size_t start = starts[j];
          assert(scan(input, 256, sets[i], counts[i], start, in) ==
                 previous(input, 256, sets[i], counts[i], start, in));
          assert(scan(input, 0, sets[i], counts[i], start, in) == SIZE_MAX);
          assert(scan(one, 1, sets[i], counts[i], start, in) ==
                 previous(one, 1, sets[i], counts[i], start, in));
        }
  }
}
static uint64_t tick(void) {
  struct timespec now;
  assert(!clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &now));
  return (uint64_t)now.tv_sec * 1000000000 + now.tv_nsec;
}
int main(int argc, char **argv) {
  (void)argv; verify();
  if (argc == 1) { puts("byte scan differential: passed"); return 0; }
  const size_t lengths[] = {0, 8, 64, 4096, 65536}, counts[] = {0, 1, 2, 32};
  unsigned char source[65536], members[32];
  for (size_t i = 0; i < 5; ++i)
    for (size_t j = 0; j < 4; ++j)
      for (int in = 0; in < 2; ++in) {
        size_t length = lengths[i], count = counts[j];
        for (size_t k = 0; k < 32; ++k) members[k] = (unsigned char)k;
        members[0] = '\n'; members[1] = '\r';
        memset(source, in ? 'A' : '\n', length);
        if (length) source[length - 1] = in ? '\n' : 'A';
        size_t rounds = length < 4096 ? 100000 : length == 4096 ? 2000 : 500;
        uint64_t checksum = 0, begin = tick();
        for (size_t k = 0; k < rounds; ++k) {
          __asm__ volatile("" : : "r"(source), "r"(members) : "memory");
          size_t found = scan(source, length, members, count, 0, in);
          checksum += found == SIZE_MAX ? length : found;
        }
        uint64_t cpu = tick() - begin;
        printf("{\"case\":\"byte-%zu-set%zu-in%d\",\"operations\":%zu,"
               "\"nanoseconds\":%" PRIu64 ",\"cpu_nanoseconds\":%" PRIu64
               ",\"allocations\":0,\"allocated_bytes\":0,\"checksum\":%" PRIu64 "}\n",
               length, count, in, rounds, cpu, cpu, checksum);
      }
}
