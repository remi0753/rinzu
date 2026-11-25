#include "observer.h"
#include <assert.h>
#include <inttypes.h>
#include <stdio.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdlib.h>
static _Thread_local uint64_t producer;
static uint64_t current_call;
static intptr_t current_carrier;
static unsigned calls, receipts[5];
static void (*finish_producers)(void);
static void (*callback_publisher)(void);
static void (*selection_publisher)(void);
static _Atomic uint64_t late_receipt = UINT64_MAX;
static uint64_t selected_receipt;
void rinzu_cohort_callback_publisher(void (*f)(void)) {
  callback_publisher = f;
}
static pthread_mutex_t helper_gate = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t helper_changed = PTHREAD_COND_INITIALIZER;
static int helper_held, join_started;
static int poll_published, late_physical;
void rinzu_cohort_publish_after_selection(void (*publish)(void)) {
  assert(!selection_publisher);
  selection_publisher = publish;
}
void rinzu_cohort_ready_selection(uint64_t id) {
  if (!selection_publisher || id != selected_receipt)
    return;
  void (*publish)(void) = selection_publisher;
  selection_publisher = NULL;
  rinzu_cohort_event("ready_selection", id, 0, 0);
  publish();
  assert(!pthread_mutex_lock(&helper_gate));
  while (!late_physical)
    assert(!pthread_cond_wait(&helper_changed, &helper_gate));
  assert(!pthread_mutex_unlock(&helper_gate));
}
void rinzu_cohort_wait_helper(void) {
  assert(!pthread_mutex_lock(&helper_gate));
  while (!helper_held)
    assert(!pthread_cond_wait(&helper_changed, &helper_gate));
  assert(!pthread_mutex_unlock(&helper_gate));
}
void rinzu_cohort_controller(void (*f)(void)) {
  finish_producers = f;
}
void rinzu_cohort_producer(uint64_t id) {
  producer = id;
}
static void row(const char *k, uint64_t id, int64_t a, int64_t b) {
  printf("%s,%" PRIu64 ",%" PRId64 ",%" PRId64 "\n", k, id, a, b);
}
void rinzu_cohort_event(const char *k, uint64_t id, int64_t a, int64_t b) {
  flockfile(stdout);
  if (!strcmp(k, "generation")) {
    assert(id == current_call);
    current_carrier = a;
  }
  if (!strcmp(k, "carrier_bind")) {
    assert(current_carrier && a == current_carrier);
    funlockfile(stdout);
    return;
  }
  if (!strcmp(k, "admit")) {
    assert((current_call >= 100 && current_call < 104) || current_call == 200);
    unsigned g = current_call == 200 ? 4 : (unsigned)(current_call - 100), i = receipts[g]++;
    assert(i < (current_call == 200 ? 1u : 2u));
    if (getenv("COHORT_LATE_ARRIVAL") && g == 2)
      atomic_store(&late_receipt, id);
    if (getenv("COHORT_LATE_ARRIVAL") && g == 3)
      selected_receipt = id;
    row("future_bind", id, (int64_t)current_call, i);
    row("drain_receipt", id, 0, 0);
    row("receipt_link", id, (int64_t)id, 0);
    row("receipt", id, (int64_t)current_call, 0);
  }
  if (!strcmp(k, "record_release"))
    row("drain_release", id, 0, 0);
  if (producer &&
      (!strcmp(k, "sdk_prefix") || !strcmp(k, "sdk_exchange") || !strcmp(k, "notify_enter") ||
       !strcmp(k, "notify_ready") || !strcmp(k, "notify_leave") || !strcmp(k, "producer_release") ||
       !strcmp(k, "producer_released") || !strcmp(k, "context_touch")))
    id = producer;
  if (!strcmp(k, "admit") && current_call == 200)
    row("poll_bind", 200, 1, 0);
  row(k, id, a, b);
  funlockfile(stdout);
  int late = getenv("COHORT_LATE_ARRIVAL") != NULL;
  if (late && (!strcmp(k, "poll_publish") ||
               (!strcmp(k, "physical") && id == atomic_load(&late_receipt)))) {
    assert(!pthread_mutex_lock(&helper_gate));
    if (!strcmp(k, "poll_publish"))
      poll_published = 1;
    else
      late_physical = 1;
    assert(!pthread_cond_broadcast(&helper_changed));
    assert(!pthread_mutex_unlock(&helper_gate));
  }
  if (getenv("COHORT_POLLING") &&
      (!strcmp(k, "helper_wake_begin") || !strcmp(k, "helper_join_wait"))) {
    assert(!pthread_mutex_lock(&helper_gate));
    if (!strcmp(k, "helper_join_wait")) {
      join_started = 1;
      assert(!pthread_cond_broadcast(&helper_changed));
    } else if (!helper_held && (!late || poll_published)) {
      helper_held = 1;
      assert(!pthread_cond_broadcast(&helper_changed));
      while (!join_started)
        assert(!pthread_cond_wait(&helper_changed, &helper_gate));
    }
    assert(!pthread_mutex_unlock(&helper_gate));
  }
  if (!strcmp(k, "env_free")) {
    assert(finish_producers);
    finish_producers();
  }
}
void rinzu_cohort_call(const char *k, const char *symbol) {
  int poll = !strcmp(symbol, "cohort_poll_start");
  if (strcmp(symbol, "cohort_start") && !poll)
    return;
  if (!strcmp(k, "call_enter")) {
    assert(poll || calls < (getenv("COHORT_SERIES_FIRST") ? 4u : 3u));
    current_call = poll ? 200 : 100 + calls++;
    current_carrier = 0;
  }
  rinzu_cohort_event(k, current_call, 0, 0);
}
void rinzu_cohort_callback(const char *k) {
  rinzu_cohort_event(k, current_call, 0, 0);
  if (!strcmp(k, "callback_begin") && callback_publisher) {
    void (*publish)(void) = callback_publisher;
    callback_publisher = NULL;
    publish();
  }
}
