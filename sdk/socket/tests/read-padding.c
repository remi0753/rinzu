/* Poison real pending read buffers, then verify only the documented prefix.
   No test reads or writes the private initialization watermark. */
#include "../native/src/socket.c"
#include "accounting-check.h"
#include <stdio.h>

static void pause_once(void) {
  struct timespec delay = {0, 1000000};
  nanosleep(&delay, NULL);
}
static void *admitted(int64_t id) {
  assert(id > 0);
  return rinzu_socket_operation(id);
}
static void wait_operation(void *token) {
  for (unsigned count = 0; !rinzu_native_completion_is_done(rinzu_socket_completion(token)); ++count) {
    assert(count < 10000);
    pause_once();
  }
}
static void finish(void *token) {
  rinzu_socket_ack(token);
  rinzu_socket_finish(token);
}
static void close_stream(void *stream) {
  int64_t id = rinzu_socket_close(stream);
  if (id == RINZU_SOCKET_CLOSED)
    return;
  void *token = admitted(id);
  wait_operation(token);
  assert(!rinzu_socket_status(token));
  finish(token);
}
static uint8_t pattern(size_t index) { return (uint8_t)(index * 139); }
static void check_prefix(const uint8_t *bytes, size_t prefix, size_t received) {
  for (size_t index = 0; index < prefix; ++index)
    assert(bytes[index] == (index < received ? pattern(index) : 0));
}
enum Outcome { DATA, EOF_READ, DEADLINE, CANCEL, READY_CANCEL };
static void run_case(enum Outcome outcome, size_t maximum, size_t sent, bool descending) {
  int pair[2];
  assert(!socketpair(AF_UNIX, SOCK_STREAM, 0, pair));
  pthread_mutex_lock(&lock);
  struct Handle *handle = reserve_handle();
  assert(handle && !configure(pair[0]));
  handle->fd = pair[0];
  void *stream = (void *)handle->id;
  pthread_mutex_unlock(&lock);
  void *reading = admitted(rinzu_socket_read(stream, (int64_t)maximum, outcome == DEADLINE ? 1000 : 0));
  pthread_mutex_lock(&lock);
  struct Operation *op = owned(reading);
  assert(!op->ready && op->buffer && op->capacity == CHUNK);
  memset(op->buffer, 165, CHUNK);
  pthread_mutex_unlock(&lock);
  uint8_t input[4096];
  assert(sent <= sizeof(input));
  for (size_t index = 0; index < sent; ++index) input[index] = pattern(index);
  bool data = outcome == DATA || outcome == READY_CANCEL;
  if (data) {
    assert(write(pair[1], input, sent) == (ssize_t)sent);
  } else if (outcome == EOF_READ) {
    assert(!shutdown(pair[1], SHUT_WR));
  } else if (outcome == CANCEL) {
    rinzu_socket_cancel(reading);
  }
  wait_operation(reading);
  int32_t status = rinzu_socket_status(reading);
  assert(status == (outcome == DEADLINE ? RINZU_SOCKET_DEADLINE : outcome == CANCEL ? ECANCELED : 0));
  size_t received = (size_t)rinzu_socket_length(reading);
  assert(data ? received && received <= sent && received <= maximum : !received);
  check_accounting();
  const rinzu_native_byte_array_t *view = rinzu_socket_bytes_view(reading);
  assert(view->length == received);
  check_prefix(view->data, received, received);
  size_t small = received <= 256 ? 256 : 4096;
  const uint8_t *bytes = descending ? rinzu_socket_bytes(reading)
                                  : small == 256 ? rinzu_socket_bytes_256(reading)
                                                 : rinzu_socket_bytes_4096(reading);
  check_prefix(bytes, descending ? CHUNK : small, received);
  uint8_t snapshot[4096];
  memcpy(snapshot, bytes, small);
  if (outcome == READY_CANCEL) {
    rinzu_socket_cancel(reading);
    assert(!rinzu_socket_status(reading) && rinzu_socket_length(reading) == (int64_t)received);
  }
  const uint8_t *large = rinzu_socket_bytes(reading);
  assert(bytes == large && view->data == large && view->length == received &&
         !memcmp(snapshot, bytes, small));
  check_prefix(large, CHUNK, received);
  const uint8_t *again = small == 256 ? rinzu_socket_bytes_256(reading)
                                     : rinzu_socket_bytes_4096(reading);
  assert(again == bytes && !memcmp(snapshot, again, small));
  check_prefix(again, small, received);
  finish(reading);
  close_stream(stream);
  close(pair[1]);
  for (unsigned count = 0; !rinzu_socket_join_idle(); ++count) {
    assert(count < 10000);
    pause_once();
  }
  check_accounting();
  assert(!rinzu_socket_live_handles() && !rinzu_socket_live_operations() &&
         !rinzu_socket_live_buffers() && !rinzu_socket_thread_count() && !buffer_bytes);
}
int main(void) {
  const size_t lengths[] = {1, 17, 255, 256, 257, 1024, 4096};
  for (size_t index = 0; index < sizeof(lengths) / sizeof(*lengths); ++index)
    for (unsigned order = 0; order < 2; ++order) {
      run_case(DATA, CHUNK, lengths[index], order != 0);
      run_case(DATA, 17, lengths[index], order != 0);
    }
  for (unsigned order = 0; order < 2; ++order) {
    run_case(EOF_READ, CHUNK, 0, order != 0);
    run_case(DEADLINE, CHUNK, 0, order != 0);
    run_case(CANCEL, CHUNK, 0, order != 0);
    run_case(READY_CANCEL, CHUNK, 257, order != 0);
  }
  puts("read-padding:36;poison:all;small/full/descending:stable;EOF/deadline/cancel:zero;ready-cancel:stable;cleanup:zero");
}
