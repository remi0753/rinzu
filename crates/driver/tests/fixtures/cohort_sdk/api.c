#define RINZU_COHORT_SDK 1
#include "observer.h"
#include "api.h"
#include "completion.h"
#include <assert.h>
#include <pthread.h>
#include <stdlib.h>
static rinzu_native_completion_v2_t *storage[3];
static pthread_t threads[3];
typedef struct {
  _Alignas(8) uint32_t prefix;
  rinzu_native_completion_t completion;
} PollingStorage;
_Static_assert(offsetof(PollingStorage, completion) == 4, "v1 field offset");
static PollingStorage *polling;
static int poll_retired;
static pthread_mutex_t gate = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t changed = PTHREAD_COND_INITIALIZER;
static int exchanged[4], started[3], retired[4], released, release_each[3], finished[3],
    notified[3];
static const int64_t payload[3] = {7183, 29, 65537};
static rinzu_native_completion_wake_t *batch_targets[4];
static void event(const char *k, int64_t g, int64_t a, int64_t b) {
  rinzu_cohort_event(k, (uint64_t)(100 + g), a, b);
}
static int extended(void) {
  return getenv("COHORT_EXTENDED") != NULL;
}
static int unordered(void) {
  return getenv("COHORT_ORDER") != NULL;
}
static int batched(void) {
  return getenv("COHORT_BATCH_FIRST") != NULL;
}
static int mixed_batches(void) {
  return getenv("COHORT_SERIES_FIRST") != NULL;
}
static int storage_slot(int64_t g) {
  return g == 2 ? 0 : g == 3 ? 2 : (int)g;
}
int cohort_wave_first(int64_t wave) {
  const char *first = getenv("COHORT_SERIES_FIRST");
  assert(first && (first[0] == '0' || first[0] == '1') && (first[1] == '0' || first[1] == '1') &&
         !first[2]);
  assert(wave == 0 || wave == 1);
  return (int)(2 * wave) + first[wave] - '0';
}
int cohort_batch_first(void) {
  const char *first = getenv("COHORT_BATCH_FIRST");
  assert(first && (first[0] == '1' || first[0] == '2') && !first[1]);
  return first[0] - '0';
}
int cohort_staggered(void) {
  return getenv("COHORT_STAGGERED") != NULL;
}
int cohort_late_arrival(void) {
  return getenv("COHORT_LATE_ARRIVAL") != NULL;
}
static void publish_after_selection(void) {
  (void)cohort_capture(2);
  (void)cohort_wave_notify(1);
}
static void finish_capture(int g) {
  rinzu_cohort_producer((uint64_t)(100 + g));
  rinzu_native_completion_wake_t *target = batch_targets[g];
  assert(target);
  batch_targets[g] = NULL;
  event("producer_release", g, 0, 0);
  target->release(target);
  event("producer_released", g, 0, 0);
  event("publish_end", g, 0, 0);
  rinzu_cohort_producer(0);
}
int cohort_recovery_step(int64_t step) {
  static const int orders[6][3] = {{0, 1, 2}, {0, 2, 1}, {1, 0, 2},
                                   {1, 2, 0}, {2, 0, 1}, {2, 1, 0}};
  const char *order = getenv("COHORT_ORDER");
  assert(order && order[0] >= '0' && order[0] <= '5' && !order[1]);
  assert(step >= 0 && step < 3);
  return orders[order[0] - '0'][step];
}
static int early_notify(int64_t g) {
  return extended() && cohort_polling() && cohort_controlled() && g == 0;
}
static void publish_in_callback(void) {
  (void)cohort_publish(1);
}
static void finish_producers(void) {
  if (mixed_batches()) {
    int first = getenv("COHORT_RELEASE_REVERSE") ? 1 : 0;
    finish_capture(cohort_wave_first(first));
    finish_capture(cohort_wave_first(1 - first));
    return;
  }
  if (batched()) {
    assert(!pthread_mutex_lock(&gate));
    released = 1;
    assert(!pthread_cond_broadcast(&changed));
    assert(!pthread_mutex_unlock(&gate));
    assert(started[0] && !pthread_join(threads[0], NULL));
    finish_capture(cohort_batch_first());
    return;
  }
  if (extended() || unordered()) {
    for (int g = 0; g < cohort_count(); g++) {
      assert(!pthread_mutex_lock(&gate));
      release_each[g] = 1;
      assert(!pthread_cond_broadcast(&changed));
      while (!finished[g])
        assert(!pthread_cond_wait(&changed, &gate));
      assert(!pthread_mutex_unlock(&gate));
    }
  }
  assert(!pthread_mutex_lock(&gate));
  released = 1;
  assert(!pthread_cond_broadcast(&changed));
  assert(!pthread_mutex_unlock(&gate));
  for (int g = 0; g < cohort_count(); g++) {
    assert(started[g]);
    assert(!pthread_join(threads[g], NULL));
  }
}
CohortCompletion *cohort_start(int64_t g, size_t (*callback)(void)) {
  assert(g >= 0 && g < cohort_count());
  int slot = storage_slot(g);
  if (g == 0) {
    assert(!storage[0]);
    for (int i = 0; i < (mixed_batches() ? 3 : 2); i++) {
      size_t size = mixed_batches() && i != 0 ? sizeof(rinzu_native_completion_v2_t)
                                              : sizeof(rinzu_native_completion_v3_t);
      storage[i] = calloc(1, size);
      assert(storage[i]);
    }
    rinzu_cohort_controller(finish_producers);
  }
  if (g == 2)
    assert(retired[0]);
  rinzu_native_completion_v2_t *carrier = storage[slot];
  rinzu_native_completion_v2_init(carrier);
  if (!mixed_batches() || g % 2 == 0) {
    rinzu_native_completion_v3_t *transfer = (rinzu_native_completion_v3_t *)carrier;
    carrier->base.version = 3;
    carrier->base.size = sizeof(*transfer);
    transfer->result = payload[g];
  }
  event("sdk_start", g, 0, 0);
  event("generation", g, (intptr_t)carrier, slot);
  if (mixed_batches())
    event("carrier_abi", g, carrier->base.version, carrier->base.size);
  if (extended() && g == 1)
    rinzu_cohort_callback_publisher(publish_in_callback);
  assert(callback() == 7183);
  return (CohortCompletion *)carrier;
}
static void *publish(void *raw) {
  int64_t g = (int64_t)(intptr_t)raw;
  rinzu_native_completion_v2_t *carrier = storage[storage_slot(g)];
  rinzu_cohort_producer((uint64_t)(100 + g));
  event("publish_begin", g, 0, 0);
  /* The final exchange transfers all subsequent accesses to the retained target.
     No carrier pointer is dereferenced below, even if this address is reused. */
  rinzu_native_completion_wake_t *target = rinzu_native_completion_v2_publish_target(carrier);

  assert(!pthread_mutex_lock(&gate));
  exchanged[g] = 1;
  assert(!pthread_cond_broadcast(&changed));
  assert(!pthread_mutex_unlock(&gate));
  if (target && early_notify(g)) {
    target->notify(target);
    assert(!pthread_mutex_lock(&gate));
    notified[g] = 1;
    assert(!pthread_cond_broadcast(&changed));
    assert(!pthread_mutex_unlock(&gate));
  }
  assert(!pthread_mutex_lock(&gate));
  while (target && !((extended() || unordered()) ? release_each[g] : released))
    assert(!pthread_cond_wait(&changed, &gate));
  assert(!pthread_mutex_unlock(&gate));
  if (target) {
    if (!early_notify(g))
      target->notify(target);
    event("producer_release", g, 0, 0);
    target->release(target);
    event("producer_released", g, 0, 0);
  }
  event("publish_end", g, 0, 0);
  assert(!pthread_mutex_lock(&gate));
  finished[g] = 1;
  assert(!pthread_cond_broadcast(&changed));
  assert(!pthread_mutex_unlock(&gate));
  return NULL;
}
int cohort_publish(int64_t g) {
  assert(g >= 0 && g < cohort_count());
  if (g == 1 && started[g] && cohort_early())
    return 1;
  assert(!started[g]);
  started[g] = 1;
  assert(!pthread_create(&threads[g], NULL, publish, (void *)(intptr_t)g));
  assert(!pthread_mutex_lock(&gate));
  while (!exchanged[g] || (early_notify(g) && !notified[g]) ||
         (extended() && g == 1 && !finished[g]))
    assert(!pthread_cond_wait(&changed, &gate));
  assert(!pthread_mutex_unlock(&gate));
  return 1;
}
int cohort_capture(int64_t g) {
  assert(g >= 0 && g < cohort_count() && !exchanged[g]);
  assert(mixed_batches() || (batched() && (g == 1 || g == 2)));
  rinzu_cohort_producer((uint64_t)(100 + g));
  event("publish_begin", g, 0, 0);
  batch_targets[g] = rinzu_native_completion_v2_publish_target(storage[storage_slot(g)]);
  assert(batch_targets[g]);
  exchanged[g] = 1;
  event("batch_target", g, (intptr_t)batch_targets[g], 0);
  rinzu_cohort_producer(0);
  if (g == 3 && cohort_late_arrival())
    rinzu_cohort_publish_after_selection(publish_after_selection);
  return 1;
}
static int notify_batch(int leader, int duplicate) {
  rinzu_native_completion_wake_t *target = batch_targets[leader];
  assert(target && target == batch_targets[duplicate]);
  event("merge_wake", duplicate, 100 + leader, (intptr_t)target);
  finish_capture(duplicate);
  rinzu_cohort_producer((uint64_t)(100 + leader));
  target->notify(target);
  rinzu_cohort_producer(0);
  return 1;
}
int cohort_notify_batch(void) {
  int leader = cohort_batch_first();
  return notify_batch(leader, 3 - leader);
}
int cohort_wave_notify(int64_t wave) {
  int leader = cohort_wave_first(wave);
  return notify_batch(leader, leader ^ 1);
}
int cohort_retire(int64_t g) {
  assert(g >= 0 && g < cohort_count() && exchanged[g] && !retired[g]);
  retired[g] = 1;
  event("close_admission", g, 0, 0);
  if (unordered() || ((batched() || mixed_batches()) && g != 0)) {
    int slot = storage_slot(g);
    free(storage[slot]);
    storage[slot] = NULL;
  }
  event("carrier_free", g, 0, 0);
  event("sdk_pin_free", g, 0, 0);
  return 1;
}
int cohort_alias(int64_t value) {
  assert(value == payload[0]);
  event("alias_result", 0, value, 1);
  return 1;
}
int cohort_status(void) {
  rinzu_native_completion_v2_t *c = storage[0];
  int cancelled = (int)atomic_load_explicit(&c->base.cancelled, memory_order_acquire);
  int done = (int)atomic_load_explicit(&c->base.done, memory_order_acquire);
  event("generation_status", 2, cancelled, done);
  assert(!cancelled && !done);
  return 1;
}
int cohort_finish(void) {
  for (int g = 0; g < cohort_count(); g++)
    assert(retired[g]);
  for (size_t i = 0; i < sizeof(storage) / sizeof(storage[0]); i++) {
    free(storage[i]);
    storage[i] = NULL;
  }
  if (cohort_polling())
    assert(poll_retired);
  return 1;
}
int cohort_count(void) {
  return mixed_batches() ? 4 : getenv("COHORT_REUSE") ? 3 : 2;
}
int cohort_controlled(void) {
  return getenv("COHORT_CONTROLLED") != NULL;
}

int cohort_early(void) {
  return getenv("COHORT_EARLY") != NULL;
}
int cohort_polling(void) {
  return getenv("COHORT_POLLING") != NULL;
}
CohortCompletion *cohort_poll_start(size_t (*callback)(void)) {
  polling = calloc(1, sizeof(*polling));
  assert(polling);
  rinzu_native_completion_t *completion = &polling->completion;
  assert((uintptr_t)completion % 8 == 4);
  rinzu_native_completion_init(completion);
  rinzu_cohort_event("sdk_start", 200, 0, 0);
  rinzu_cohort_event("generation", 200, (intptr_t)completion, mixed_batches() ? 4 : 2);
  if (mixed_batches())
    rinzu_cohort_event("carrier_abi", 200, completion->version, completion->size);
  assert(callback() == 7183);
  return (CohortCompletion *)completion;
}
int cohort_poll_publish(void) {
  rinzu_cohort_event("poll_publish", 200, 0, 0);
  atomic_store_explicit(&polling->completion.done, 1, memory_order_release);
  rinzu_cohort_wait_helper();
  return 1;
}
int cohort_poll_retire(void) {
  assert(atomic_load_explicit(&polling->completion.done, memory_order_acquire));
  poll_retired = 1;
  rinzu_cohort_event("close_admission", 200, 0, 0);
  free(polling);
  polling = NULL;
  rinzu_cohort_event("carrier_free", 200, 0, 0);
  rinzu_cohort_event("sdk_pin_free", 200, 0, 0);
  return 1;
}
