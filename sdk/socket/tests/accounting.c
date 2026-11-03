/* Counter lifetimes across rollback, completed-but-unfinished transfers, a
   control close, Owner turnover and reactor restart. TCP pressure is bounds.c. */
#include "../native/src/socket.c"
#include "accounting-check.h"
#include <stdio.h>

static void pause_once(void) {
  struct timespec delay = {0, 1000000};
  nanosleep(&delay, NULL);
}
static void wait_operation(void *token) {
  for (unsigned i = 0; !rinzu_native_completion_is_done(rinzu_socket_completion(token)); ++i) {
    assert(i < 10000);
    pause_once();
  }
  check_accounting();
}
static void finish(void *token) {
  rinzu_socket_ack(token);
  check_accounting();
  rinzu_socket_finish(token);
  check_accounting();
}
static void *turnover(void *unused) {
  (void)unused;
  pthread_mutex_lock(&lock);
  struct Handle *h = reserve_handle();
  assert(h);
  discard_handle(h); /* Admission rollback has not acquired an operation. */
  h = reserve_handle();
  assert(h);
  h->closing = true;
  release_handle(h);
  release_handle(h); /* Repeated cleanup must not decrement a free slot. */
  pthread_mutex_unlock(&lock);
  return NULL;
}
int main(void) {
  for (unsigned cycle = 0; cycle < 3; ++cycle) {
    int pair[2];
    assert(!socketpair(AF_UNIX, SOCK_STREAM, 0, pair));
    pthread_mutex_lock(&lock);
    struct Handle *h = reserve_handle();
    assert(h && !configure(pair[0]));
    h->fd = pair[0];
    void *stream = (void *)h->id;
    pthread_mutex_unlock(&lock);
    void *reading = rinzu_socket_operation(rinzu_socket_read(stream, 1, 0));
    uint8_t byte = 193;
    assert(write(pair[1], &byte, 1) == 1);
    wait_operation(reading);
    assert(!rinzu_socket_status(reading) && rinzu_socket_length(reading) == 1);
    assert(rinzu_socket_read(stream, 1, 0) == RINZU_SOCKET_BUSY);
    rinzu_socket_ack(reading);
    rinzu_socket_cancel(reading); /* Acked results must not enqueue a close. */
    assert(rinzu_socket_read(stream, 1, 0) == RINZU_SOCKET_BUSY);
    check_accounting();

    void *writing = rinzu_socket_operation(rinzu_socket_write(stream, &byte, 1, true, 0));
    wait_operation(writing);
    assert(!rinzu_socket_status(writing));
    assert(rinzu_socket_shutdown(stream) == RINZU_SOCKET_BUSY);
    rinzu_socket_ack(writing);
    assert(rinzu_socket_write(stream, &byte, 1, true, 0) == RINZU_SOCKET_BUSY);
    check_accounting();
    finish(writing);

    void *half = rinzu_socket_operation(rinzu_socket_shutdown(stream));
    wait_operation(half);
    assert(!rinzu_socket_status(half));
    finish(half);
    void *closing = rinzu_socket_operation(rinzu_socket_close(stream));
    wait_operation(closing);
    finish(closing);
    assert(rinzu_socket_live_handles() == 1 && rinzu_socket_live_operations() == 1);
    finish(reading); /* Last reference releases the closed handle and Owner. */
    assert(!rinzu_socket_live_handles() && !rinzu_socket_live_operations() &&
           !rinzu_socket_live_buffers() && !rinzu_socket_thread_count());
    assert(rinzu_socket_read(stream, 1, 0) == RINZU_SOCKET_CLOSED);
    close(pair[1]);
  }
  {
    int pair[2];
    assert(!socketpair(AF_UNIX, SOCK_STREAM, 0, pair));
    pthread_mutex_lock(&lock);
    struct Handle *h = reserve_handle();
    assert(h && !configure(pair[0]));
    h->fd = pair[0];
    void *stream = (void *)h->id;
    pthread_mutex_unlock(&lock);
    void *reading = rinzu_socket_operation(rinzu_socket_read(stream, 1, 0));
    uint8_t byte = 97;
    assert(write(pair[1], &byte, 1) == 1);
    wait_operation(reading);
    assert(!rinzu_socket_status(reading));
    pthread_mutex_lock(&lock);
    assert(!pending_count); /* Ready operations are absent from the pending set. */
    pthread_mutex_unlock(&lock);
    rinzu_socket_cancel(reading);
    for (unsigned i = 0;; ++i) {
      pthread_mutex_lock(&lock);
      bool closed = owned(reading)->handle->fd < 0;
      pthread_mutex_unlock(&lock);
      if (closed)
        break;
      assert(i < 10000);
      pause_once();
    }
    check_accounting();
    assert(rinzu_socket_read(stream, 1, 0) == RINZU_SOCKET_CLOSED);
    rinzu_socket_finish(reading); /* Unreceived result still releases everything. */
    check_accounting();
    assert(!rinzu_socket_live_handles() && !rinzu_socket_live_operations() &&
           !rinzu_socket_live_buffers() && !rinzu_socket_thread_count());
    close(pair[1]);
  }
  for (unsigned i = 0; i < 2 * HANDLES; ++i) {
    pthread_t thread;
    assert(!pthread_create(&thread, NULL, turnover, NULL));
    assert(!pthread_join(thread, NULL));
  }
  check_accounting();
  printf("accounting:ready-retains:ack-retains:ready-cancel:directions:close-retains:rollback:"
         "owners=%d:restart=4;"
         "cleanup=0;owners-bytes=%zu;controls=%lld\n",
         2 * HANDLES, sizeof(owners), (long long)rinzu_socket_stat(RINZU_SOCKET_CONTROL_BYTES));
}
