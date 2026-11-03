#include <stdlib.h>
#include <stdint.h>
#include <errno.h>
static size_t allocation_budget = SIZE_MAX;
static void *allocate(size_t count, size_t bytes) {
  if (!allocation_budget) {
    errno = ENOMEM;
    return NULL;
  }
  if (allocation_budget != SIZE_MAX)
    allocation_budget--;
  return calloc(count, bytes);
}
#define calloc allocate
#include "rinzu/native_diagnostics.h"
#undef calloc
#include <assert.h>
#include <sched.h>
#include <stdatomic.h>
static pthread_mutex_t external_lock = PTHREAD_MUTEX_INITIALIZER;
static _Atomic int lock_held, native_entered;
static void *external_holder(void *unused) {
  (void)unused;
  assert(!pthread_mutex_lock(&external_lock));
  atomic_store(&lock_held, 1);
  while (!atomic_load(&native_entered))
    sched_yield();
  struct timespec duration = {.tv_sec = 0, .tv_nsec = 120000000L};
  while (nanosleep(&duration, &duration)) {
    assert(errno == EINTR);
  }
  assert(!pthread_mutex_unlock(&external_lock));
  return NULL;
}
static void unobserved_native_lock(rinzu_activity_board_t *board) {
  pthread_t holder;
  assert(!pthread_create(&holder, NULL, external_holder, NULL));
  while (!atomic_load(&lock_held))
    sched_yield();
  rinzu_activity_t *native =
      rinzu_activity_prepare_native(board, "unobserved.native.lock", 17, 31, 49, 41, 1);
  rinzu_activity_start(native);
  atomic_store(&native_entered, 1);
  assert(!pthread_mutex_lock(&external_lock));
  assert(!pthread_mutex_unlock(&external_lock));
  rinzu_activity_end(native);
  assert(!pthread_join(holder, NULL));
  rinzu_activity_stats_t stats = rinzu_activity_stats(board);
  assert(!stats.wait_live && !stats.wait_bytes && stats.warnings[RINZU_ACTIVITY_NATIVE] == 1);
}
static rinzu_wait_dependency_t *begin(rinzu_activity_board_t *board, uint64_t from, uint64_t to,
                                      const char *symbol) {
  return rinzu_activity_wait_begin(board, from, to, symbol, 17, 31, 49, 41, 2);
}
static void *wrong_thread(void *raw) {
  begin(raw, 1, 2, "wrong.thread");
  return NULL;
}
int main(int argc, char **argv) {
  alarm(20);
  assert(argc == 2);
  rinzu_activity_board_t *board = rinzu_activity_new();
  if (!strcmp(argv[1], "oom-wait"))
    allocation_budget = 0;
  if (!strcmp(argv[1], "wrong-thread")) {
    pthread_t thread;
    assert(!pthread_create(&thread, NULL, wrong_thread, board));
    assert(!pthread_join(thread, NULL));
  }
  if (!strcmp(argv[1], "self"))
    begin(board, 1, 1, "self.await");
  if (!strcmp(argv[1], "normal"))
    unobserved_native_lock(board);
  char symbol[] = "first.await";
  rinzu_wait_dependency_t *a = begin(board, 1, 2, symbol);
  memset(symbol, '?', sizeof(symbol) - 1);
  assert(!strcmp(a->symbol, "first.await"));
  rinzu_wait_dependency_t *b = begin(board, 2, 3, "second.await");
  if (!strcmp(argv[1], "cycle")) {
    rinzu_activity_t *native =
        rinzu_activity_prepare_native(board, "native.join", 17, 31, 49, 41, 1);
    rinzu_activity_start(native);
    begin(board, 3, 1, "closing.await");
  }
  if (!strcmp(argv[1], "duplicate"))
    begin(board, 1, 4, "duplicate.await");
  if (!strcmp(argv[1], "pinned-board")) {
    rinzu_activity_dispose(board);
    return 3;
  }
  assert(rinzu_activity_stats(board).wait_live == 2 &&
         rinzu_activity_stats(board).wait_bytes == a->bytes + b->bytes);
  rinzu_activity_wait_end(b);
  b = begin(board, 2, 4, "changed.await"); // Cancellation/resume removed the old dependency.
  rinzu_activity_wait_end(a);
  rinzu_activity_wait_end(b);
  enum { COUNT = 8192 };
  rinzu_wait_dependency_t **chain = calloc(COUNT, sizeof(*chain));
  assert(chain);
  for (size_t n = 0; n < COUNT; n++)
    chain[n] = begin(board, n, n + 1, "chain.await");
  assert(rinzu_activity_stats(board).wait_live == COUNT);
  if (!strcmp(argv[1], "long-cycle"))
    begin(board, COUNT, 0, "chain.closing");
  for (size_t n = COUNT; n > 0; n--)
    rinzu_activity_wait_end(chain[n - 1]);
  free(chain);
  rinzu_activity_stats_t stats = rinzu_activity_stats(board);
  assert(!stats.wait_live && !stats.wait_bytes && !stats.live && !stats.active && !stats.bytes);
  rinzu_activity_dispose(board);
  puts("PASS observed wait dependencies and native metadata baseline");
}
