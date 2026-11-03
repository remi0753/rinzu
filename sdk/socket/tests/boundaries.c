/* Deterministic native boundary/race probes; source API processes are separate. */
#include "../native/src/socket.c"
#include "accounting-check.h"
#include <assert.h>
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
  rinzu_native_completion_t *completion = rinzu_socket_completion(token);
  for (unsigned i = 0; !rinzu_native_completion_is_done(completion); ++i) {
    assert(i < 10000);
    pause_once();
  }
}
static void finish(void *token) {
  rinzu_socket_ack(token);
  rinzu_socket_finish(token);
}
static void *value(int64_t id) {
  void *token = admitted(id);
  wait_operation(token);
  assert(rinzu_socket_status(token) == 0);
  void *h = rinzu_socket_value(token);
  finish(token);
  return h;
}
static void close_value(void *token) {
  void *op = admitted(rinzu_socket_close(token));
  wait_operation(op);
  assert(rinzu_socket_status(op) == 0);
  finish(op);
}
static void clean(void) {
  check_accounting();
  for (unsigned i = 0; !rinzu_socket_join_idle(); ++i) {
    assert(i < 10000);
    pause_once();
  }
  assert(rinzu_socket_live_handles() == 0 && rinzu_socket_live_operations() == 0 &&
         rinzu_socket_live_buffers() == 0 && rinzu_socket_thread_count() == 0);
  check_accounting();
}
static void *paired(int *peer) {
  int pair[2];
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
  pthread_mutex_lock(&lock);
  struct Handle *h = reserve_handle();
  assert(h && configure(pair[0]) == 0);
  h->fd = pair[0];
  void *token = (void *)h->id;
  pthread_mutex_unlock(&lock);
  *peer = pair[1];
  return token;
}
static void await_registration(void *token) {
  for (unsigned i = 0;; ++i) {
    pthread_mutex_lock(&lock);
    bool registered = owned(token)->registered;
    assert(!owned(token)->ready);
    pthread_mutex_unlock(&lock);
    if (registered)
      return;
    assert(i < 10000);
    pause_once();
  }
}
static void stale(void) {
  int peer;
  void *h = paired(&peer);
  void *op = admitted(rinzu_socket_read(h, 1, 0));
  await_registration(op);
  pthread_mutex_lock(&lock);
  struct Operation *pending = owned(op);
  struct kevent event;
  uintptr_t old_id = pending->id;
  int old_fd = pending->handle->fd;
  size_t before = stale_events;
  EV_SET(&event, old_fd + 1, EVFILT_READ, 0, 0, 0, op);
  observe_event(&event);
  EV_SET(&event, old_fd, EVFILT_WRITE, 0, 0, 0, op);
  observe_event(&event);
  assert(stale_events == before + 2 && pending->registered && !pending->ready);
  pthread_mutex_unlock(&lock);
  close_value(h);
  wait_operation(op);
  assert(rinzu_socket_status(op) == ECANCELED);
  finish(op);
  close(peer);
  clean();
  h = paired(&peer);
  op = admitted(rinzu_socket_read(h, 1, 0));
  await_registration(op);
  pthread_mutex_lock(&lock);
  pending = owned(op);
  assert(pending->id != old_id);
  /* Simulate an old batch arriving after its operation slot was recycled. */
  EV_SET(&event, pending->handle->fd, EVFILT_READ, 0, 0, 0, (void *)old_id);
  before = stale_events;
  observe_event(&event);
  assert(stale_events == before + 1 && !pending->ready && pending->registered);
  pthread_mutex_unlock(&lock);
  uint8_t byte = 255;
  assert(write(peer, &byte, 1) == 1);
  wait_operation(op);
  assert(rinzu_socket_status(op) == 0 && rinzu_socket_bytes(op)[0] == 255);
  finish(op);
  close_value(h);
  close(peer);
  clean();
}
static void data_deadlines(void) {
  int peer;
  void *h = paired(&peer);
  void *op = admitted(rinzu_socket_read(h, 1, 30));
  wait_operation(op);
  assert(rinzu_socket_status(op) == RINZU_SOCKET_DEADLINE);
  finish(op);
  assert(rinzu_socket_read(h, 1, 0) == RINZU_SOCKET_CLOSED);
  close(peer);
  clean();
  h = paired(&peer);
  uint8_t data[65536] = {0, 128, 255};
  op = admitted(rinzu_socket_write(h, data, sizeof(data), true, 100));
  wait_operation(op);
  assert(rinzu_socket_status(op) == RINZU_SOCKET_DEADLINE);
  assert(rinzu_socket_length(op) > 0 && rinzu_socket_length(op) < 65536);
  printf("write-deadline-prefix:%lld\n", (long long)rinzu_socket_length(op));
  finish(op);
  close(peer);
  clean();
}
static void failures(void) {
  int fd = socket(AF_INET, SOCK_STREAM, 0);
  assert(fd >= 0);
  struct sockaddr_in address = {.sin_len = sizeof(address), .sin_family = AF_INET};
  assert(inet_pton(AF_INET, "127.0.0.1", &address.sin_addr) == 1);
  assert(bind(fd, (struct sockaddr *)&address, sizeof(address)) == 0);
  socklen_t size = sizeof(address);
  assert(getsockname(fd, (struct sockaddr *)&address, &size) == 0);
  /* A bound but non-listening Darwin socket can defer SYN handling. Use a closed port. */
  close(fd);
  void *op = admitted(rinzu_socket_connect("127.0.0.1", ntohs(address.sin_port), 1000));
  wait_operation(op);
  fprintf(stderr, "refused-status:%d port:%u\n", rinzu_socket_status(op), ntohs(address.sin_port));
  assert(rinzu_socket_status(op) == ECONNREFUSED);
  finish(op);
  clean();
  void *listener = value(rinzu_socket_listen("127.0.0.1", 0, 16));
  void *accepting = admitted(rinzu_socket_accept(listener, 0));
  void *client =
      value(rinzu_socket_connect("127.0.0.1", (int32_t)rinzu_socket_port(listener), 1000));
  void *server = value((int64_t)(uintptr_t)accepting);
  pthread_mutex_lock(&lock);
  struct linger reset = {1, 0};
  assert(setsockopt(handle(server)->fd, SOL_SOCKET, SO_LINGER, &reset, sizeof(reset)) == 0);
  pthread_mutex_unlock(&lock);
  close_value(server);
  op = admitted(rinzu_socket_read(client, 64, 1000));
  wait_operation(op);
  assert(rinzu_socket_status(op) == ECONNRESET);
  finish(op);
  assert(rinzu_socket_read(client, 1, 0) == RINZU_SOCKET_CLOSED);
  close_value(listener);
  clean();
}
static void connect_cancellation(void) {
  void *listener = value(rinzu_socket_listen("127.0.0.1", 0, 1));
  void *pending[32];
  for (unsigned i = 0; i < 32; ++i)
    pending[i] =
        admitted(rinzu_socket_connect("127.0.0.1", (int32_t)rinzu_socket_port(listener), 0));
  for (unsigned i = 0; i < 200; ++i)
    pause_once();
  size_t waiting = 0;
  for (unsigned i = 0; i < 32; ++i) {
    rinzu_native_completion_t *completion = rinzu_socket_completion(pending[i]);
    waiting += !rinzu_native_completion_is_done(completion);
    rinzu_socket_cancel(pending[i]);
  }
  assert(waiting > 0);
  for (unsigned i = 0; i < 32; ++i) {
    wait_operation(pending[i]);
    rinzu_socket_finish(pending[i]);
  }
  close_value(listener);
  clean();
  printf("connect-pending-cancel:%zu;stale:%lld;control:%lld;cleanup=0\n", waiting,
         (long long)rinzu_socket_stale_events(),
         (long long)rinzu_socket_stat(RINZU_SOCKET_RETAINED_CONTROL_BYTES));
}
int main(void) {
  stale();
  data_deadlines();
  failures();
  connect_cancellation();
}
