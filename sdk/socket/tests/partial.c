/* Partial write result, ready-credit retention, duplex and send-closed lane semantics. */
#include "../native/src/socket.c"
#include <assert.h>
#include <stdio.h>
static void *admitted(int64_t id) {
  assert(id > 0);
  return rinzu_socket_operation(id);
}
static void wait_operation(void *token) {
  rinzu_native_completion_t *completion = rinzu_socket_completion(token);
  for (unsigned i = 0; !rinzu_native_completion_is_done(completion); ++i) {
    assert(i < 10000);
    struct timespec delay = {0, 1000000};
    nanosleep(&delay, NULL);
  }
}
static void finish(void *token) {
  rinzu_socket_ack(token);
  rinzu_socket_finish(token);
}
int main(void) {
  int pair[2];
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
  pthread_mutex_lock(&lock);
  struct Handle *h = reserve_handle();
  assert(h && configure(pair[0]) == 0);
  h->fd = pair[0];
  void *stream = (void *)h->id;
  pthread_mutex_unlock(&lock);
  uint8_t data[65536], received[65536];
  for (size_t i = 0; i < sizeof(data); ++i)
    data[i] = (uint8_t)i;
  void *reading = admitted(rinzu_socket_read(stream, 1, 0));
  void *writing = admitted(rinzu_socket_write(stream, data, sizeof(data), false, 1000));
  wait_operation(writing);
  int64_t prefix = rinzu_socket_length(writing);
  assert(rinzu_socket_status(writing) == 0 && prefix > 0 && prefix < 65536);
  assert(rinzu_socket_stat(RINZU_SOCKET_BUFFER_BYTES) == 131072);
  assert(rinzu_socket_write(stream, NULL, 0, false, 0) == RINZU_SOCKET_BUSY);
  size_t count = 0;
  while (count < (size_t)prefix) {
    ssize_t n = read(pair[1], received + count, (size_t)prefix - count);
    assert(n > 0);
    count += (size_t)n;
  }
  assert(memcmp(received, data, count) == 0);
  finish(writing);
  assert(rinzu_socket_stat(RINZU_SOCKET_BUFFER_BYTES) == 65536);
  writing = admitted(rinzu_socket_write(stream, NULL, 0, false, 0));
  wait_operation(writing);
  assert(rinzu_socket_status(writing) == 0 && rinzu_socket_length(writing) == 0);
  finish(writing);
  void *half = admitted(rinzu_socket_shutdown(stream));
  wait_operation(half);
  assert(rinzu_socket_status(half) == 0);
  finish(half);
  assert(rinzu_socket_write(stream, data, 1, false, 0) == RINZU_SOCKET_SEND_CLOSED);
  assert(rinzu_socket_shutdown(stream) == RINZU_SOCKET_SEND_CLOSED);
  uint8_t byte = 255;
  assert(write(pair[1], &byte, 1) == 1);
  wait_operation(reading);
  assert(rinzu_socket_status(reading) == 0 && rinzu_socket_length(reading) == 1 &&
         rinzu_socket_bytes(reading)[0] == byte);
  finish(reading);
  assert(rinzu_socket_read(stream, -1, 0) == RINZU_SOCKET_ARGUMENT);
  assert(rinzu_socket_read(stream, 65537, 0) == RINZU_SOCKET_ARGUMENT);
  assert(rinzu_socket_write(stream, data, 65537, true, 0) == RINZU_SOCKET_SEND_CLOSED);
  void *closing = admitted(rinzu_socket_close(stream));
  wait_operation(closing);
  assert(rinzu_socket_status(closing) == 0);
  finish(closing);
  close(pair[1]);
  assert(rinzu_socket_stat(RINZU_SOCKET_HANDLE_SLOTS) == 0 &&
         rinzu_socket_stat(RINZU_SOCKET_OPERATIONS) == 0 &&
         rinzu_socket_stat(RINZU_SOCKET_BUFFER_BYTES) == 0 &&
         rinzu_socket_stat(RINZU_SOCKET_EVENT_THREADS) == 0);
  printf("partial-prefix:%lld;ready-credit:131072;empty:0;duplex-after-half:255;cleanup=0\n",
         (long long)prefix);
}
