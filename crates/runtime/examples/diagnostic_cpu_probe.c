/* Process CPU includes the diagnostic watchdog as well as its Owner. */
#include <assert.h>
#include <inttypes.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

static _Atomic int counting;
static _Atomic uint64_t signals, timed_waits, idle_waits;
static int counted_signal(pthread_cond_t *condition) {
  if (atomic_load_explicit(&counting, memory_order_relaxed))
    atomic_fetch_add_explicit(&signals, 1, memory_order_relaxed);
  return pthread_cond_signal(condition);
}
static int counted_wait(pthread_cond_t *condition, pthread_mutex_t *gate) {
  if (atomic_load_explicit(&counting, memory_order_relaxed))
    atomic_fetch_add_explicit(&idle_waits, 1, memory_order_relaxed);
  return pthread_cond_wait(condition, gate);
}
static int counted_timed_wait(pthread_cond_t *condition, pthread_mutex_t *gate,
                              const struct timespec *duration) {
  if (atomic_load_explicit(&counting, memory_order_relaxed))
    atomic_fetch_add_explicit(&timed_waits, 1, memory_order_relaxed);
  return pthread_cond_timedwait_relative_np(condition, gate, duration);
}
#define pthread_cond_signal counted_signal
#define pthread_cond_wait counted_wait
#define pthread_cond_timedwait_relative_np counted_timed_wait
#include "rinzu/native_diagnostics.h"
#undef pthread_cond_signal
#undef pthread_cond_wait
#undef pthread_cond_timedwait_relative_np

static uint64_t tick(clockid_t clock) {
  struct timespec now;
  assert(!clock_gettime(clock, &now));
  return (uint64_t)now.tv_sec * UINT64_C(1000000000) + (uint64_t)now.tv_nsec;
}
static void pause_ms(long ms) {
  struct timespec duration = {.tv_sec = ms / 1000, .tv_nsec = (ms % 1000) * 1000000L};
  while (nanosleep(&duration, &duration))
    assert(errno == EINTR);
}
static __attribute__((noinline)) void cycle(rinzu_activity_board_t *board, unsigned kind) {
  rinzu_activity_owner_turn(board);
  if (!kind)
    return;
  rinzu_activity_t *activity =
      kind == 2
          ? rinzu_activity_prepare_native(board, "cpu.native", 0, 0, 1, 1, 1)
          : rinzu_activity_prepare(board,
                                   kind == 3 ? RINZU_ACTIVITY_OWNER_STALL : RINZU_ACTIVITY_MANAGED,
                                   "cpu.activity", 0, 0, 1, 1, 1);
  rinzu_activity_start(activity);
  rinzu_activity_end(activity);
}
static void run(unsigned kind) {
  rinzu_activity_board_t *board = rinzu_activity_new();
  for (unsigned i = 0; i < 100; ++i)
    cycle(board, kind);
  uint64_t elapsed = 0, cpu = 0;
  size_t rounds = 100000;
  uint64_t allocations = kind ? rounds * (kind == 2 ? 2 : 1) : 0;
  for (int diagnostic = 0; diagnostic < 2; ++diagnostic) {
    atomic_store(&signals, 0);
    atomic_store(&timed_waits, 0);
    atomic_store(&idle_waits, 0);
    atomic_store(&counting, diagnostic);
    uint64_t cpu_start = tick(CLOCK_PROCESS_CPUTIME_ID), start = tick(CLOCK_MONOTONIC);
    for (size_t i = 0; i < rounds; ++i)
      cycle(board, kind);
    if (!diagnostic) {
      elapsed = tick(CLOCK_MONOTONIC) - start;
      cpu = tick(CLOCK_PROCESS_CPUTIME_ID) - cpu_start;
    }
    atomic_store(&counting, 0);
  }
  rinzu_activity_stats_t stats = rinzu_activity_stats(board);
  assert(!stats.active && stats.prepared == stats.ended);
  uint64_t signal_count = atomic_load(&signals);
  rinzu_activity_dispose(board);
  const char *names[] = {"owner-empty", "managed-cycle", "native-pair", "ready-cycle"};
  printf("{\"case\":\"%s\",\"operations\":%zu,\"nanoseconds\":%" PRIu64
         ",\"cpu_nanoseconds\":%" PRIu64 ",\"allocations\":%" PRIu64 ",\"allocated_bytes\":%" PRIu64
         ",\"signal_calls\":%" PRIu64 ",\"checksum\":%" PRIu64 "}\n",
         names[kind], rounds, elapsed, cpu, allocations,
         allocations *
             (sizeof(rinzu_activity_t) + strlen(kind == 2 ? "cpu.native" : "cpu.activity") + 1),
         signal_count, (uint64_t)rounds);
}
static void verify(void) {
  rinzu_activity_board_t *board = rinzu_activity_new();
  atomic_store(&signals, 0);
  atomic_store(&counting, 1);
  for (unsigned i = 0; i < 100; ++i)
    cycle(board, 0);
#ifdef RINZU_EXPECT_QUIET_DIAGNOSTICS
  assert(!atomic_load(&signals));
#endif
  for (unsigned i = 0; i < 100; ++i)
    cycle(board, 1);
#ifdef RINZU_EXPECT_QUIET_DIAGNOSTICS
  rinzu_activity_stats_t short_stats = rinzu_activity_stats(board);
  if (!short_stats.warnings[RINZU_ACTIVITY_MANAGED])
    assert(atomic_load(&signals) == 100);
#endif
  rinzu_activity_t *ready =
      rinzu_activity_prepare(board, RINZU_ACTIVITY_OWNER_STALL, "overdue", 1, 2, 3, 1, 1);
  rinzu_activity_start(ready);
  rinzu_activity_gate_lock(board);
  uint64_t now = rinzu_activity_now(),
           threshold = rinzu_activity_threshold(RINZU_ACTIVITY_OWNER_STALL);
  assert(now > threshold + 1);
  ready->started = now - threshold - 1;
  board->owner_turn = ready->started;
  rinzu_activity_gate_unlock(board);
  rinzu_activity_owner_turn(board);
  assert(rinzu_activity_stats(board).warnings[RINZU_ACTIVITY_OWNER_STALL] == 1);
  rinzu_activity_end(ready);
  atomic_store(&counting, 0);
  rinzu_activity_dispose(board);
}
static void idle(void) {
  rinzu_activity_board_t *board = rinzu_activity_new();
  pause_ms(20);
  uint64_t start_cpu = tick(CLOCK_PROCESS_CPUTIME_ID), start = tick(CLOCK_MONOTONIC);
  pause_ms(200);
  uint64_t elapsed = tick(CLOCK_MONOTONIC) - start,
           cpu = tick(CLOCK_PROCESS_CPUTIME_ID) - start_cpu;
  atomic_store(&timed_waits, 0);
  atomic_store(&idle_waits, 0);
  atomic_store(&counting, 1);
  pause_ms(200);
  atomic_store(&counting, 0);
  uint64_t waits = atomic_load(&timed_waits);
  rinzu_activity_dispose(board);
  printf("{\"case\":\"idle-200ms\",\"operations\":1,\"nanoseconds\":%" PRIu64
         ",\"cpu_nanoseconds\":%" PRIu64 ",\"allocations\":0,\"allocated_bytes\":0,"
         "\"signal_calls\":0,\"timed_wait_calls\":%" PRIu64 ",\"checksum\":1}\n",
         elapsed, cpu, waits);
}
int main(int argc, char **argv) {
  assert(argc == 2);
  assert(!setenv("RINZU_DIAGNOSTIC_OUTPUT", "/dev/null", 1));
  /* Refer to both wait wrappers even when the old header has no idle wait. */
  (void)counted_wait;
  if (!strcmp(argv[1], "verify"))
    verify();
  else {
    assert(!strcmp(argv[1], "diagnostic"));
    for (unsigned i = 0; i < 4; ++i)
      run(i);
    idle();
  }
}
