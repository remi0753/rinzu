/* Deterministic bridge bounds using completed native read carriers. Real TCP,
   cancellation and credit-pressure fixtures exercise production I/O separately. */
#include "../native/src/socket.c"
#include "accounting-check.h"
#include <assert.h>
#include <stdio.h>

static uint8_t pattern(size_t index) {
  return (uint8_t)(index * 139);
}
static const struct {
  const char *mode;
  size_t length;
  const uint8_t *(*get)(void *);
} prefixes[] = {
    {"small", 256, rinzu_socket_bytes_256},          {"medium", 4096, rinzu_socket_bytes_4096},
    {"eight", 8192, rinzu_socket_bytes_8192},        {"sixteen", 16384, rinzu_socket_bytes_16384},
    {"thirty-two", 32768, rinzu_socket_bytes_32768}, {"full", CHUNK, rinzu_socket_bytes},
};
static void *completed(size_t length) {
  pthread_mutex_lock(&lock);
  struct Operation *op = reserve(READ, NULL);
  assert(op);
  op->buffer = malloc(CHUNK);
  assert(op->buffer);
  memset(op->buffer, 165, CHUNK); /* Never rely on fresh zero pages or allocator reuse. */
  op->capacity = CHUNK;
  op->length = length;
  for (size_t i = 0; i < CHUNK && i < length; ++i)
    op->buffer[i] = pattern(i);
  ++buffers;
  ++allocations;
  buffer_bytes += CHUNK;
  op->account->bytes += CHUNK;
  if (buffer_bytes > peak_buffer_bytes)
    peak_buffer_bytes = buffer_bytes;
  publish(op, 0);
  void *token = (void *)op->id;
  pthread_mutex_unlock(&lock);
  return token;
}
static void *wrong_owner(void *token) {
  (void)rinzu_socket_bytes_256(token);
  return NULL;
}
int main(int argc, char **argv) {
  if (argc > 1) {
    if (!strcmp(argv[1], "pending")) {
      struct Operation *op = &operations[0];
      op->id = identity(0);
      op->owner = pthread_self();
      op->kind = READ;
      op->buffer = calloc(CHUNK, 1);
      op->capacity = CHUNK;
      (void)rinzu_socket_bytes_256((void *)op->id);
    } else {
      for (size_t i = 0; i < sizeof(prefixes) / sizeof(*prefixes); ++i)
        if (!strcmp(argv[1], prefixes[i].mode)) {
          void *token = completed(prefixes[i].length + 1);
          (void)prefixes[i].get(token);
          return 3;
        }
      void *token = completed(1);
      if (!strcmp(argv[1], "owner")) {
        pthread_t thread;
        assert(!pthread_create(&thread, NULL, wrong_owner, token));
        assert(!pthread_join(thread, NULL));
      } else if (!strcmp(argv[1], "stale")) {
        rinzu_socket_ack(token);
        rinzu_socket_finish(token);
        (void)rinzu_socket_bytes_256(token);
      } else if (!strcmp(argv[1], "kind")) {
        pthread_mutex_lock(&lock);
        owned(token)->kind = WRITE;
        pthread_mutex_unlock(&lock);
        (void)rinzu_socket_bytes_8192(token);
      } else
        return 2;
    }
    return 3;
  }
  const size_t lengths[] = {0,    1,     255,   256,   257,   4095,  4096,  4097,  8191, 8192,
                            8193, 16383, 16384, 16385, 32767, 32768, 32769, 65535, 65536};
  for (size_t n = 0; n < sizeof(lengths) / sizeof(*lengths); ++n) {
    size_t length = lengths[n];
    void *token = completed(length);
    check_accounting();
    assert(rinzu_socket_length(token) == (int64_t)length);
    const uint8_t *pointer = NULL;
    for (size_t pass = 0; pass < 2; ++pass)
      for (size_t index = 0; index < sizeof(prefixes) / sizeof(*prefixes); ++index) {
        size_t selected = pass ? sizeof(prefixes) / sizeof(*prefixes) - 1 - index : index;
        size_t prefix = prefixes[selected].length;
        if (length > prefix)
          continue;
        size_t before = copied_bytes;
        const uint8_t *bytes = prefixes[selected].get(token);
        assert(copied_bytes - before == prefix);
        assert(!pointer || pointer == bytes);
        pointer = bytes;
        for (size_t i = 0; i < prefix; ++i)
          assert(bytes[i] == (i < length ? pattern(i) : 0));
      }
    rinzu_socket_ack(token);
    rinzu_socket_finish(token);
    check_accounting();
    assert(!rinzu_socket_live_handles() && !rinzu_socket_live_operations() &&
           !rinzu_socket_live_buffers() && !rinzu_socket_thread_count() && !buffer_bytes);
  }
  printf("prefix:%zu;binary:all;zero-padding:all;full-getter:compatible;pointers:stable;resources:"
         "zero\n",
         sizeof(lengths) / sizeof(*lengths));
  return 0;
}
