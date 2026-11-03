#include <assert.h>
#include <errno.h>
#include <mach/mach_time.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <time.h>

static _Thread_local unsigned fake_metadata, fake_clock, observations;
static _Thread_local uint32_t numerator = 125, denominator = 3;
static _Thread_local uint64_t ticks, fallback_ns = 37;
static kern_return_t metadata(mach_timebase_info_t info) {
  if (fake_metadata == 1) return KERN_INVALID_ARGUMENT;
  if (!fake_metadata) return mach_timebase_info(info);
  info->numer = numerator;
  info->denom = denominator;
  return KERN_SUCCESS;
}
static uint64_t continuous(void) {
  ++observations;
  return fake_clock ? ticks++ : mach_continuous_time();
}
static int gettime(clockid_t clock, struct timespec *time) {
  if (fake_clock == 2) return -1;
  if (!fake_clock) return clock_gettime(clock, time);
  time->tv_sec = (time_t)(fallback_ns / UINT64_C(1000000000));
  time->tv_nsec = (long)(fallback_ns % UINT64_C(1000000000));
  return 0;
}
#define mach_timebase_info metadata
#define mach_continuous_time continuous
#define clock_gettime gettime
#include "rinzu/native_diagnostics.h"
#undef mach_timebase_info
#undef mach_continuous_time
#undef clock_gettime

static void configure(rinzu_activity_board_t *board, uint32_t n, uint32_t d) {
  numerator = n; denominator = d; fake_metadata = 2;
  rinzu_activity_clock_init(board);
  assert(board->clock_numer == n && board->clock_denom == d);
}
static uint64_t random_word(uint64_t *state) {
  *state ^= *state << 13; *state ^= *state >> 7; *state ^= *state << 17;
  return *state;
}
static void arithmetic(void) {
  const uint32_t denominators[] = {1,3,125,UINT32_MAX};
  uint64_t random = 517;
  for (unsigned d = 0; d < 4; ++d) {
    rinzu_activity_board_t board = {0}; configure(&board,125,denominators[d]);
    for (unsigned kind = 0; kind < RINZU_ACTIVITY_KINDS; ++kind)
      for (unsigned i = 0; i < 40000; ++i) {
        uint64_t start = random_word(&random), now = random_word(&random);
        uint64_t limit = rinzu_activity_threshold(kind);
        if (i % 2) {
          __uint128_t boundary = (__uint128_t)limit * denominators[d] + start;
          if (boundary > UINT64_MAX - 4) continue;
          now = (uint64_t)boundary + (int)(i % 7) - 3;
        }
        uint64_t old_now = now / denominators[d], old_start = start / denominators[d];
        assert(rinzu_activity_sample_ns(&board,now) == old_now);
        assert(rinzu_activity_sample_before(&board,now,start) == (old_now < old_start));
        assert(rinzu_activity_sample_due(&board,now,start,kind) ==
               (old_now >= old_start && old_now - old_start >= limit));
      }
  }
  rinzu_activity_board_t board = {0}; configure(&board,125,3);
  fake_clock = 1; ticks = UINT64_MAX - 8; observations = 0;
  for (unsigned i = 0; i < 8; ++i) {
    uint64_t before = ticks;
    uint64_t actual = rinzu_activity_sample(&board);
    assert(actual == before * UINT64_C(125));
    assert(rinzu_activity_sample_ns(&board,actual) == (before * UINT64_C(125)) / 3);
  }
  assert(observations == 8);
  /* Metadata failure leaves the old observation responsible, at its old site. */
  board = (rinzu_activity_board_t){0}; fake_metadata = 1;
  rinzu_activity_clock_init(&board); assert(!board.clock_denom);
  assert(rinzu_activity_sample(&board) == fallback_ns);
}
static void warnings(void) {
  rinzu_activity_board_t board = {.owner = pthread_self()};
  configure(&board,1,3); fake_clock = 1; ticks = UINT64_C(3000000000);
  rinzu_activity_gate_init(&board);
  rinzu_foreign_sync(pthread_cond_init(&board.changed,NULL));
  for (unsigned kind = 0; kind < RINZU_ACTIVITY_KINDS; ++kind) {
    rinzu_activity_t *activity = rinzu_activity_prepare(&board,kind,"clock.boundary",7,3,9,41,2);
    rinzu_activity_start(activity);
    uint64_t base = activity->started, limit = rinzu_activity_threshold(kind);
    uint64_t first_due = ((base / 3) + limit) * 3;
    rinzu_activity_gate_lock(&board);
    assert(!rinzu_activity_warn_locked(activity,first_due - 1));
    assert(rinzu_activity_warn_locked(activity,first_due));
    char expected[64]; snprintf(expected,sizeof expected,"elapsed_ms=%llu ",(unsigned long long)(limit/1000000));
    assert(strstr(activity->message,expected));
    assert(!rinzu_activity_warn_locked(activity,first_due + 1));
    assert(activity->active && board.stats.warning_pins == 1);
    --board.stats.warning_pins; rinzu_activity_release_locked(activity);
    rinzu_activity_gate_unlock(&board);
    ticks = first_due + 4; rinzu_activity_end(activity);
  }
  /* A later Owner sample suppresses stale watchdog observations exactly. */
  rinzu_activity_t *ready = rinzu_activity_prepare(&board,RINZU_ACTIVITY_OWNER_STALL,"newer.owner",0,0,0,0,0);
  rinzu_activity_start(ready); board.owner_turn = ready->started + UINT64_C(4000000000);
  assert(!rinzu_activity_warn_locked(ready,ready->started+UINT64_C(3000000000)));
  ticks = board.owner_turn; rinzu_activity_end(ready);
  assert(board.stats.prepared == board.stats.ended && !board.stats.live && !board.stats.active);
  rinzu_activity_cache_drain(&board);
  rinzu_foreign_sync(pthread_cond_destroy(&board.changed));
  rinzu_activity_gate_dispose(&board);
}
static void *real_clock(void *unused) {
  (void)unused; fake_metadata = fake_clock = 0;
  rinzu_activity_board_t board = {0}; rinzu_activity_clock_init(&board);
  assert(board.clock_numer && board.clock_denom);
  for (unsigned i = 0; i < 10000; ++i) {
    uint64_t before = rinzu_activity_now();
    int saved = errno; uint64_t now = rinzu_activity_sample(&board); assert(errno == saved);
    uint64_t after = rinzu_activity_now(); now = rinzu_activity_sample_ns(&board,now);
    assert(now >= before && now <= after);
  }
  return NULL;
}
int main(int argc, char **argv) {
  if (argc == 2) {
    rinzu_activity_board_t board = {0}; configure(&board,1,3); fake_clock = 1; ticks = 2;
    if (!strcmp(argv[1],"zero")) (void)rinzu_activity_sample(&board);
    else if (!strcmp(argv[1],"fallback-error")) {
      board = (rinzu_activity_board_t){0}; fake_metadata = 1;
      rinzu_activity_clock_init(&board); assert(!board.clock_denom);
      fake_clock = 2; (void)rinzu_activity_sample(&board);
    } else if (!strcmp(argv[1],"backwards")) {
      rinzu_activity_t activity = {.board=&board,.active=1,.started=1000};
      (void)rinzu_activity_warn_locked(&activity,996);
    } else return 1;
    return 2;
  }
  arithmetic(); warnings();
  pthread_t threads[4];
  for (unsigned i=0;i<4;++i)assert(!pthread_create(&threads[i],NULL,real_clock,NULL));
  for (unsigned i=0;i<4;++i)assert(!pthread_join(threads[i],NULL));
  puts("ACTIVITY-CLOCK:rounding:wrap:fresh:failure:warning:thread:OK");
}
