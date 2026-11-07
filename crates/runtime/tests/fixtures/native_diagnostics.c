#include <stdlib.h>
#include <stdint.h>
#include <errno.h>
#include <time.h>
static int simulate_owner_clock;
static uint64_t owner_clock;
static int simulate_adjusted_clock;
static unsigned simulated_ticks;
static int diagnostic_clock_gettime(clockid_t clock, struct timespec *time) {
  if (simulate_owner_clock) {
    time->tv_sec = (time_t)(owner_clock / 1000000000);
    time->tv_nsec = (long)(owner_clock % 1000000000);
    return 0;
  }
  if (!simulate_adjusted_clock)
    return clock_gettime(clock, time);
  time->tv_sec = 1;
  time->tv_nsec = (long)simulated_ticks++;
#ifdef __APPLE__
  /* Calendar-backed observations can move behind a previously sampled start.
     The raw elapsed clock remains increasing. No watchdog is started here. */
  if (clock == CLOCK_MONOTONIC && simulated_ticks > 1) {
    time->tv_sec = 0;
    time->tv_nsec = 999978000;
  }
#endif
  return 0;
}
#define clock_gettime diagnostic_clock_gettime
static int fail_allocation;
static void *allocate(size_t count, size_t size) {
  if (fail_allocation && !--fail_allocation) {
    errno = ENOMEM;
    return NULL;
  }
  return calloc(count, size);
}
#define calloc allocate
#include "rinzu/native_diagnostics.h"
#undef clock_gettime
#include <assert.h>
#undef calloc
static void wait_ms(unsigned millis) {
  struct timespec duration = {.tv_sec = millis / 1000, .tv_nsec = (long)(millis % 1000) * 1000000L};
  while (nanosleep(&duration, &duration) && errno == EINTR) {
  }
}
static void boundary(void) {
  rinzu_activity_board_t board = {0};
  rinzu_activity_gate_init(&board);
  rinzu_foreign_sync(pthread_cond_init(&board.changed, NULL));
  const uint64_t limits[] = {50000000, 50000000, 100000000, 1000000000, 5000000000};
  for (unsigned kind = 0; kind < RINZU_ACTIVITY_KINDS; kind++) {
    assert(rinzu_activity_threshold(kind) == limits[kind]);
    rinzu_activity_t *activity =
        rinzu_activity_prepare(&board, kind, "exact.threshold", 9, 7, 3, 41, 5);
    rinzu_activity_start(activity);
    rinzu_activity_gate_lock(&board);
    assert(!rinzu_activity_due_locked(&board, activity->started + limits[kind] - 1));
    assert(rinzu_activity_due_locked(&board, activity->started + limits[kind]) == activity);
    assert(activity->warned && board.stats.warning_pins == 1);
    assert(!rinzu_activity_due_locked(&board, activity->started + limits[kind] + 1));
    assert(activity->active && activity->references == 2); // Active owner plus warning pin.
    board.stats.warning_pins--;
    rinzu_activity_release_locked(activity);
    rinzu_activity_gate_unlock(&board);
    rinzu_activity_end(activity);
  }
  assert(board.stats.prepared == 5 && board.stats.ended == 5 && !board.stats.live &&
         !board.stats.bytes);
  rinzu_activity_t *late =
      rinzu_activity_prepare(&board, RINZU_ACTIVITY_NATIVE, "late.watchdog", 0, 0, 0, 42, 1);
  rinzu_activity_start(late);
  wait_ms(60);
  rinzu_activity_end(late);
  assert(board.pending == late && board.pending_tail == late && !board.head && !board.stats.active);
  assert(board.stats.live == 1 && board.stats.warning_pins == 1 &&
         board.stats.warnings[RINZU_ACTIVITY_NATIVE] == 2);
  rinzu_activity_gate_lock(&board);
  board.pending = NULL;
  board.pending_tail = NULL;
  board.stats.warning_pins--;
  rinzu_activity_release_locked(late);
  rinzu_activity_gate_unlock(&board);
#ifdef RINZU_ACTIVITY_CACHE_SLOT_BYTES
  rinzu_activity_cache_drain(&board);
#endif
  rinzu_foreign_sync(pthread_cond_destroy(&board.changed));
  rinzu_activity_gate_dispose(&board);
}
static void normal(void) {
  boundary();
  rinzu_activity_board_t *board = rinzu_activity_new();
  rinzu_activity_t *activities[RINZU_ACTIVITY_KINDS];
  for (unsigned kind = 0; kind < RINZU_ACTIVITY_KINDS; kind++) {
    char symbol[] = "sdk.native-loop";
    activities[kind] = rinzu_activity_prepare(board, kind, symbol, 3, 12, 19, 0x1234, 7);
    memset(symbol, 'x', sizeof(symbol) - 1);
    rinzu_activity_start(activities[kind]);
  }
  /* Main cannot service a dispatcher/managed turn while blocked here. */
  wait_ms(5200);
  rinzu_activity_stats_t stats = rinzu_activity_stats(board);
  for (unsigned kind = 0; kind < RINZU_ACTIVITY_KINDS; kind++)
    assert(stats.warnings[kind] == 1);
  assert(stats.active == 5 && stats.live == 5 && stats.prepared == 5 && !stats.ended);
  for (unsigned kind = 0; kind < RINZU_ACTIVITY_KINDS; kind++)
    rinzu_activity_end(activities[kind]);
  rinzu_activity_t **prepared = calloc(8192, sizeof(*prepared));
  assert(prepared);
  for (size_t index = 0; index < 8192; index++)
    prepared[index] = rinzu_activity_prepare(board, RINZU_ACTIVITY_NATIVE, "abandoned.before.entry",
                                             0, 0, 0, 1, 0);
  assert(rinzu_activity_stats(board).live == 8192);
  for (size_t index = 0; index < 8192; index++)
    rinzu_activity_end(prepared[index]);
  free(prepared);
  stats = rinzu_activity_stats(board);
  assert(stats.prepared == 8197 && stats.ended == 8197 && !stats.active && !stats.live &&
         !stats.bytes && !stats.warning_pins);
  for (unsigned kind = 0; kind < RINZU_ACTIVITY_KINDS; kind++)
    assert(stats.warnings[kind] == 1);
  rinzu_activity_dispose(board);
  puts("PASS native watchdog and exact thresholds");
}
static pthread_mutex_t pin_gate = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t pin_changed = PTHREAD_COND_INITIALIZER;
static int entered, released;
static void before_emit(void *raw) {
  (void)raw;
  rinzu_foreign_lock(&pin_gate);
  entered = 1;
  rinzu_foreign_sync(pthread_cond_broadcast(&pin_changed));
  while (!released)
    rinzu_foreign_sync(pthread_cond_wait(&pin_changed, &pin_gate));
  rinzu_foreign_unlock(&pin_gate);
}
static void pin(void) {
  rinzu_activity_board_t *board = rinzu_activity_new();
  rinzu_activity_test_hook(board, before_emit, NULL);
  rinzu_activity_t *activity =
      rinzu_activity_prepare(board, RINZU_ACTIVITY_MANAGED, "retained.warning", 0, 0, 0, 42, 1);
  rinzu_activity_start(activity);
  rinzu_foreign_lock(&pin_gate);
  while (!entered)
    rinzu_foreign_sync(pthread_cond_wait(&pin_changed, &pin_gate));
  rinzu_foreign_unlock(&pin_gate);
  rinzu_activity_end(activity);
  rinzu_activity_stats_t stats = rinzu_activity_stats(board);
  assert(stats.prepared == stats.ended && !stats.active && stats.live == 1 && stats.bytes &&
         stats.warning_pins == 1);
  rinzu_foreign_lock(&pin_gate);
  released = 1;
  rinzu_foreign_sync(pthread_cond_broadcast(&pin_changed));
  rinzu_foreign_unlock(&pin_gate);
  /* Disposal joins the emitter before freeing its immutable target and record. */
  rinzu_activity_dispose(board);
  puts("PASS native warning pin after interval end");
}
static void *wrong_thread(void *raw) {
  rinzu_activity_start(raw);
  return NULL;
}
static void *shutdown_request(void *raw) {
  for (size_t index = 0; index < 64; index++)
    rinzu_activity_request_shutdown(raw);
  return NULL;
}
static void paired(void) {
  rinzu_activity_board_t *board = rinzu_activity_new();
  rinzu_activity_t *abandoned =
      rinzu_activity_prepare_native(board, "pair.abandoned", 17, 31, 49, 41, 2);
  assert(rinzu_activity_stats(board).prepared == 2 && !rinzu_activity_stats(board).active);
  rinzu_activity_end(abandoned);
  char symbol[] = "pair.blocked";
  rinzu_activity_t *native = rinzu_activity_prepare_native(board, symbol, 17, 31, 49, 41, 2);
  memset(symbol, 'x', sizeof(symbol) - 1);
  rinzu_activity_start(native);
  assert(rinzu_activity_stats(board).active == 1);
  pthread_t request;
  assert(!pthread_create(&request, NULL, shutdown_request, board));
  assert(!pthread_join(request, NULL));
  assert(rinzu_activity_stats(board).active == 2);
  wait_ms(5200);
  rinzu_activity_stats_t stats = rinzu_activity_stats(board);
  assert(stats.active == 2 && stats.prepared == 4 && stats.ended == 2);
  assert(stats.warnings[RINZU_ACTIVITY_NATIVE] == 1 &&
         stats.warnings[RINZU_ACTIVITY_SHUTDOWN] == 1);
  /* Threshold is observation only; physical return ends both intervals. */
  rinzu_activity_end(native);
  rinzu_activity_t *cleanup =
      rinzu_activity_prepare_native(board, "pair.cleanup", 17, 51, 79, 41, 2);
  assert(!rinzu_activity_stats(board).active);
  rinzu_activity_start(cleanup);
  assert(rinzu_activity_stats(board).active == 2);
  rinzu_activity_end(cleanup);
  rinzu_activity_request_shutdown(board);
  stats = rinzu_activity_stats(board);
  assert(!stats.active && stats.prepared == 6 && stats.ended == 6 &&
         stats.warnings[RINZU_ACTIVITY_SHUTDOWN] == 1);
  rinzu_activity_owner_turn(board);
  rinzu_activity_t *next =
      rinzu_activity_prepare_native(board, "pair.next-task", 17, 81, 99, 41, 2);
  rinzu_activity_start(next);
  assert(rinzu_activity_stats(board).active == 1);
  rinzu_activity_end(next);
  stats = rinzu_activity_stats(board);
  assert(stats.prepared == 8 && stats.ended == 8 && stats.warnings[RINZU_ACTIVITY_SHUTDOWN] == 1);
  rinzu_activity_dispose(board);
  puts("PASS paired native blocker waits for physical return");
}
static void owner_turn_boundaries(void) {
  simulate_owner_clock = 1;
  owner_clock = 2000000000;
  rinzu_activity_board_t board = {.owner = pthread_self(), .owner_turn = owner_clock};
  rinzu_activity_gate_init(&board);
  rinzu_foreign_sync(pthread_cond_init(&board.changed, NULL));
  rinzu_activity_t *ready =
      rinzu_activity_prepare(&board, RINZU_ACTIVITY_OWNER_STALL, "turn.boundary", 0, 0, 0, 1, 0);
  rinzu_activity_start(ready);
  owner_clock += rinzu_activity_threshold(RINZU_ACTIVITY_OWNER_STALL) - 1;
  rinzu_activity_owner_turn(&board);
  assert(!ready->warned && !board.pending);
  /* Each progress restarts the deadline, including just below its boundary. */
  owner_clock += rinzu_activity_threshold(RINZU_ACTIVITY_OWNER_STALL) - 1;
  rinzu_activity_owner_turn(&board);
  assert(!ready->warned && !board.pending);
  owner_clock += rinzu_activity_threshold(RINZU_ACTIVITY_OWNER_STALL);
  rinzu_activity_owner_turn(&board);
  assert(ready->warned && board.pending == ready && board.pending_tail == ready);
  assert(board.stats.warning_pins == 1);
  rinzu_activity_t *native =
      rinzu_activity_prepare(&board, RINZU_ACTIVITY_NATIVE, "turn.native", 0, 0, 0, 1, 0);
  rinzu_activity_start(native);
  board.shutdown_requested = 1;
  owner_clock++;
  rinzu_activity_owner_turn(&board);
  assert(board.shutdown_requested && board.pending == ready);
  rinzu_activity_end(native);
  owner_clock++;
  rinzu_activity_owner_turn(&board);
  assert(!board.shutdown_requested && board.pending == ready);
  /* A stale observation must not underflow the interval test. */
  owner_clock--;
  rinzu_activity_owner_turn(&board);
  assert(board.pending == ready && board.stats.warning_pins == 1);
  rinzu_activity_end(ready);
  rinzu_activity_gate_lock(&board);
  board.pending = board.pending_tail = NULL;
  board.stats.warning_pins--;
  rinzu_activity_release_locked(ready);
  rinzu_activity_gate_unlock(&board);
  assert(board.stats.prepared == 2 && board.stats.ended == 2);
  assert(!board.stats.live && !board.stats.active && !board.stats.bytes);
#ifdef RINZU_ACTIVITY_CACHE_SLOT_BYTES
  rinzu_activity_cache_drain(&board);
#endif
  rinzu_foreign_sync(pthread_cond_destroy(&board.changed));
  rinzu_activity_gate_dispose(&board);
}
int main(int argc, char **argv) {
  assert(argc == 2);
  if (!strcmp(argv[1], "owner-turn-boundaries")) {
    owner_turn_boundaries();
    return 0;
  }
  if (!strcmp(argv[1], "adjusted-clock")) {
    simulate_adjusted_clock = 1;
    rinzu_activity_board_t board = {0};
    rinzu_activity_gate_init(&board);
    rinzu_foreign_sync(pthread_cond_init(&board.changed, NULL));
    rinzu_activity_t *activity =
        rinzu_activity_prepare(&board, RINZU_ACTIVITY_NATIVE, "elapsed.clock", 0, 0, 0, 1, 0);
    rinzu_activity_start(activity);
    rinzu_activity_end(activity);
    assert(simulated_ticks >= 2 && board.stats.prepared == board.stats.ended && !board.stats.live &&
           !board.stats.active && !board.stats.warning_pins);
#ifdef RINZU_ACTIVITY_CACHE_SLOT_BYTES
    rinzu_activity_cache_drain(&board);
#endif
    rinzu_foreign_sync(pthread_cond_destroy(&board.changed));
    rinzu_activity_gate_dispose(&board);
    puts("PASS elapsed diagnostics ignore calendar clock adjustment");
    return 0;
  }
  if (!strcmp(argv[1], "recent-progress")) {
    rinzu_activity_board_t board = {0};
    rinzu_activity_t *activity = calloc(1, sizeof(*activity) + sizeof("progress"));
    assert(activity);
    strcpy(activity->symbol, "progress");
    activity->board = &board;
    activity->kind = RINZU_ACTIVITY_OWNER_STALL;
    activity->active = 1;
    activity->started = 100;
    activity->references = 1;
    board.head = activity;
    board.owner_turn = 200;
    assert(!rinzu_activity_warn_locked(activity, 150));
    assert(!activity->warned && !board.stats.warning_pins);
    assert(!rinzu_activity_warn_locked(activity, 201));
    assert(rinzu_activity_warn_locked(activity, 200 + rinzu_activity_threshold(activity->kind)));
    assert(activity->warned && board.stats.warning_pins == 1);
    free(activity);
    puts("PASS newer Owner progress delays a stale watchdog observation");
    return 0;
  }
  if (!strcmp(argv[1], "normal")) {
    normal();
    return 0;
  }
  if (!strcmp(argv[1], "pin")) {
    pin();
    return 0;
  }
  if (!strcmp(argv[1], "paired")) {
    paired();
    return 0;
  }
  if (!strcmp(argv[1], "oom-board")) {
    fail_allocation = 1;
    (void)rinzu_activity_new();
    return 3;
  }
  rinzu_activity_board_t *board = rinzu_activity_new();
  if (!strcmp(argv[1], "oom-pair-first") || !strcmp(argv[1], "oom-pair-second")) {
    fail_allocation = !strcmp(argv[1], "oom-pair-first") ? 1 : 2;
    (void)rinzu_activity_prepare_native(board, "failed.pair", 1, 2, 3, 4, 5);
    abort();
  }
  if (!strcmp(argv[1], "oom-record"))
    fail_allocation = 1;
  rinzu_activity_t *activity =
      rinzu_activity_prepare(board, RINZU_ACTIVITY_NATIVE, "negative", 1, 2, 3, 4, 5);
  if (!strcmp(argv[1], "active-dispose")) {
    rinzu_activity_start(activity);
    rinzu_activity_dispose(board);
    return 3;
  }
  if (!strcmp(argv[1], "double-start")) {
    rinzu_activity_start(activity);
    rinzu_activity_start(activity);
  }
  if (!strcmp(argv[1], "wrong-thread")) {
    pthread_t thread;
    assert(!pthread_create(&thread, NULL, wrong_thread, activity));
    assert(!pthread_join(thread, NULL));
  }
  if (!strcmp(argv[1], "invalid-kind"))
    (void)rinzu_activity_prepare(board, 5, "invalid", 0, 0, 0, 0, 0);
  return 3;
}
