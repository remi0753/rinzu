#include "rinzu/supplant.h"
#include <assert.h>
#include <stdio.h>
#include <string.h>

typedef struct {
  rinzu_supplant_t *state;
  uint64_t generation;
  pthread_mutex_t gate;
  pthread_cond_t ready;
  int start, won;
} Race;
static void *cancel_race(void *raw) {
  Race *race = raw;
  assert(!pthread_mutex_lock(&race->gate));
  while (!race->start)
    assert(!pthread_cond_wait(&race->ready, &race->gate));
  assert(!pthread_mutex_unlock(&race->gate));
  race->won = rinzu_supplant_cancel(race->state, race->generation, RINZU_SUPPLANT_CANCELLED);
  assert(!rinzu_supplant_cancel(race->state, race->generation, RINZU_SUPPLANT_SHUTDOWN));
  return NULL;
}
static void *wrong_owner(void *raw) {
  rinzu_supplant_t *state = raw;
  uint64_t generation = 0;
  (void)rinzu_supplant_reserve(state, &generation);
  return NULL;
}
static void busy(rinzu_supplant_t *state) {
  uint64_t untouched = UINT64_MAX;
  assert(rinzu_supplant_reserve(state, &untouched) == 1 && untouched == UINT64_MAX);
}
int main(int argc, char **argv) {
  rinzu_supplant_t state;
  rinzu_supplant_init(&state);
  uint64_t generation = 0;
  if (argc > 1) {
    const char *mode = argv[1];
    if (!strcmp(mode, "owner")) {
      pthread_t thread;
      assert(!pthread_create(&thread, NULL, wrong_owner, &state));
      assert(!pthread_join(thread, NULL));
      return 2;
    }
    if (!strcmp(mode, "overflow"))
      state.next_generation = UINT64_MAX;
    assert(!rinzu_supplant_reserve(&state, &generation));
    if (!strcmp(mode, "early-entry"))
      rinzu_supplant_begin_entry(&state, generation);
    else if (!strcmp(mode, "early-return"))
      rinzu_supplant_returned(&state, generation);
    else if (!strcmp(mode, "early-release"))
      rinzu_supplant_finish_settlement(&state, generation);
    else if (!strcmp(mode, "dispose"))
      rinzu_supplant_dispose(&state);
    else if (!strcmp(mode, "stale"))
      rinzu_supplant_queue(&state, generation + 1);
    else if (!strcmp(mode, "double-entry")) {
      assert(rinzu_supplant_queue(&state, generation));
      assert(rinzu_supplant_begin_entry(&state, generation));
      rinzu_supplant_entered(&state, generation);
      rinzu_supplant_entered(&state, generation);
    } else if (!strcmp(mode, "double-return")) {
      assert(rinzu_supplant_queue(&state, generation));
      assert(rinzu_supplant_begin_entry(&state, generation));
      rinzu_supplant_entered(&state, generation);
      rinzu_supplant_returned(&state, generation);
      rinzu_supplant_returned(&state, generation);
    } else if (!strcmp(mode, "cancel-release")) {
      assert(rinzu_supplant_queue(&state, generation));
      assert(rinzu_supplant_begin_entry(&state, generation));
      rinzu_supplant_entered(&state, generation);
      assert(rinzu_supplant_cancel(&state, generation, RINZU_SUPPLANT_CANCELLED));
      rinzu_supplant_finish_settlement(&state, generation);
    } else if (!strcmp(mode, "bad-cancel"))
      rinzu_supplant_cancel(&state, generation, RINZU_SUPPLANT_RETURNED);
    return 2;
  }
  for (unsigned queued = 0; queued < 2; queued++)
    for (unsigned reason = RINZU_SUPPLANT_CANCELLED; reason <= RINZU_SUPPLANT_SHUTDOWN; reason++) {
      assert(!rinzu_supplant_reserve(&state, &generation));
      busy(&state);
      if (queued)
        assert(rinzu_supplant_queue(&state, generation));
      assert(rinzu_supplant_cancel(&state, generation, reason));
      busy(&state);
      assert(!rinzu_supplant_queue(&state, generation));
      assert(!rinzu_supplant_begin_entry(&state, generation));
      assert(!rinzu_supplant_snapshot(&state).entered);
      assert(rinzu_supplant_finish_settlement(&state, generation) == reason);
    }
  for (unsigned queued = 0; queued < 2; queued++) {
    assert(!rinzu_supplant_reserve(&state, &generation));
    if (queued)
      assert(rinzu_supplant_queue(&state, generation));
    assert(rinzu_supplant_fail_preparation(&state, generation));
    assert(!rinzu_supplant_cancel(&state, generation, RINZU_SUPPLANT_CANCELLED));
    assert(!rinzu_supplant_begin_entry(&state, generation));
    assert(!rinzu_supplant_snapshot(&state).entered);
    busy(&state);
    assert(rinzu_supplant_finish_settlement(&state, generation) == RINZU_SUPPLANT_FAILED);
  }
  size_t entered = 0, returned = 0;
  uint64_t previous = generation;
  for (unsigned round = 0; round < 1024; round++) {
    assert(!rinzu_supplant_reserve(&state, &generation));
    assert(generation > previous);
    assert(!rinzu_supplant_cancel(&state, previous, RINZU_SUPPLANT_CANCELLED));
    previous = generation;
    busy(&state);
    assert(rinzu_supplant_queue(&state, generation));
    busy(&state);
    if (!(round % 2)) {
      assert(rinzu_supplant_begin_entry(&state, generation));
      busy(&state);
      rinzu_supplant_entered(&state, generation);
      entered++;
    }
    Race race = {.state = &state, .generation = generation};
    assert(!pthread_mutex_init(&race.gate, NULL));
    assert(!pthread_cond_init(&race.ready, NULL));
    pthread_t thread;
    assert(!pthread_create(&thread, NULL, cancel_race, &race));
    assert(!pthread_mutex_lock(&race.gate));
    race.start = 1;
    assert(!pthread_cond_signal(&race.ready));
    assert(!pthread_mutex_unlock(&race.gate));
    int started = !(round % 2) || rinzu_supplant_begin_entry(&state, generation);
    if (started) {
      if (round % 2) {
        rinzu_supplant_entered(&state, generation);
        entered++;
      }
      rinzu_supplant_returned(&state, generation);
      returned++;
    }
    assert(!pthread_join(thread, NULL));
    assert(started || race.won);
    busy(&state);
    assert(rinzu_supplant_finish_settlement(&state, generation) ==
           (race.won ? RINZU_SUPPLANT_CANCELLED : RINZU_SUPPLANT_RETURNED));
    assert(!rinzu_supplant_cancel(&state, generation, RINZU_SUPPLANT_CANCELLED));
    assert(rinzu_supplant_snapshot(&state).phase == RINZU_SUPPLANT_IDLE);
    assert(!pthread_cond_destroy(&race.ready));
    assert(!pthread_mutex_destroy(&race.gate));
  }
  assert(entered == returned && entered >= 512);
  rinzu_supplant_dispose(&state);
  puts("PASS supplant 1024 entry/cancel/return races, origin settlement and generation isolation");
  return 0;
}
