#include <assert.h>
#include <pthread.h>
#include <stdatomic.h>
#include <sched.h>
#include <unistd.h>
static int a18_wait(pthread_cond_t *, pthread_mutex_t *);
#define pthread_cond_wait a18_wait
#define RINZU_FOREIGN_TESTING 1
#include "rinzu/native_diagnostics.h"
#undef pthread_cond_wait
static pthread_cond_t *observed_condition;
static _Atomic unsigned wait_entries, inside;
#if RINZU_ACTIVITY_GATE_LEASE
static _Atomic unsigned gap, publisher;
#endif
static uint64_t shared;
static int a18_wait(pthread_cond_t *c, pthread_mutex_t *m) {
  if (c == observed_condition)
    atomic_fetch_add_explicit(&wait_entries, 1, memory_order_release);
  return pthread_cond_wait(c, m);
}
#if RINZU_ACTIVITY_GATE_LEASE
static void until(_Atomic unsigned *p, unsigned n) {
  while (atomic_load_explicit(p, memory_order_acquire) < n)
    sched_yield();
}
#endif
static void init(rinzu_activity_board_t *b) {
  *b = (rinzu_activity_board_t){0};
  rinzu_activity_gate_init(b);
  assert(!pthread_cond_init(&b->changed, NULL));
}
static void dispose(rinzu_activity_board_t *b) {
  assert(!pthread_cond_destroy(&b->changed));
  rinzu_activity_gate_dispose(b);
}
static void enter(rinzu_activity_board_t *b, int slow) {
  if (slow)
    rinzu_activity_gate_lock_slow(b);
  else
    rinzu_activity_gate_lock(b);
  assert(rinzu_foreign_held_locks == 1);
  assert(atomic_fetch_add_explicit(&inside, 1, memory_order_relaxed) == 0);
}
static void leave(rinzu_activity_board_t *b) {
  assert(atomic_fetch_sub_explicit(&inside, 1, memory_order_relaxed) == 1);
  rinzu_activity_gate_unlock(b);
  assert(!rinzu_foreign_held_locks);
}
#if RINZU_ACTIVITY_GATE_LEASE
static void *slow_once(void *raw) {
  rinzu_activity_board_t *b = raw;
  enter(b, 1);
  ++shared;
  leave(b);
  return NULL;
}
#endif
static void *stress(void *raw) {
  rinzu_activity_board_t *b = raw;
  for (unsigned i = 0; i < 8192; ++i) {
    errno = 73;
    enter(b, i % 17 == 0);
    ++shared;
    if (!(i % 31))
      sched_yield();
    leave(b);
    assert(errno == 73);
  }
  return NULL;
}
#if RINZU_ACTIVITY_GATE_LEASE
static void handoff(unsigned promote) {
  rinzu_activity_board_t b;
  init(&b);
  shared = 0;
  atomic_store(&wait_entries, 0);
  observed_condition = &b.gate_handoff;
  rinzu_activity_gate_lock(&b);
  assert(atomic_load(&b.gate_lease) == 1);
  pthread_t threads[6];
  for (unsigned i = 0; i < 6; ++i)
    assert(!pthread_create(&threads[i], NULL, slow_once, &b));
  until(&wait_entries, 6);
  assert(atomic_load(&b.gate_lease) == 3);
  if (promote) {
    rinzu_activity_gate_signal(&b);
    assert(atomic_load(&b.gate_lease) == 2 && rinzu_foreign_held_locks == 1);
  }
  rinzu_activity_gate_unlock(&b);
  assert(!rinzu_foreign_held_locks);
  for (unsigned i = 0; i < 6; ++i)
    assert(!pthread_join(threads[i], NULL));
  observed_condition = NULL;
  assert(shared == 6 && !atomic_load(&b.gate_lease));
  dispose(&b);
}
static void *gap_waiter(void *raw) {
  rinzu_activity_board_t *b = raw;
  rinzu_activity_gate_lock_slow(b);
  assert(!b->stats.active);
  rinzu_activity_gate_before_wait(b);
  atomic_store_explicit(&gap, 1, memory_order_release);
  until(&publisher, 1);
  assert(!pthread_cond_wait(&b->changed, &b->gate));
  rinzu_activity_gate_claim_locked(b);
  assert(b->stats.active == 1 && rinzu_foreign_held_locks == 1);
  rinzu_activity_gate_unlock(b);
  assert(!rinzu_foreign_held_locks);
  return NULL;
}
static void notify_gap(void) {
  rinzu_activity_board_t b;
  init(&b);
  atomic_store(&gap, 0);
  atomic_store(&publisher, 0);
  observed_condition = NULL;
  pthread_t thread;
  assert(!pthread_create(&thread, NULL, gap_waiter, &b));
  until(&gap, 1);
  rinzu_activity_gate_lock(&b);
  assert(atomic_load(&b.gate_lease) == 1);
  b.stats.active = 1;
  atomic_store_explicit(&publisher, 1, memory_order_release);
  rinzu_activity_gate_signal(&b);
  rinzu_activity_gate_unlock(&b);
  assert(!pthread_join(thread, NULL));
  dispose(&b);
}
static void *waking_waiter(void *raw) {
  rinzu_activity_board_t *b = raw;
  rinzu_activity_gate_lock_slow(b);
  rinzu_activity_gate_before_wait(b);
  assert(!a18_wait(&b->changed, &b->gate));
  rinzu_activity_gate_claim_locked(b);
  assert(shared == 777);
  rinzu_activity_gate_unlock(b);
  return NULL;
}
static void wake_during_fast(void) {
  rinzu_activity_board_t b;
  init(&b);
  shared = 0;
  atomic_store(&wait_entries, 0);
  observed_condition = &b.changed;
  pthread_t thread;
  assert(!pthread_create(&thread, NULL, waking_waiter, &b));
  until(&wait_entries, 1);
  /* Scheduling barrier only: the watcher reaches its real cond_wait before
     this lock can return. No board data is accessed through this raw mutex. */
  assert(!pthread_mutex_lock(&b.gate));
  assert(!pthread_mutex_unlock(&b.gate));
  rinzu_activity_gate_lock(&b);
  assert(atomic_load(&b.gate_lease) == 1);
  assert(!pthread_cond_signal(&b.changed));
  while (atomic_load_explicit(&b.gate_lease, memory_order_acquire) != 3)
    sched_yield();
  shared = 777;
  rinzu_activity_gate_unlock(&b);
  assert(!pthread_join(thread, NULL));
  observed_condition = NULL;
  dispose(&b);
}
#endif
int main(int argc, char **argv) {
  alarm(45);
  if (argc > 1) {
    rinzu_activity_board_t b;
    init(&b);
    if (!strcmp(argv[1], "unlocked")) {
      rinzu_activity_gate_unlock(&b);
      return 1;
    }
    if (!strcmp(argv[1], "double")) {
      rinzu_activity_gate_lock(&b);
      rinzu_activity_gate_unlock(&b);
      rinzu_activity_gate_unlock(&b);
      return 1;
    }
#if RINZU_ACTIVITY_GATE_LEASE
    if (!strcmp(argv[1], "state")) {
      atomic_store(&b.gate_lease, 4);
      rinzu_activity_gate_lock(&b);
      return 1;
    }
    if (!strcmp(argv[1], "wait")) {
      rinzu_activity_gate_lock(&b);
      rinzu_activity_gate_before_wait(&b);
      return 1;
    }
#endif
    return 2;
  }
#if RINZU_ACTIVITY_GATE_LEASE
  for (unsigned i = 0; i < 32; ++i) {
    handoff(0);
    handoff(1);
    notify_gap();
    wake_during_fast();
  }
#endif
  observed_condition = NULL;
  rinzu_activity_board_t b;
  init(&b);
  shared = 0;
  pthread_t threads[8];
  for (unsigned i = 0; i < 8; ++i)
    assert(!pthread_create(&threads[i], NULL, stress, &b));
  for (unsigned i = 0; i < 8; ++i)
    assert(!pthread_join(threads[i], NULL));
  assert(shared == 65536 && !atomic_load(&inside));
  dispose(&b);
  fprintf(stderr,
          "PASS "
          "A18:lease=%d:handoff6:promote:wait-gap:wake-during-fast:32rounds:65536transactions:"
          "errno:logical-locks\n",
          RINZU_ACTIVITY_GATE_LEASE);
  return 0;
}
