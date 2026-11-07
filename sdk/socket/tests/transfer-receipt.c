/* Private full-generation ID receipt path agrees with legacy metadata/getters. */
#include "../native/src/socket.c"
#include "accounting-check.h"
#include <assert.h>
#include <stdio.h>
static int64_t receipt_completed(enum Kind kind, int32_t status, size_t length) {
  pthread_mutex_lock(&lock);
  struct Operation *op = reserve(kind, NULL);
  assert(op);
  op->length = length;
  if (kind == READ) {
    op->buffer = malloc(CHUNK);
    assert(op->buffer);
    op->capacity = CHUNK;
    memset(op->buffer, 165, CHUNK);
    ++buffers;
    ++allocations;
    buffer_bytes += CHUNK;
    op->account->bytes += CHUNK;
  }
  publish(op, status);
  int64_t id = (int64_t)op->id;
  pthread_mutex_unlock(&lock);
  return id;
}
static void *foreign_receipt(void *raw) {
  (void)rinzu_socket_transfer_completion((int64_t)(uintptr_t)raw);
  return NULL;
}
int main(int argc, char **argv) {
  if (argc > 1) {
    int64_t id = receipt_completed(!strcmp(argv[1], "kind") ? HALF : READ, 0, 1);
    if (!strcmp(argv[1], "owner")) {
      pthread_t t;
      assert(!pthread_create(&t, NULL, foreign_receipt, (void *)(uintptr_t)id));
      assert(!pthread_join(t, NULL));
    } else if (!strcmp(argv[1], "stale")) {
      rinzu_socket_ack_id(id);
      rinzu_socket_finish_id(id);
      int64_t newer = receipt_completed(READ, 0, 1);
      assert(newer != id);
    }
    (void)rinzu_socket_transfer_completion(id);
    return 3;
  }
  const int32_t statuses[] = {INT32_MIN, -7509, -1, 0, 1, ECONNRESET, ECANCELED, INT32_MAX};
  const size_t lengths[] = {0, 1, 255, 256, 4096, 65535, 65536};
  unsigned rows = 0;
  for (unsigned kind = 0; kind < 2; ++kind)
    for (unsigned i = 0; i < sizeof(statuses) / sizeof(*statuses); ++i)
      for (unsigned n = 0; n < sizeof(lengths) / sizeof(*lengths); ++n) {
        int64_t id = receipt_completed(kind ? WRITE : READ, statuses[i], lengths[n]);
        void *old = rinzu_socket_operation(id);
        rinzu_native_completion_t *completion = rinzu_socket_transfer_completion(id);
        assert(completion == rinzu_socket_completion(old));
        assert(completion->version == 3 && rinzu_native_completion_valid(completion));
        int64_t copied = rinzu_native_completion_transfer_result(completion);
        assert(copied == rinzu_socket_transfer_result(old));
        int64_t length = (int64_t)((uint64_t)copied & UINT64_C(131071));
        assert(length == (int64_t)lengths[n] && (copied - length) / 131072 == statuses[i]);
        if (!kind) {
          const rinzu_native_byte_array_t *view = rinzu_socket_bytes_view_id(id);
          assert(view == rinzu_socket_bytes_view(old) && view->length == lengths[n]);
        }
        rinzu_socket_ack_id(id);
        assert(owned(old)->ack);
        rinzu_socket_finish_id(id);
        check_accounting();
        assert(!handle_count && !operation_count && !buffer_bytes);
        assert(copied == (int64_t)statuses[i] * 131072 + (int64_t)lengths[n]);
        ++rows;
      }
  puts("transfer-receipt:112;legacy-equivalent;exact-view;signed-prefix;cleanup:zero");
  assert(rows == 112);
}
