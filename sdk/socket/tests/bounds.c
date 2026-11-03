/* White-box credit/cleanup pressure with real nonblocking socketpairs. TCP is transport.c. */
#include "../native/src/socket.c"
#include "accounting-check.h"
#include <assert.h>
#include <stdio.h>
#include <sys/resource.h>

static pthread_mutex_t gate_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t gate_changed = PTHREAD_COND_INITIALIZER;
static unsigned arrivals, phase;
static void *tokens[4][64];
static void barrier(void) {
  pthread_mutex_lock(&gate_lock);
  unsigned observed = phase;
  if (++arrivals == 5) {
    arrivals = 0;
    ++phase;
    pthread_cond_broadcast(&gate_changed);
  } else
    while (phase == observed)
      pthread_cond_wait(&gate_changed, &gate_lock);
  pthread_mutex_unlock(&gate_lock);
}
static void pause_once(void) {
  struct timespec pause = {0, 1000000};
  nanosleep(&pause, NULL);
}
static void wait_operation(void *op) {
  rinzu_native_completion_t *completion = rinzu_socket_completion(op);
  for (unsigned i = 0; !rinzu_native_completion_is_done(completion); ++i) {
    assert(i < 10000);
    pause_once();
  }
}
static void *admitted(int64_t id) {
  assert(id > 0);
  return rinzu_socket_operation(id);
}
static void *pressure(void *raw) {
  size_t owner = (size_t)raw;
  int peers[64];
  void *reads[64], *writes[64], *closes[64];
  uint8_t data[65536] = {0, 255, 128};
  for (size_t i = 0; i < 64; ++i) {
    int pair[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
    pthread_mutex_lock(&lock);
    struct Handle *h = reserve_handle();
    assert(h);
    h->fd = pair[0];
    assert(configure(h->fd) == 0);
    tokens[owner][i] = (void *)h->id;
    pthread_mutex_unlock(&lock);
    peers[i] = pair[1];
  }
  assert(rinzu_socket_listen("127.0.0.1", 0, 16) == RINZU_SOCKET_SATURATED);
  for (size_t i = 0; i < 64; ++i) {
    reads[i] = admitted(rinzu_socket_read(tokens[owner][i], 1, 0));
    writes[i] = admitted(rinzu_socket_write(tokens[owner][i], data, sizeof(data), true, 0));
    assert(rinzu_socket_read(tokens[owner][i], 1, 0) == RINZU_SOCKET_BUSY);
  }
  barrier();
  /* Dedicated control slots remain available with all 512 data credits held. */
  for (size_t i = 0; i < 64; ++i)
    closes[i] = admitted(rinzu_socket_close(tokens[owner][i]));
  barrier();
  barrier();
  for (size_t i = 0; i < 64; ++i) {
    wait_operation(closes[i]);
    assert(rinzu_socket_status(closes[i]) == 0);
    rinzu_socket_ack(closes[i]);
    rinzu_socket_finish(closes[i]);
    wait_operation(reads[i]);
    wait_operation(writes[i]);
    assert(rinzu_socket_status(reads[i]) == ECANCELED);
    assert(rinzu_socket_length(writes[i]) >= 0 && rinzu_socket_length(writes[i]) <= 65536);
    rinzu_socket_ack(reads[i]);
    rinzu_socket_finish(reads[i]);
    rinzu_socket_ack(writes[i]);
    rinzu_socket_finish(writes[i]);
    assert(rinzu_socket_read(tokens[owner][i], 1, 0) == RINZU_SOCKET_CLOSED);
    (void)close(peers[i]);
  }
  while (!rinzu_socket_join_idle())
    pause_once();
  return NULL;
}
int main(void) {
  struct rlimit limit;
  assert(getrlimit(RLIMIT_NOFILE, &limit) == 0);
  if (limit.rlim_cur < 1536) {
    assert(limit.rlim_max >= 1536);
    limit.rlim_cur = 1536;
    assert(setrlimit(RLIMIT_NOFILE, &limit) == 0);
  }
  pthread_t owners[4];
  for (size_t i = 0; i < 4; ++i)
    assert(pthread_create(&owners[i], NULL, pressure, (void *)i) == 0);
  barrier();
  check_accounting();
  assert(rinzu_socket_stat(RINZU_SOCKET_DATA_OPERATIONS) == 512);
  assert(rinzu_socket_stat(RINZU_SOCKET_BUFFER_BYTES) == 32 * 1024 * 1024);
  assert(rinzu_socket_listen("127.0.0.1", 0, 16) == RINZU_SOCKET_SATURATED);
  assert(rinzu_socket_read(tokens[0][0], 1, 0) == RINZU_SOCKET_OWNER);
  barrier();
  check_accounting();
  assert(rinzu_socket_stat(RINZU_SOCKET_CONTROL_OPERATIONS) == 256);
  assert(rinzu_socket_stat(RINZU_SOCKET_OPERATIONS) == 768);
  assert(rinzu_socket_stat(RINZU_SOCKET_BUFFER_BYTES) == 32 * 1024 * 1024);
  barrier();
  for (size_t i = 0; i < 4; ++i)
    assert(pthread_join(owners[i], NULL) == 0);
  check_accounting();
  assert(rinzu_socket_stat(RINZU_SOCKET_OPERATIONS) == 0);
  assert(rinzu_socket_stat(RINZU_SOCKET_HANDLE_SLOTS) == 0);
  assert(rinzu_socket_stat(RINZU_SOCKET_BUFFER_BYTES) == 0);
  assert(rinzu_socket_stat(RINZU_SOCKET_EVENT_THREADS) == 0);
  printf("bounds:owners=4,handles=256,data=512,cleanup=256,peak=%lld,control=%lld;cleanup=0\n",
         (long long)rinzu_socket_stat(RINZU_SOCKET_PEAK_BUFFER_BYTES),
         (long long)rinzu_socket_stat(RINZU_SOCKET_CONTROL_BYTES));
}
