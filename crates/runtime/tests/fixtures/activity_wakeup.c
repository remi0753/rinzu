#include <assert.h>
#include <errno.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

static _Atomic unsigned fake_clock;
static _Atomic uint64_t now_ns = UINT64_C(1000000000);
static _Atomic(pthread_cond_t *) watched_condition;
static _Atomic unsigned signals;
static int fixture_clock(clockid_t clock, struct timespec *value) {
  if (!atomic_load(&fake_clock))
    return clock_gettime(clock, value);
  uint64_t now = atomic_load(&now_ns);
  value->tv_sec = (time_t)(now / UINT64_C(1000000000));
  value->tv_nsec = (long)(now % UINT64_C(1000000000));
  return 0;
}
static int fixture_signal(pthread_cond_t *condition) {
  if (condition == atomic_load(&watched_condition))
    atomic_fetch_add(&signals, 1);
  return pthread_cond_signal(condition);
}
#define RINZU_ACTIVITY_TESTING 1
#define clock_gettime fixture_clock
#define pthread_cond_signal fixture_signal
#include "rinzu/native_diagnostics.h"
#undef pthread_cond_signal
#undef clock_gettime

static void baseline(rinzu_activity_board_t *board) {
  rinzu_activity_stats_t stats = rinzu_activity_stats(board);
  assert(stats.prepared == stats.ended && !stats.live && !stats.active && !stats.bytes &&
         !stats.warning_pins && !stats.ready_live && !stats.ready_bytes && !stats.wait_live &&
         !stats.wait_bytes);
}
static void manual(unsigned coalesced) {
  rinzu_activity_board_t board = {.owner = pthread_self()};
  rinzu_activity_gate_init(&board);
  assert(!pthread_cond_init(&board.changed, NULL));
  atomic_store(&watched_condition, &board.changed);
  atomic_store(&signals, 0);
  rinzu_activity_t *first =
      rinzu_activity_prepare(&board, RINZU_ACTIVITY_MANAGED, "first", 3, 5, 8, 41, 1);
  rinzu_activity_t *second =
      rinzu_activity_prepare(&board, RINZU_ACTIVITY_MANAGED, "second", 3, 9, 12, 41, 1);
  rinzu_activity_start(first);
  assert(atomic_load(&signals) == 1);
  rinzu_activity_start(second);
  assert(atomic_load(&signals) == (coalesced ? 1 : 2));
  rinzu_activity_end(first);
  rinzu_activity_end(second);
  baseline(&board);
  /* A prearmed shutdown activates both the native record and its blocker. */
  board.shutdown_requested = 1;
  rinzu_activity_t *native = rinzu_activity_prepare_native(&board, "paired", 4, 13, 17, 41, 1);
  rinzu_activity_start(native);
  assert(rinzu_activity_stats(&board).active == 2);
  assert(atomic_load(&signals) == (coalesced ? 2 : 3));
  rinzu_activity_t *third =
      rinzu_activity_prepare(&board, RINZU_ACTIVITY_MANAGED, "third", 4, 18, 21, 41, 1);
  rinzu_activity_start(third);
  assert(atomic_load(&signals) == (coalesced ? 2 : 4));
  /* Explicit requests retain their immediate notification even while active. */
  rinzu_activity_request_shutdown(&board);
  assert(atomic_load(&signals) == (coalesced ? 3 : 5));
  rinzu_activity_end(third);
  rinzu_activity_end(native);
  baseline(&board);
  atomic_store(&watched_condition, NULL);
#ifdef RINZU_ACTIVITY_CACHE_SLOT_BYTES
  rinzu_activity_cache_drain(&board);
#endif
  assert(!pthread_cond_destroy(&board.changed));
  rinzu_activity_gate_dispose(&board);
}
typedef struct {
  rinzu_activity_board_t *board;
  _Atomic unsigned seen;
} Observation;
static void emitted(void *raw) {
  Observation *observation = raw;
  rinzu_activity_stats_t stats = rinzu_activity_stats(observation->board);
  unsigned seen = 0;
  for (unsigned kind = 0; kind < RINZU_ACTIVITY_KINDS; ++kind)
    if (stats.warnings[kind])
      seen |= 1u << kind;
  atomic_fetch_or(&observation->seen, seen);
}
static void wait_warning(Observation *observation, unsigned kind) {
  for (unsigned attempt = 0; attempt < 3000; ++attempt) {
    if (atomic_load(&observation->seen) & (1u << kind))
      return;
    struct timespec pause = {.tv_nsec = 1000000};
    while (nanosleep(&pause, &pause) && errno == EINTR) {
    }
  }
  assert(!"watchdog lost a wake or stopped its active timer");
}
static void live_timer(unsigned coalesced) {
  rinzu_activity_board_t *board = rinzu_activity_new();
  Observation observation = {.board = board};
  rinzu_activity_test_hook(board, emitted, &observation);
  atomic_store(&watched_condition, &board->changed);
  atomic_store(&signals, 0);
  rinzu_activity_t *first =
      rinzu_activity_prepare(board, RINZU_ACTIVITY_MANAGED, "already.warned", 7, 11, 19, 41, 2);
  rinzu_activity_start(first);
  atomic_fetch_add(&now_ns, UINT64_C(50000000));
  wait_warning(&observation, RINZU_ACTIVITY_MANAGED);
  /* The remaining active record is already warned. The watcher must still
     retain its timed tick; activation of a second record need not signal. */
  rinzu_activity_t *second =
      rinzu_activity_prepare(board, RINZU_ACTIVITY_NATIVE, "later.deadline", 8, 23, 31, 41, 2);
  rinzu_activity_start(second);
  assert(atomic_load(&signals) == (coalesced ? 1 : 2));
  atomic_fetch_add(&now_ns, UINT64_C(50000000));
  wait_warning(&observation, RINZU_ACTIVITY_NATIVE);
  rinzu_activity_end(second);
  rinzu_activity_end(first);
  rinzu_activity_dispose(board); /* Joins the emitter before observation dies. */
  assert(atomic_load(&signals) == (coalesced ? 2 : 3));
  atomic_store(&watched_condition, NULL);
}
static void *publish(void *raw) {
  rinzu_activity_publish_ready(raw);
  return NULL;
}
static void foreign_publication(void) {
  rinzu_activity_board_t *board = rinzu_activity_new();
  Observation observation = {.board = board};
  rinzu_activity_test_hook(board, emitted, &observation);
  atomic_store(&watched_condition, &board->changed);
  atomic_store(&signals, 0);
  rinzu_activity_t *ready =
      rinzu_activity_prepare(board, RINZU_ACTIVITY_OWNER_STALL, "foreign.ready", 9, 37, 41, 41, 1);
  pthread_t producer;
  assert(!pthread_create(&producer, NULL, publish, ready));
  assert(!pthread_join(producer, NULL));
  assert(atomic_load(&signals) == 1);
  atomic_fetch_add(&now_ns, UINT64_C(1000000000));
  wait_warning(&observation, RINZU_ACTIVITY_OWNER_STALL);
  rinzu_activity_end(ready);
  rinzu_activity_dispose(board);
  assert(atomic_load(&signals) == 2);
  atomic_store(&watched_condition, NULL);
}
int main(int argc, char **argv) {
  assert(argc == 2 && (!strcmp(argv[1], "baseline") || !strcmp(argv[1], "coalesced")));
  unsigned coalesced = !strcmp(argv[1], "coalesced");
  atomic_store(&fake_clock, 1);
  manual(coalesced);
  live_timer(coalesced);
  foreign_publication();
  atomic_store(&fake_clock, 0);
  puts("PASS activation transitions, warned-active timer, shutdown, foreign wake and emitter joins");
}
