/* Metadata bridge matrix. TCP, partial writes and cancellation are covered by
   the transport/boundaries/partial fixtures rather than these synthetic rows. */
#include "../native/src/socket.c"
#include "accounting-check.h"
#include <assert.h>
#include <stdio.h>

static void *completed(enum Kind kind, int32_t status, size_t length) {
  pthread_mutex_lock(&lock);
  struct Operation *op = reserve(kind, NULL);
  assert(op);
  op->length = length;
  publish(op, status);
  void *token = (void *)op->id;
  pthread_mutex_unlock(&lock);
  return token;
}
static void *wrong_owner(void *token) {
  (void)rinzu_socket_transfer_result(token);
  return NULL;
}
int main(int argc, char **argv) {
  if (argc > 1) {
    if (!strcmp(argv[1], "pending")) {
      struct Operation *op = &operations[0];
      op->id = identity(0);
      op->owner = pthread_self();
      op->kind = READ;
      (void)rinzu_socket_transfer_result((void *)op->id);
    } else {
      enum Kind kind = !strcmp(argv[1], "kind") ? HALF : READ;
      size_t length = !strcmp(argv[1], "length") ? CHUNK + 1 : 1;
      void *token = completed(kind, 0, length);
      if (!strcmp(argv[1], "owner")) {
        pthread_t thread;
        assert(!pthread_create(&thread, NULL, wrong_owner, token));
        assert(!pthread_join(thread, NULL));
      } else if (!strcmp(argv[1], "stale")) {
        rinzu_socket_ack(token);
        rinzu_socket_finish(token);
        (void)rinzu_socket_transfer_result(token);
      } else {
        (void)rinzu_socket_transfer_result(token);
      }
    }
    return 3;
  }
  const int32_t statuses[] = {INT32_MIN, RINZU_SOCKET_DEADLINE, -1, 0, 1, ECONNRESET, ECANCELED,
                              INT32_MAX};
  const size_t lengths[] = {0, 1, 255, 256, 4096, 65535, 65536};
  size_t rows = 0;
  for (unsigned kind = 0; kind < 2; ++kind) {
    for (size_t i = 0; i < sizeof(statuses) / sizeof(*statuses); ++i) {
      for (size_t j = 0; j < sizeof(lengths) / sizeof(*lengths); ++j) {
        void *token = completed(kind ? WRITE : READ, statuses[i], lengths[j]);
        check_accounting();
        int64_t packed = rinzu_socket_transfer_result(token);
        int64_t length = (int64_t)((uint64_t)packed & UINT64_C(131071));
        int64_t status = (packed - length) / INT64_C(131072);
        assert(status == rinzu_socket_status(token) && status == statuses[i]);
        assert(length == rinzu_socket_length(token) && length == (int64_t)lengths[j]);
        rinzu_socket_ack(token);
        rinzu_socket_finish(token);
        check_accounting();
        assert(!rinzu_socket_live_operations() && !rinzu_socket_live_buffers() &&
               !rinzu_socket_thread_count());
        ++rows;
      }
    }
  }
  printf("transfer-result:%zu;signed-status:all;partial-prefix:all;cleanup:zero\n", rows);
}
