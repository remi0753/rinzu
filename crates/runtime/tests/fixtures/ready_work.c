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
#include "rinzu/ready_work.h"
#undef calloc
#include <assert.h>
#include <sched.h>
static void wait_ms(unsigned millis) {
  struct timespec duration = {millis / 1000, (long)(millis % 1000) * 1000000L};
  while (nanosleep(&duration, &duration) && errno == EINTR) {
  }
}
static void *publish(void *raw) {
  rinzu_ready_publish(raw);
  return NULL;
}
static void *wrong_receive(void *raw) {
  rinzu_ready_receive(raw);
  return NULL;
}
static void *wrong_turn(void *raw) {
  rinzu_activity_owner_turn(raw);
  return NULL;
}
typedef struct {
  rinzu_ready_work_t **work;
  size_t start, stride, count;
} Producer;
static void *race(void *raw) {
  Producer *producer = raw;
  for (size_t index = producer->start; index < producer->count; index += producer->stride)
    rinzu_ready_publish(producer->work[index]);
  return NULL;
}
static void overdue_owner_turn(void) {
  rinzu_activity_board_t board = {0};
  board.owner = pthread_self();
  rinzu_activity_gate_init(&board);
  rinzu_foreign_sync(pthread_cond_init(&board.changed, NULL));
  rinzu_ready_work_t *a = rinzu_ready_prepare(&board, "late.a", 11, 31, 49, 41, 2);
  rinzu_ready_work_t *b = rinzu_ready_prepare(&board, "late.b", 11, 31, 49, 41, 2);
  rinzu_ready_publish(a);
  rinzu_ready_publish(b);
  rinzu_activity_gate_lock(&board);
  uint64_t old = rinzu_activity_now() - UINT64_C(1000000001);
  a->activity->started = old;
  b->activity->started = old;
  rinzu_activity_gate_unlock(&board);
  rinzu_activity_owner_turn(&board);
  assert(board.stats.warnings[RINZU_ACTIVITY_OWNER_STALL] == 2 && board.stats.warning_pins == 2);
  assert(board.head == b->activity && board.head->next == a->activity &&
         board.pending == b->activity);
  rinzu_ready_receive(a);
  rinzu_ready_receive(b);
  rinzu_ready_dispose(a);
  rinzu_ready_dispose(b);
  assert(!board.head && !board.stats.active && !board.stats.ready_live && board.stats.live == 2);
  rinzu_activity_gate_lock(&board);
  while (board.pending) {
    rinzu_activity_t *warning = board.pending;
    board.pending = warning->pending_next;
    board.stats.warning_pins--;
    rinzu_activity_release_locked(warning);
  }
  board.pending_tail = NULL;
  rinzu_activity_gate_unlock(&board);
  assert(!board.stats.live && !board.stats.bytes);
#ifdef RINZU_ACTIVITY_CACHE_SLOT_BYTES
  rinzu_activity_cache_drain(&board);
#endif
  rinzu_foreign_sync(pthread_cond_destroy(&board.changed));
  rinzu_activity_gate_dispose(&board);
}
static void normal(void) {
  overdue_owner_turn();
  rinzu_activity_board_t *board = rinzu_activity_new();
  uint64_t owner = (uint64_t)(uintptr_t)pthread_self();
  rinzu_ready_work_t *early = rinzu_ready_prepare(board, "already.received", 11, 31, 49, owner, 2);
  rinzu_ready_receive(early);
  pthread_t thread;
  assert(!pthread_create(&thread, NULL, publish, early));
  assert(!pthread_join(thread, NULL));
  assert(!rinzu_activity_stats(board).active);
  rinzu_ready_dispose(early);
  char symbol[] = "physical.packet";
  rinzu_ready_work_t *work = rinzu_ready_prepare(board, symbol, 11, 31, 49, owner, 2);
  memset(symbol, '?', sizeof(symbol) - 1);
  assert(!rinzu_activity_stats(board).active);
  assert(!pthread_create(&thread, NULL, publish, work));
  assert(!pthread_join(thread, NULL));
  assert(rinzu_activity_stats(board).active == 1);
  rinzu_activity_t *native =
      rinzu_activity_prepare(board, RINZU_ACTIVITY_NATIVE, "owner.loop", 11, 31, 49, owner, 1);
  rinzu_activity_start(native);
  /* A queued record can live longer than 1s while Owner continues to turn. */
  for (size_t index = 0; index < 3; index++) {
    wait_ms(550);
    rinzu_activity_owner_turn(board);
    assert(!rinzu_activity_stats(board).warnings[RINZU_ACTIVITY_OWNER_STALL]);
  }
  wait_ms(1150);
  assert(rinzu_activity_stats(board).warnings[RINZU_ACTIVITY_OWNER_STALL] == 1);
  rinzu_ready_receive(work);
  rinzu_ready_dispose(work);
  rinzu_activity_end(native);
  enum { COUNT = 8192, THREADS = 8 };
  rinzu_ready_work_t **records = calloc(COUNT, sizeof(*records));
  assert(records);
  for (size_t index = 0; index < COUNT; index++)
    records[index] = rinzu_ready_prepare(board, "race.packet", 11, 31, 49, owner, 2);
  assert(rinzu_activity_stats(board).ready_live == COUNT);
  Producer producers[THREADS];
  pthread_t threads[THREADS];
  for (size_t index = 0; index < THREADS; index++) {
    producers[index] = (Producer){records, index, THREADS, COUNT};
    assert(!pthread_create(&threads[index], NULL, race, &producers[index]));
  }
  for (size_t index = COUNT; index > 0; index--) {
    rinzu_ready_receive(records[index - 1]);
    if (index % 64 == 0)
      rinzu_activity_owner_turn(board);
  }
  for (size_t index = 0; index < THREADS; index++)
    assert(!pthread_join(threads[index], NULL));
  for (size_t index = 0; index < COUNT; index++)
    rinzu_ready_dispose(records[index]);
  free(records);
  rinzu_activity_stats_t stats = rinzu_activity_stats(board);
  assert(!stats.ready_live && !stats.ready_bytes && !stats.active && stats.prepared == stats.ended);
  assert(stats.warnings[RINZU_ACTIVITY_OWNER_STALL] == 1);
  rinzu_activity_dispose(board);
  puts("PASS physical ready publication/receipt and Owner progress races");
}
int main(int argc, char **argv) {
  alarm(20);
  assert(argc == 2);
  if (!strcmp(argv[1], "normal")) {
    normal();
    return 0;
  }
  rinzu_activity_board_t *board = rinzu_activity_new();
  if (!strcmp(argv[1], "oom-ready"))
    allocation_budget = 0;
  if (!strcmp(argv[1], "oom-interval"))
    allocation_budget = 1;
  rinzu_ready_work_t *work = rinzu_ready_prepare(board, "negative", 1, 2, 3, 4, 5);
  if (!strcmp(argv[1], "wrong-receive") || !strcmp(argv[1], "wrong-turn")) {
    pthread_t thread;
    assert(!pthread_create(&thread, NULL,
                           !strcmp(argv[1], "wrong-turn") ? wrong_turn : wrong_receive,
                           !strcmp(argv[1], "wrong-turn") ? (void *)board : work));
    assert(!pthread_join(thread, NULL));
  }
  if (!strcmp(argv[1], "double-publish")) {
    rinzu_ready_publish(work);
    rinzu_ready_publish(work);
  }
  if (!strcmp(argv[1], "double-receive")) {
    rinzu_ready_receive(work);
    rinzu_ready_receive(work);
  }
  if (!strcmp(argv[1], "unreceived-dispose")) {
    rinzu_ready_dispose(work);
    return 3;
  }
  if (!strcmp(argv[1], "pinned-board-dispose")) {
    rinzu_ready_receive(work);
    rinzu_activity_dispose(board);
  }
  return 3;
}
