#include "rinzu_socket.h"
#include "rinzu/native_completion.h"
#include <assert.h>
#include <stdatomic.h>
#include <stdio.h>
#include <string.h>
#include <time.h>

static void *admitted(int64_t id) {
  assert(id > 0);
  return rinzu_socket_operation(id);
}
static bool ready(void *op) {
  rinzu_native_completion_t *completion = rinzu_socket_completion(op);
  return rinzu_native_completion_is_done(completion);
}
static void wait(void *op) {
  for (unsigned i = 0; !ready(op); ++i) {
    assert(i < 10000);
    struct timespec delay = {0, 1000000};
    nanosleep(&delay, NULL);
  }
}
static void finish(void *op) {
  rinzu_socket_ack(op);
  rinzu_socket_finish(op);
}
static void *value(void *op) {
  wait(op);
  assert(rinzu_socket_status(op) == 0);
  void *h = rinzu_socket_value(op);
  finish(op);
  return h;
}
static void close_value(void *h) {
  void *op = admitted(rinzu_socket_close(h));
  wait(op);
  assert(rinzu_socket_status(op) == 0);
  finish(op);
}
int main(void) {
  void *listener = value(admitted(rinzu_socket_listen("127.0.0.1", 0, 16)));
  void *deadline = admitted(rinzu_socket_accept(listener, 30));
  wait(deadline);
  assert(rinzu_socket_status(deadline) == -7509 && rinzu_socket_port(listener) > 0);
  finish(deadline);
  void *accepting = admitted(rinzu_socket_accept(listener, 0));
  void *client = value(
      admitted(rinzu_socket_connect("127.0.0.1", (int32_t)rinzu_socket_port(listener), 1000)));
  void *server = value(accepting);
  uint8_t data[65536];
  for (size_t i = 0; i < sizeof(data); ++i)
    data[i] = (uint8_t)i;
  void *writing = admitted(rinzu_socket_write(client, data, sizeof(data), true, 1000));
  size_t received = 0;
  while (received < sizeof(data)) {
    void *reading = admitted(rinzu_socket_read(server, 257, 1000));
    wait(reading);
    assert(rinzu_socket_status(reading) == 0);
    size_t n = (size_t)rinzu_socket_length(reading);
    assert(n && n <= 257);
    assert(memcmp(rinzu_socket_bytes(reading), data + received, n) == 0);
    received += n;
    finish(reading);
  }
  wait(writing);
  assert(rinzu_socket_status(writing) == 0 && rinzu_socket_length(writing) == 65536);
  finish(writing);
  void *half = admitted(rinzu_socket_shutdown(client));
  wait(half);
  assert(rinzu_socket_status(half) == 0);
  finish(half);
  void *eof = admitted(rinzu_socket_read(server, 64, 1000));
  wait(eof);
  assert(rinzu_socket_status(eof) == 0 && rinzu_socket_length(eof) == 0);
  finish(eof);
  void *old = server;
  close_value(server);
  for (unsigned i = 0; i < 100; ++i) {
    void *new_listener = value(admitted(rinzu_socket_listen("127.0.0.1", 0, 16)));
    assert(rinzu_socket_read(old, 64, 0) == RINZU_SOCKET_CLOSED);
    close_value(new_listener);
  }
  close_value(client);
  close_value(listener);
  void *unclaimed = admitted(rinzu_socket_listen("127.0.0.1", 0, 16));
  wait(unclaimed);
  rinzu_socket_cancel(unclaimed);
  rinzu_socket_finish(unclaimed);
  while (!rinzu_socket_join_idle()) {
    struct timespec delay = {0, 1000000};
    nanosleep(&delay, NULL);
  }
  assert(rinzu_socket_live_handles() == 0 && rinzu_socket_live_operations() == 0 &&
         rinzu_socket_live_buffers() == 0 && rinzu_socket_thread_count() == 0);
  assert(rinzu_socket_stat(RINZU_SOCKET_BUFFER_BYTES) == 0);
  assert(rinzu_socket_stat(RINZU_SOCKET_CONTROL_BYTES) > 0);
  printf("native-binary-cleanup:65536;alias-reuse:100;events:%lld;partial:%lld\n",
         (long long)rinzu_socket_event_count(), (long long)rinzu_socket_partial_writes());
}
