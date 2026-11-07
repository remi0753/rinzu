/* Build with -std=c11 -O2 -pthread -Iinclude; no Cargo/build work in timing. */
#include <assert.h>
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

static volatile int counting;
static uint64_t allocations, allocated_bytes, copied_bytes;
static void *counted_calloc(size_t count, size_t width) {
  void *pointer = calloc(count, width);
  if (counting && pointer) {
    allocations++;
    allocated_bytes += count * width;
  }
  return pointer;
}
static void *counted_memcpy(void *to, const void *from, size_t length) {
  if (counting)
    copied_bytes += length;
  return memcpy(to, from, length);
}
static __attribute__((unused)) void *counted_memmove(void *to, const void *from, size_t length) {
  if (counting)
    copied_bytes += length;
  return memmove(to, from, length);
}
#undef calloc
#undef memcpy
#undef memmove
#define calloc counted_calloc
#define memcpy counted_memcpy
#define memmove counted_memmove
#include "rinzu/error_value.h"
#undef calloc
#undef memcpy
#undef memmove

static uint64_t nanos(void) {
  struct timespec now;
  assert(!clock_gettime(CLOCK_MONOTONIC, &now));
  return (uint64_t)now.tv_sec * UINT64_C(1000000000) + (uint64_t)now.tv_nsec;
}
static __attribute__((noinline)) rinzu_error_buffer_t graph(size_t count,
                                                            const unsigned char *message,
                                                            size_t length) {
  rinzu_error_buffer_t buffer = {0};
  rinzu_error_write(&buffer, count, 4);
  for (uint32_t i = 0; i < count; ++i) {
    unsigned char cleanup[4] = {(unsigned char)(i - 1), 0, 0, 0};
    rinzu_error_node_t node = {
        .code = i,
        .message = message,
        .message_length = length,
        .context = (const unsigned char *)"probe",
        .context_length = 5,
        .source = {42, 3, 9, 1},
        .native_present = 1,
        .native_status = -7,
        .cause = i ? i - 1 : UINT32_MAX,
        .cleanup_count = i ? 1 : 0,
        .cleanup = cleanup,
    };
    rinzu_error_encode_node(&buffer, &node, 0);
  }
  return buffer;
}
static void run(size_t nodes, size_t length, size_t rounds) {
  unsigned char *message = malloc(length);
  assert(message);
  for (size_t i = 0; i < length; ++i)
    message[i] = (unsigned char)('a' + i % 26);
  /* Embedded NUL is data in the Error format. */
  message[length / 2] = 0;
  for (size_t warmup = 0; warmup < 100; ++warmup) {
    rinzu_error_buffer_t buffer = graph(nodes, message, length);
    free(buffer.bytes);
  }
  uint64_t elapsed = 0;
  for (int diagnostic = 0; diagnostic < 2; ++diagnostic) {
    allocations = allocated_bytes = copied_bytes = 0;
    counting = diagnostic;
    uint64_t start = nanos();
    for (size_t i = 0; i < rounds; ++i) {
      rinzu_error_buffer_t buffer = graph(nodes, message, length);
      __asm__ volatile("" : : "r"(buffer.bytes) : "memory");
      free(buffer.bytes);
    }
    if (!diagnostic)
      elapsed = nanos() - start;
    counting = 0;
  }
  rinzu_error_buffer_t buffer = graph(nodes, message, length);
  assert(rinzu_error_value_valid(buffer.bytes, buffer.length));
  assert(!buffer.bytes[buffer.length]);
  rinzu_error_cursor_t cursor = {buffer.bytes, buffer.length, 4};
  for (uint32_t i = 0; i < nodes; ++i) {
    rinzu_error_node_t node;
    assert(!rinzu_error_node_read(&cursor, i, &node));
    assert(node.code == i && node.message_length == length);
    assert(!memcmp(node.message, message, length));
    assert(node.context_length == 5 && !memcmp(node.context, "probe", 5));
    assert(node.cause == (i ? i - 1 : UINT32_MAX));
    assert(node.cleanup_count == (i ? 1 : 0));
    assert(node.source.present && node.source.file == 42 && node.source.start_byte == 3 &&
           node.source.end_byte == 9);
    assert(node.native_present && node.native_status == -7);
  }
  assert(cursor.offset == buffer.length);
  uint64_t checksum = UINT64_C(14695981039346656037);
  for (size_t i = 0; i < buffer.length; ++i)
    checksum = (checksum ^ buffer.bytes[i]) * UINT64_C(1099511628211);
  printf("{\"case\":\"error-%zux%zu\",\"operations\":%zu,\"nanoseconds\":%" PRIu64
         ",\"allocations\":%" PRIu64 ",\"allocated_bytes\":%" PRIu64 ",\"copied_bytes\":%" PRIu64
         ",\"checksum\":%" PRIu64 "}\n",
         nodes, length, rounds, elapsed, allocations, allocated_bytes, copied_bytes, checksum);
  free(buffer.bytes);
  free(message);
}
static void verify(void) {
  rinzu_error_buffer_t buffer = {0};
  rinzu_error_append(&buffer, NULL, 0);
  assert(!buffer.length && buffer.bytes && !buffer.bytes[0]);
  rinzu_error_append(&buffer, (const unsigned char *)"abc", 3);
  rinzu_error_append(&buffer, buffer.bytes, buffer.length);
  assert(buffer.length == 6 && !memcmp(buffer.bytes, "abcabc", 6));
  unsigned char bytes[200];
  for (size_t i = 0; i < sizeof(bytes); ++i)
    bytes[i] = (unsigned char)i;
  rinzu_error_append(&buffer, bytes, sizeof(bytes));
  size_t previous = buffer.length;
  rinzu_error_append(&buffer, buffer.bytes, previous);
  assert(buffer.length == previous * 2 && !memcmp(buffer.bytes, buffer.bytes + previous, previous));
  assert(!buffer.bytes[buffer.length]);
  free(buffer.bytes);
#ifdef RINZU_EXPECT_AMORTIZED_ERROR_BUFFER
  buffer = (rinzu_error_buffer_t){0};
  allocations = copied_bytes = allocated_bytes = 0;
  counting = 1;
  for (size_t i = 0; i < 4096; ++i) {
    unsigned char byte = (unsigned char)i;
    rinzu_error_append(&buffer, &byte, 1);
  }
  counting = 0;
  assert(allocations <= 8 && copied_bytes < 4 * buffer.length);
  assert(buffer.capacity >= buffer.length + 1 && buffer.capacity <= 2 * (buffer.length + 1));
  for (size_t i = 0; i < buffer.length; ++i)
    assert(buffer.bytes[i] == (unsigned char)i);
  assert(!buffer.bytes[buffer.length]);
  free(buffer.bytes);
#endif
}
int main(int argc, char **argv) {
  assert(argc == 2);
  verify();
  if (!strcmp(argv[1], "verify"))
    return 0;
  if (!strcmp(argv[1], "overflow")) {
    rinzu_error_buffer_t buffer = {0};
    buffer.length = SIZE_MAX;
    rinzu_error_append(&buffer, NULL, 0);
    return 99;
  }
  assert(!strcmp(argv[1], "error"));
  run(1, 64, 20000);
  run(64, 64, 1000);
  run(1, 65536, 2000);
  return 0;
}
