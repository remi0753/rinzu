/* Isolate the fixed 64KiB native backing pattern used by socket/IO getters.
 * Both variants retain a stable, zero-filled, full-size readable allocation.
 * Build twice with RINZU_BUFFER_MMAP=0/1; this is a trial, not a new SDK API. */
#include <assert.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>

#ifndef RINZU_BUFFER_MMAP
#define RINZU_BUFFER_MMAP 0
#endif
enum { CAPACITY = 65536 };
static __attribute__((noinline)) unsigned char *allocate_buffer(void) {
#if RINZU_BUFFER_MMAP
  void *buffer = mmap(NULL, CAPACITY, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
  assert(buffer != MAP_FAILED);
#else
  void *buffer = calloc(CAPACITY, 1);
  assert(buffer);
#endif
  return buffer;
}
static __attribute__((noinline)) void release_buffer(unsigned char *buffer) {
#if RINZU_BUFFER_MMAP
  assert(!munmap(buffer, CAPACITY));
#else
  free(buffer);
#endif
}
static __attribute__((noinline)) void getter_copy(unsigned char *out, const unsigned char *buffer,
                                                  size_t prefix) {
  memcpy(out, buffer, prefix);
  __asm__ volatile("" : : "r"(out) : "memory");
}
static uint64_t nanos(void) {
  struct timespec now;
  assert(!clock_gettime(CLOCK_MONOTONIC, &now));
  return (uint64_t)now.tv_sec * UINT64_C(1000000000) + (uint64_t)now.tv_nsec;
}
static void verify(void) {
  unsigned char *buffer = allocate_buffer();
  const unsigned char *small = buffer, *medium = buffer, *full = buffer;
  for (size_t i = 0; i < CAPACITY; ++i)
    assert(full[i] == 0);
  memset(buffer, 0xa5, 256);
  assert(small == full && medium == full);
  for (size_t i = 0; i < CAPACITY; ++i)
    assert(full[i] == (i < 256 ? 0xa5 : 0));
  memset(buffer, 0x6b, CAPACITY);
  for (size_t i = 0; i < CAPACITY; ++i)
    assert(full[i] == 0x6b);
  release_buffer(buffer);
}
static void run(size_t prefix) {
  unsigned char out[CAPACITY];
  const size_t rounds = 10000;
  uint64_t checksum = 0;
  for (size_t i = 0; i < 1000; ++i) {
    unsigned char *buffer = allocate_buffer();
    memset(buffer, 0xa5, prefix / 2);
    getter_copy(out, buffer, prefix);
    release_buffer(buffer);
  }
  uint64_t start = nanos();
  for (size_t i = 0; i < rounds; ++i) {
    unsigned char *buffer = allocate_buffer();
    /* Partial native read followed by a prefix getter, including zero padding. */
    memset(buffer, 0xa5, prefix / 2);
    getter_copy(out, buffer, prefix);
    assert(out[0] == 0xa5 && out[prefix - 1] == 0);
    checksum += out[0] + out[prefix - 1];
    release_buffer(buffer);
  }
  uint64_t elapsed = nanos() - start;
  assert(checksum == rounds * UINT64_C(0xa5));
  printf("{\"case\":\"native-backing-prefix-%zu\",\"operations\":%zu,\"nanoseconds\":%" PRIu64
         ",\"allocations\":%zu,\"allocated_bytes\":%zu,\"checksum\":%" PRIu64
         ",\"allocation_kind\":\"%s\"}\n",
         prefix, rounds, elapsed, rounds, rounds * (size_t)CAPACITY, checksum,
         RINZU_BUFFER_MMAP ? "vm-mapping" : "calloc");
}
int main(int argc, char **argv) {
  assert(argc == 2 && !strcmp(argv[1], "native-buffer"));
  verify();
  run(256);
  run(4096);
  run(CAPACITY);
}
