#include "api.h"
#include "rinzu/native_completion.h"
#include <assert.h>
#include <errno.h>
#include <pthread.h>
#include <sched.h>
#include <stdatomic.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>
static pthread_t owner;
static int initialized, active, prefix_stage, busy_count, phases[2], runs;
static const void *callbacks[2];
static const char *results[2] = {"first native result 日本語", "second native result 🧭"};
static _Atomic int first_worker_entered, first_children_released;
static rinzu_native_completion_t wrapper_ready[2], child_done[2];
static void same_owner(void) {
  assert(initialized && pthread_equal(owner, pthread_self()));
}
void supplant_reset(int32_t process_main) {
  owner = pthread_self();
  initialized = 1;
  active = prefix_stage = busy_count = runs = 0;
  phases[0] = phases[1] = 0;
  callbacks[0] = callbacks[1] = NULL;
  for (int index = 0; index < 2; ++index) {
    rinzu_native_completion_init(&wrapper_ready[index]);
    rinzu_native_completion_init(&child_done[index]);
  }
  atomic_store(&first_worker_entered, 0);
  atomic_store(&first_children_released, 0);
  if (process_main)
    assert(pthread_main_np());
}
void supplant_prefix(int32_t stage) {
  same_owner();
  assert(stage == prefix_stage + 1 && !active);
  if (stage <= 2)
    assert(!runs && !phases[0] && !phases[1]);
  else if (stage == 3 || stage == 4)
    assert(runs == 1 && phases[0] >= 3 && phases[0] <= 4 && !phases[1] &&
           atomic_load(&first_worker_entered) && !atomic_load(&first_children_released));
  /* Another ready child's I/O continuation may progress while Main awaits the
     second wrapper. Only Main's later explicit await advances the first to 5. */
  else if (stage == 5)
    assert(runs == 2 && phases[0] >= 3 && phases[0] <= 4 && phases[1] == 5);
  else
    assert(0);
  prefix_stage = stage;
}
void supplant_busy(void) {
  same_owner();
  assert(prefix_stage == 2 && !runs && !active && !busy_count);
  busy_count++;
}
void supplant_mark(int32_t round, int32_t step) {
  same_owner();
  assert(round >= 1 && round <= 2);
  int *phase = &phases[round - 1];
  if (step == 1)
    assert(active == round && *phase == 0);
  else if (step == 3)
    assert(!active && *phase == 2);
  else if (step == 4)
    assert(!active && *phase == 3);
  else if (step == 5)
    assert(!active && *phase == 4);
  else
    assert(0);
  *phase = step;
  if (step == 3)
    atomic_store_explicit(&wrapper_ready[round - 1].done, 1, memory_order_release);
  else if (step == 4)
    atomic_store_explicit(&child_done[round - 1].done, 1, memory_order_release);
}
void *supplant_completion(int32_t round, int32_t child) {
  same_owner();
  assert(round >= 1 && round <= 2 && (child == 0 || child == 1));
  return child ? &child_done[round - 1] : &wrapper_ready[round - 1];
}
const char *supplant_run(int32_t (*callback)(int32_t), int32_t round) {
  same_owner();
  assert(callback && round == runs + 1 && round >= 1 && round <= 2 && !active);
  assert(busy_count == 1 && prefix_stage == (round == 1 ? 2 : 4));
  assert(!phases[round - 1]);
  if (round == 2)
    assert(phases[0] >= 3 && phases[0] <= 4 && !atomic_load(&first_children_released));
  callbacks[round - 1] = (const void *)callback;
  if (round == 2)
    assert(callbacks[0] != callbacks[1]);
  active = round;
  assert(callback(round) == 41 && phases[round - 1] == 1);
  if (round == 1)
    while (!atomic_load(&first_worker_entered))
      sched_yield();
  else
    atomic_store(&first_children_released, 1);
  phases[round - 1] = 2;
  active = 0;
  runs++;
  return results[round - 1];
}
int32_t supplant_worker_barrier(int32_t round) {
  assert(initialized && !pthread_equal(owner, pthread_self()) && round >= 1 && round <= 2);
  if (round == 1) {
    atomic_store(&first_worker_entered, 1);
    while (!atomic_load(&first_children_released))
      sched_yield();
  }
  return round;
}
int32_t supplant_finish(void) {
  same_owner();
  assert(!active && runs == 2 && prefix_stage == 5 && busy_count == 1 && phases[0] == 5 &&
         phases[1] == 5);
  return runs;
}
const char *supplant_run_alias(int32_t (*callback)(int32_t), int32_t round) {
  assert(round == 2);
  return supplant_run(callback, round);
}

/* These witnesses never request cancellation from the SDK. The runtime injects
   it at managed polls and must preserve callback/native physical returns. */
static int check_entries, check_finally, check_returns;
static _Atomic int blocked_worker_entered, blocked_worker_returned;
static void *unreadable;
static size_t unreadable_size;
void supplant_check_reset(int32_t process_main) {
  owner = pthread_self();
  initialized = 1;
  active = 0;
  assert(!unreadable);
  check_entries = check_finally = check_returns = 0;
  atomic_store(&blocked_worker_entered, 0);
  atomic_store(&blocked_worker_returned, 0);
  if (process_main)
    assert(pthread_main_np());
}
void supplant_check_inner(void) {
  same_owner();
  assert(active && check_finally < check_entries);
  check_finally++;
  fputs("SUPPLANT_CALLBACK_FINALLY\n", stderr);
}
void supplant_check_outer(int32_t expected) {
  same_owner();
  assert(!active && check_entries == expected && check_finally == expected &&
         check_returns == expected);
  if (unreadable) {
    assert(!munmap(unreadable, unreadable_size));
    unreadable = NULL;
  }
  fputs("SUPPLANT_OUTER_FINALLY\n", stderr);
}
void supplant_check_child(void) {
  same_owner();
  assert(!active && check_returns == 1);
  fputs("SUPPLANT_CHILD_FINALLY\n", stderr);
}
static void worker_input(const char *input, const uint8_t *bytes) {
  const uint8_t expected[4] = {0, 0xAA, 0, 0x7F};
  assert(initialized && !pthread_equal(owner, pthread_self()));
  assert(input && bytes && !strcmp(input, "worker argument 日本語") && !memcmp(bytes, expected, 4));
}
int32_t supplant_worker_block(const char *input, const uint8_t *bytes) {
  worker_input(input, bytes);
  assert(!atomic_load(&blocked_worker_entered));
  fputs("WORKER_BLOCKER_NATIVE_ENTER\n", stderr);
  atomic_store(&blocked_worker_entered, 1);
  struct timespec duration = {.tv_sec = 7, .tv_nsec = 0};
  while (nanosleep(&duration, &duration)) {
    assert(errno == EINTR);
  }
  worker_input(input, bytes);
  atomic_store(&blocked_worker_returned, 1);
  fputs("WORKER_BLOCKER_NATIVE_RETURN\n", stderr);
  return 41;
}
void supplant_wait_worker_entry(void) {
  same_owner();
  while (!atomic_load(&blocked_worker_entered))
    sched_yield();
  fputs("WORKER_BLOCKER_ORIGIN_OBSERVED_ENTRY\n", stderr);
}
void supplant_worker_finally(const char *input, const uint8_t *bytes) {
  worker_input(input, bytes);
  assert(atomic_load(&blocked_worker_returned));
  fputs("WORKER_BLOCKER_WORKER_FINALLY\n", stderr);
}
const char *supplant_check_run(int32_t (*callback)(int32_t), const char *input,
                               const uint8_t *bytes, int32_t mode) {
  fputs("SUPPLANT_NATIVE_ENTER\n", stderr);
  same_owner();
  assert(callback && !active && mode >= 1 && mode <= 4);
  active = 1;
  check_entries++;
  const uint8_t expected[4] = {0, 0xAA, 0, 0x7F};
  assert(input && bytes && !strcmp(input, "queued argument 日本語") && !memcmp(bytes, expected, 4));
  assert(callback(0) == 41 && check_finally == check_entries);
  if (mode == 4) {
    fputs("SUPPLANT_BLOCKED_AFTER_CALLBACK\n", stderr);
    struct timespec duration = {.tv_sec = 5, .tv_nsec = 200000000L};
    while (nanosleep(&duration, &duration)) {
      assert(errno == EINTR);
    }
  }
  assert(!strcmp(input, "queued argument 日本語") && !memcmp(bytes, expected, 4));
  active = 0;
  check_returns++;
  fputs("SUPPLANT_NATIVE_RETURN\n", stderr);
  if (mode == 1 || mode == 4) {
    assert(!unreadable);
    long page = sysconf(_SC_PAGESIZE);
    assert(page > 0);
    unreadable_size = (size_t)page;
    unreadable = mmap(NULL, unreadable_size, PROT_NONE, MAP_PRIVATE | MAP_ANON, -1, 0);
    assert(unreadable != MAP_FAILED);
    return unreadable;
  }
  if (mode == 2)
    return NULL;
  return "fresh result after settlement 日本語";
}
