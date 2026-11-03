/* Token lookup checks bypass admission only for the metadata matrix. The last
   case uses a real socketpair/kqueue event with an Int64-high generation. */
#include "../native/src/socket.c"
#include <assert.h>
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
}
static void finish(void *token) {
  rinzu_socket_ack(token);
  rinzu_socket_finish(token);
}
int main(int argc, char **argv) {
  if (argc > 1) {
    if (!strcmp(argv[1], "exhausted")) {
      next_id = ((uintptr_t)INT64_MAX >> TOKEN_SLOT_BITS) + 1;
      (void)identity(0);
    } else if (!strcmp(argv[1], "slot")) {
      (void)identity(TOKEN_SLOT_MASK);
    } else if (!strcmp(argv[1], "operation")) {
      (void)operation((void *)TOKEN_SLOT_MASK);
    } else if (!strcmp(argv[1], "stale")) {
      uintptr_t old = identity(0);
      operations[0].id = identity(0);
      (void)operation((void *)old);
    } else {
      return 2;
    }
    return 3;
  }
  uintptr_t previous = 0;
  for (size_t i = 0; i < HANDLES; ++i) {
    struct Handle *h = &handles[i];
    uintptr_t old = identity(i);
    assert(old > previous && old <= INT64_MAX);
    previous = old;
    h->id = old;
    assert(handle((void *)old) == h);
    h->id = identity(i);
    assert(handle((void *)old) == NULL && handle((void *)h->id) == h);
    assert(lookup_operation(h->id) == NULL);
    previous = h->id;
  }
  for (size_t i = 0; i < OPS; ++i) {
    struct Operation *op = &operations[i];
    uintptr_t old = identity(i);
    assert(old > previous && old <= INT64_MAX);
    previous = old;
    op->id = old;
    assert(operation((void *)old) == op);
    assert(handle((void *)old) == NULL);
    op->id = identity(i);
    assert(lookup_operation(old) == NULL && operation((void *)op->id) == op);
    previous = op->id;
  }
  assert(handle(NULL) == NULL && lookup_operation(0) == NULL);
  assert(handle((void *)(previous & ~TOKEN_SLOT_MASK)) == NULL);
  assert(lookup_operation(previous & ~TOKEN_SLOT_MASK) == NULL);
  uintptr_t outside = (previous & ~TOKEN_SLOT_MASK) | TOKEN_SLOT_MASK;
  assert(handle((void *)outside) == NULL && lookup_operation(outside) == NULL);
  memset(handles, 0, sizeof(handles));
  memset(operations, 0, sizeof(operations));

  next_id = ((uintptr_t)INT64_MAX >> TOKEN_SLOT_BITS) - 8;
  int pair[2];
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
  pthread_mutex_lock(&lock);
  struct Handle *h = reserve_handle();
  assert(h && configure(pair[0]) == 0);
  h->fd = pair[0];
  void *stream = (void *)h->id;
  pthread_mutex_unlock(&lock);
  void *reading = rinzu_socket_operation(rinzu_socket_read(stream, 1, 1000));
  assert((uintptr_t)reading > UINT64_C(0x7fffffffffff0000));
  for (unsigned i = 0;; ++i) {
    pthread_mutex_lock(&lock);
    bool armed = owned(reading)->registered;
    pthread_mutex_unlock(&lock);
    if (armed)
      break;
    assert(i < 10000);
    pause_once();
  }
  uint8_t byte = 171;
  assert(write(pair[1], &byte, 1) == 1);
  wait_operation(reading);
  assert(rinzu_socket_status(reading) == 0 && rinzu_socket_length(reading) == 1 &&
         rinzu_socket_bytes_256(reading)[0] == byte);
  assert(event_count > 0);
  finish(reading);
  void *closing = rinzu_socket_operation(rinzu_socket_close(stream));
  wait_operation(closing);
  assert(rinzu_socket_status(closing) == 0);
  finish(closing);
  close(pair[1]);
  assert(!rinzu_socket_live_handles() && !rinzu_socket_live_operations() &&
         !rinzu_socket_live_buffers() && !rinzu_socket_thread_count());
  puts("lookup:256-handles:768-operations:reuse:bounds:cross-kind:high-kevent:cleanup-zero");
}
