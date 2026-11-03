/* Hold an actual SDK syscall interval open while its Owner uses the SDK.
   The pending operation must retain fd/buffer/slot and normal completion. */
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stddef.h>
static void io_stage(void *, int);
#define RINZU_SOCKET_DATA_IO_STAGE(operation, stage) io_stage(operation, stage)
/* These fixtures deliberately exercise the worker's unlocked I/O lease.
   owner-read/write.c separately check inline I/O and real busy fallback. */
#define RINZU_SOCKET_OWNER_WRITE_ALLOWED() false
#define RINZU_SOCKET_OWNER_READ_ALLOWED() false
#include "../native/src/socket.c"
#include <assert.h>
#include <stdio.h>

static pthread_mutex_t probe_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t probe_changed = PTHREAD_COND_INITIALIZER;
static int selected_kind, selected_stage, parked, resume_io;
static void io_stage(void *raw, int stage) {
  struct Operation *op = raw;
  assert(pthread_mutex_lock(&probe_lock) == 0);
  if ((int)op->kind == selected_kind && stage == selected_stage && !parked) {
    parked = 1;
    assert(pthread_cond_broadcast(&probe_changed) == 0);
    while (!resume_io)
      assert(pthread_cond_wait(&probe_changed, &probe_lock) == 0);
  }
  assert(pthread_mutex_unlock(&probe_lock) == 0);
}
static void wait_parked(void) {
  assert(pthread_mutex_lock(&probe_lock) == 0);
  while (!parked)
    assert(pthread_cond_wait(&probe_changed, &probe_lock) == 0);
  assert(pthread_mutex_unlock(&probe_lock) == 0);
}
static void resume(void) {
  assert(pthread_mutex_lock(&probe_lock) == 0);
  resume_io = 1;
  assert(pthread_cond_broadcast(&probe_changed) == 0);
  assert(pthread_mutex_unlock(&probe_lock) == 0);
}
static void wait_ready(void *token) {
  rinzu_native_completion_t *completion = rinzu_socket_completion(token);
  for (unsigned i = 0; !rinzu_native_completion_is_done(completion); ++i) {
    assert(i < 10000);
    struct timespec delay = {0, 1000000};
    nanosleep(&delay, NULL);
  }
}
static void finish_ready(void *token) {
  rinzu_socket_ack(token);
  rinzu_socket_finish(token);
}
static void scenario(int kind, int stage, int action) {
  int pair[2];
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
  pthread_mutex_lock(&lock);
  struct Handle *h = reserve_handle();
  assert(h && configure(pair[0]) == 0);
  h->fd = pair[0];
  uintptr_t handle_id = h->id;
  pthread_mutex_unlock(&lock);
  pthread_mutex_lock(&probe_lock);
  selected_kind = kind;
  selected_stage = stage;
  parked = resume_io = 0;
  pthread_mutex_unlock(&probe_lock);
  uint8_t expected[] = {0, 0x80, 0xff, 'x'};
  if (kind == READ) assert(write(pair[1], expected, sizeof expected) == sizeof expected);
  int64_t id = kind == READ ? rinzu_socket_read((void *)handle_id, sizeof expected, 1000)
                            : rinzu_socket_write((void *)handle_id, expected, sizeof expected, true, 1000);
  assert(id > 0);
  void *token = (void *)(uintptr_t)id;
  wait_parked();
  /* This would deadlock/fail if the SDK still held its global lock. */
  assert(pthread_mutex_trylock(&lock) == 0);
  struct Operation *op = owned(token);
  assert(!op->ready && op->id == (uintptr_t)id && op->buffer && h->refs == 1 &&
         h->id == handle_id && h->fd == pair[0]);
  uint8_t *allocation = op->buffer;
  pthread_mutex_unlock(&lock);
  if (action == 3) {
    rinzu_socket_finish(token);
    _Exit(87);
  }
  assert(rinzu_socket_stat(RINZU_SOCKET_OPERATIONS) == 1);
  assert(!rinzu_socket_join_idle());
  assert((kind == READ ? rinzu_socket_read((void *)handle_id, 1, 0)
                      : rinzu_socket_write((void *)handle_id, expected, 1, true, 0)) == RINZU_SOCKET_BUSY);
  void *closing = NULL;
  if (action == 1) rinzu_socket_cancel(token);
  if (action == 2) {
    int64_t close_id = rinzu_socket_close((void *)handle_id);
    assert(close_id > 0);
    closing = (void *)(uintptr_t)close_id;
  }
  pthread_mutex_lock(&lock);
  assert(op->id == (uintptr_t)id && !op->ready && op->buffer == allocation && h->fd == pair[0]);
  if (action >= 10) {
    switch (action - 10) {
      case 0: ++op->id; break;
      case 1: op->ready = true; break;
      case 2: ++h->id; break;
      case 3: h->fd = -1; break;
      case 4: op->buffer = allocation + 1; break;
      case 5: op->handle = NULL; break;
      default: _Exit(87);
    }
  }
  pthread_mutex_unlock(&lock);
  resume();
  if (action >= 10) {
    struct timespec delay = {2, 0};nanosleep(&delay, NULL);
    _Exit(87);
  }
  wait_ready(token);
  assert(rinzu_socket_status(token) == 0 && rinzu_socket_length(token) == sizeof expected);
  if (kind == READ) assert(!memcmp(rinzu_socket_bytes(token), expected, sizeof expected));
  else {
    uint8_t bytes[4];
    assert(read(pair[1], bytes, sizeof bytes) == sizeof bytes);
    assert(!memcmp(bytes, expected, sizeof bytes));
  }
  if (!action) {
    int64_t close_id = rinzu_socket_close((void *)handle_id);
    assert(close_id > 0);
    closing = (void *)(uintptr_t)close_id;
  }
  if (closing) {wait_ready(closing);finish_ready(closing);}
  finish_ready(token);
  for (unsigned i = 0; !rinzu_socket_join_idle(); ++i) {
    assert(i < 10000);
    struct timespec delay = {0,1000000};nanosleep(&delay,NULL);
  }
  close(pair[1]);
  assert(!rinzu_socket_stat(RINZU_SOCKET_HANDLE_SLOTS) && !rinzu_socket_stat(RINZU_SOCKET_OPERATIONS) &&
         !rinzu_socket_stat(RINZU_SOCKET_BUFFER_BYTES) && !rinzu_socket_stat(RINZU_SOCKET_EVENT_THREADS));
}
int main(int argc, char **argv) {
  if (argc == 2) {
    if (!strcmp(argv[1], "pending-finish")) scenario(READ, 0, 3);
    else scenario(READ, 0, 10 + atoi(argv[1]));
    _Exit(87);
  }
  for (unsigned repeat = 0; repeat < 4; ++repeat)
    for (int kind = READ; kind <= WRITE; ++kind)
      for (int stage = 0; stage < 2; ++stage)
        for (int action = 0; action < 3; ++action)
          scenario(kind, stage, action);
  puts("io-outside:48:pending-lease:metadata-progress:cancel:close:duplex-credit:binary:join:cleanup0");
}
