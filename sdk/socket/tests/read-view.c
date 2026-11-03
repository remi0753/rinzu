/* Exact result metadata is pinned to the operation, without prefix padding. */
#include "../native/src/socket.c"
#include "accounting-check.h"
#include <stdio.h>
static void *completed(size_t length) {
  pthread_mutex_lock(&lock);
  struct Operation *op = reserve(READ, NULL);
  assert(op);
  op->buffer = malloc(CHUNK); assert(op->buffer);
  memset(op->buffer, 165, CHUNK);
  op->capacity = CHUNK; op->length = length;
  for (size_t i = 0; i < length && i < CHUNK; ++i) op->buffer[i] = (uint8_t)(i * 139);
  ++buffers; ++allocations; buffer_bytes += CHUNK; op->account->bytes += CHUNK;
  publish(op, 0);
  void *token = (void *)op->id;
  pthread_mutex_unlock(&lock); return token;
}
static void *foreign(void *token) { (void)rinzu_socket_bytes_view(token); return NULL; }
int main(int argc, char **argv) {
  if (argc > 1) {
    void *token = completed(!strcmp(argv[1], "length") ? CHUNK + 1 : 1);
    if (!strcmp(argv[1], "owner")) {
      pthread_t thread; assert(!pthread_create(&thread, NULL, foreign, token));
      assert(!pthread_join(thread, NULL));
    } else if (!strcmp(argv[1], "stale")) {
      rinzu_socket_ack(token); rinzu_socket_finish(token);
    } else if (!strcmp(argv[1], "pending")) {
      owned(token)->ready = false;
    } else if (!strcmp(argv[1], "kind")) {
      owned(token)->kind = WRITE;
    }
    (void)rinzu_socket_bytes_view(token); return 3;
  }
  const size_t lengths[] = {0, 1, 255, 256, 257, 4095, 4096, 4097, 8191, 8192,
      8193, 16383, 16384, 16385, 32767, 32768, 32769, 65535, CHUNK};
  for (size_t n = 0; n < sizeof(lengths)/sizeof(*lengths); ++n) {
    size_t length = lengths[n];
    void *token = completed(length); size_t before = copied_bytes;
    const rinzu_native_byte_array_t *view = rinzu_socket_bytes_view(token);
    assert(view->length == length && copied_bytes == before + length);
    for (size_t i = 0; i < length; ++i) assert(view->data[i] == (uint8_t)(i * 139));
    if (length < CHUNK) assert(view->data[length] == 165); /* No padding work. */
    const uint8_t *payload = view->data;
    const uint8_t *full = rinzu_socket_bytes(token);
    assert(payload == full);
    for (size_t i = length; i < CHUNK; ++i) assert(!full[i]);
    assert(view == rinzu_socket_bytes_view(token) && view->data == payload && view->length == length);
    rinzu_socket_ack(token); rinzu_socket_finish(token); check_accounting();
    assert(!buffers && !buffer_bytes && !operation_count && !handle_count);
  }
  puts("read-view:19;exact-copy-count;no-padding;legacy-full-compatible;pointer-stable;cleanup:zero");
}
