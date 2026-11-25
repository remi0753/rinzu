#include <assert.h>
#include <inttypes.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static pthread_mutex_t log_gate = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t changed = PTHREAD_COND_INITIALIZER;
static unsigned scenario, admissions, producer_ready, prefixes, exchanges, race_ready;
static uint64_t first_id;
static int begin_allowed, exchange_allowed;
static void event(const char *kind, uint64_t id, int64_t a, int64_t b) {
  if (!strcmp(kind, "carrier_bind"))
    return;
  assert(!pthread_mutex_lock(&log_gate));
  printf("%s,%" PRIu64 ",%" PRId64 ",%" PRId64 "\n", kind, id, a, b);
  if (!strcmp(kind, "admit") && !admissions++)
    first_id = id;
  if (!strcmp(kind, "sdk_prefix"))
    ++prefixes;
  if (!strcmp(kind, "sdk_exchange"))
    ++exchanges;
  assert(!pthread_cond_broadcast(&changed));
  if (id == first_id && admissions &&
      ((scenario == 1 && !strcmp(kind, "register_begin")) ||
       (scenario == 2 && !strcmp(kind, "register_retain")) ||
       (scenario == 3 && !strcmp(kind, "register_cas")))) {
    begin_allowed = 1;
    assert(!pthread_cond_broadcast(&changed));
    while (!exchanges)
      assert(!pthread_cond_wait(&changed, &log_gate));
  }
  if (scenario == 7 && id == first_id && !strcmp(kind, "register_retain")) {
    begin_allowed = 1;
    assert(!pthread_cond_broadcast(&changed));
    while (!prefixes)
      assert(!pthread_cond_wait(&changed, &log_gate));
    race_ready = 1;
    assert(!pthread_cond_broadcast(&changed));
  }
  if (!strcmp(kind, "sdk_prefix")) {
    while (!exchange_allowed || (scenario == 7 && !race_ready))
      assert(!pthread_cond_wait(&changed, &log_gate));
  }
  assert(!pthread_mutex_unlock(&log_gate));
}
#define RINZU_NATIVE_LIFETIME_EVENT(kind, id, a, b) event(kind, id, a, b)
#include "rinzu/source_async.h"

typedef struct {
  unsigned magic;
} environment_t;
static void wake_context(void *raw) {
  environment_t *environment = raw;
  assert(environment->magic == 7183);
  event("context_touch", 0, 0, 0);
  assert(environment->magic == 7183);
}
typedef struct {
  rinzu_native_completion_v3_t *carrier;
} publisher_t;
static void *publish(void *raw) {
  publisher_t *p = raw;
  assert(!pthread_mutex_lock(&log_gate));
  producer_ready = 1;
  assert(!pthread_cond_broadcast(&changed));
  while (!begin_allowed)
    assert(!pthread_cond_wait(&changed, &log_gate));
  assert(!pthread_mutex_unlock(&log_gate));
  event("publish_begin", 0, 0, 0);
  rinzu_native_completion_wake_t *target =
      rinzu_native_completion_v2_publish_target(&p->carrier->completion);
  event("publish_end", 0, 0, 0);
  if (target) {
    target->notify(target);
    event("producer_release", 0, 0, 0);
    target->release(target);
    event("producer_released", 0, 0, 0);
  }
  return NULL;
}
static void wait_for(unsigned *counter) {
  assert(!pthread_mutex_lock(&log_gate));
  while (!*counter)
    assert(!pthread_cond_wait(&changed, &log_gate));
  assert(!pthread_mutex_unlock(&log_gate));
}
static void allow(int *gate) {
  assert(!pthread_mutex_lock(&log_gate));
  *gate = 1;
  assert(!pthread_cond_broadcast(&changed));
  assert(!pthread_mutex_unlock(&log_gate));
}
static rinzu_async_future_t *admit(rinzu_async_runtime_t *runtime,
                                   rinzu_native_completion_v3_t *carrier) {
  rinzu_async_future_t *future = rinzu_async_new(runtime, RINZU_ASYNC_COMPLETION, RINZU_SCALAR_I64,
                                                 (rinzu_source_position_t){0});
  future->native_completion = &carrier->completion.base;
  rinzu_async_start(future);
  return future;
}
int main(void) {
  for (unsigned count = 1; count <= 2; ++count) {
    for (scenario = 0; scenario < 8; ++scenario) {
      printf("BEGIN %u %u\n", count, scenario);
      admissions = producer_ready = prefixes = exchanges = race_ready = 0;
      begin_allowed = scenario == 0 || scenario == 6;
      exchange_allowed = scenario != 6;
      rinzu_async_runtime_t *runtime = calloc(1, sizeof(*runtime));
      environment_t *environment = malloc(sizeof(*environment));
      rinzu_native_completion_v3_t *carrier = calloc(1, sizeof(*carrier));
      assert(runtime && environment && carrier);
      environment->magic = 7183;
      rinzu_async_init(runtime);
      runtime->completion_wake = wake_context;
      runtime->wake_context = environment;
      rinzu_native_completion_v2_init(&carrier->completion);
      carrier->completion.base.version = 3;
      carrier->completion.base.size = sizeof(*carrier);
      carrier->result = 7183;
      publisher_t publisher = {carrier};
      pthread_t producer;
      assert(!pthread_create(&producer, NULL, publish, &publisher));
      wait_for(&producer_ready);
      int joined = 0;
      if (scenario == 0) {
        assert(!pthread_join(producer, NULL));
        joined = 1;
      }
      if (scenario == 6) {
        wait_for(&prefixes);
        assert(!rinzu_native_completion_is_done(&carrier->completion.base));
      }
      rinzu_async_future_t *f[2] = {admit(runtime, carrier), NULL};
      if (scenario == 5) {
        allow(&begin_allowed);
        assert(!pthread_join(producer, NULL));
        joined = 1;
      }
      if (count == 2)
        f[1] = admit(runtime, carrier);
      if (scenario == 4)
        allow(&begin_allowed);
      if (scenario == 6) {
        assert(!rinzu_native_completion_is_done(&carrier->completion.base));
        for (unsigned i = 0; i < count; ++i)
          assert(!rinzu_async_try_collect(f[i]));
        allow(&exchange_allowed);
      }
      if (!joined)
        assert(!pthread_join(producer, NULL));
      event("close_admission", 0, 0, 0);
      rinzu_async_native_progress(runtime);
      for (unsigned i = 0; i < count; ++i) {
        assert(rinzu_async_try_collect(f[i]));
        assert(f[i]->result_slot == 7183);
      }
      free(carrier);
      event("carrier_free", 0, 0, 0);
      for (unsigned i = 0; i < count; ++i)
        rinzu_async_release(f[i]);
      event("shutdown", 0, 0, 0);
      rinzu_async_clear_completion_wake(runtime);
      rinzu_async_dispose(runtime);
      free(environment);
      free(runtime);
      event("env_free", 0, 0, 0);
      for (unsigned i = 0; i < RINZU_RESOURCE_COUNT; ++i)
        assert(!rinzu_resource_counters[i].live);
      puts("END");
    }
  }
}
