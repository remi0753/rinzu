#include <pthread.h>
#include <errno.h>
#include <string.h>
static unsigned fault;
static int failure;
static int gate_cond_init(pthread_cond_t *, const pthread_condattr_t *);
static int gate_mutex_lock(pthread_mutex_t *);
static int gate_mutex_unlock(pthread_mutex_t *);
static int gate_cond_wait(pthread_cond_t *, pthread_mutex_t *);
static int gate_cond_signal(pthread_cond_t *);
static int gate_cond_broadcast(pthread_cond_t *);
#define pthread_cond_init gate_cond_init
#define pthread_mutex_lock gate_mutex_lock
#define pthread_mutex_unlock gate_mutex_unlock
#define pthread_cond_wait gate_cond_wait
#define pthread_cond_signal gate_cond_signal
#define pthread_cond_broadcast gate_cond_broadcast
#include "rinzu/native_diagnostics.h"
#undef pthread_cond_init
#undef pthread_mutex_lock
#undef pthread_mutex_unlock
#undef pthread_cond_wait
#undef pthread_cond_signal
#undef pthread_cond_broadcast
static int gate_cond_init(pthread_cond_t *p, const pthread_condattr_t *a) {
  return fault == 1 ? failure : pthread_cond_init(p, a);
}
static int gate_mutex_lock(pthread_mutex_t *p) {
  return fault == 2 ? failure : pthread_mutex_lock(p);
}
static int gate_mutex_unlock(pthread_mutex_t *p) {
  return fault == 6 ? failure : pthread_mutex_unlock(p);
}
static int gate_cond_wait(pthread_cond_t *p, pthread_mutex_t *m) {
  return fault == 3 ? failure : pthread_cond_wait(p, m);
}
static int gate_cond_signal(pthread_cond_t *p) {
  return fault == 4 ? failure : pthread_cond_signal(p);
}
static int gate_cond_broadcast(pthread_cond_t *p) {
  return fault == 5 ? failure : pthread_cond_broadcast(p);
}
int main(int argc, char **argv) {
  if (argc != 2)
    return 2;
  rinzu_activity_board_t b = {0};
  failure = EINVAL;
  if (!strcmp(argv[1], "init-memory") || !strcmp(argv[1], "init-again")) {
    fault = 1;
    failure = !strcmp(argv[1], "init-memory") ? ENOMEM : EAGAIN;
    rinzu_activity_gate_init(&b);
    return 1;
  }
  rinzu_activity_gate_init(&b);
  rinzu_foreign_sync(pthread_cond_init(&b.changed, NULL));
  if (!strcmp(argv[1], "slow-lock")) {
    atomic_store(&b.gate_lease, 2);
    fault = 2;
    rinzu_activity_gate_lock(&b);
    return 1;
  }
  if (!strcmp(argv[1], "claim-wait")) {
    atomic_store(&b.gate_lease, 1);
    fault = 3;
    rinzu_activity_gate_lock_slow(&b);
    return 1;
  }
  if (!strcmp(argv[1], "physical-unlock")) {
    rinzu_activity_gate_lock_slow(&b);
    fault = 6;
    rinzu_activity_gate_unlock(&b);
    return 1;
  }
  rinzu_activity_gate_lock(&b);
  if (!strcmp(argv[1], "promote-lock")) {
    fault = 2;
    rinzu_activity_gate_signal(&b);
    return 1;
  }
  if (!strcmp(argv[1], "signal")) {
    fault = 4;
    rinzu_activity_gate_signal(&b);
    return 1;
  }
  atomic_store(&b.gate_lease, 3);
  if (!strcmp(argv[1], "handoff-lock")) {
    fault = 2;
    rinzu_activity_gate_unlock(&b);
    return 1;
  }
  if (!strcmp(argv[1], "broadcast")) {
    fault = 5;
    rinzu_activity_gate_unlock(&b);
    return 1;
  }
  return 2;
}
