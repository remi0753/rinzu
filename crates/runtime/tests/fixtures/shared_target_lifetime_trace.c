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
static _Thread_local uint64_t producer;
static uintptr_t addresses[2];
static uint64_t generations[2];
static unsigned exchanged[3], allowed[3], destroyed, context_entered, detach_waiting,
    context_allowed;
static void event(const char *kind, uint64_t id, int64_t a, int64_t b) {
  assert(!pthread_mutex_lock(&log_gate));
  if (!strcmp(kind, "carrier_bind")) {
    unsigned s = (uintptr_t)a == addresses[0] ? 0 : 1;
    assert((uintptr_t)a == addresses[s]);
    kind = "receipt";
    a = (int64_t)generations[s];
  }
  if (producer && (!strcmp(kind, "publish_begin") || !strcmp(kind, "publish_end") ||
                   !strcmp(kind, "sdk_prefix") || !strcmp(kind, "sdk_exchange") ||
                   !strcmp(kind, "notify_enter") || !strcmp(kind, "notify_ready") ||
                   !strcmp(kind, "notify_leave") || !strcmp(kind, "producer_release") ||
                   !strcmp(kind, "producer_released") || !strcmp(kind, "context_touch")))
    id = producer;
  printf("%s,%" PRIu64 ",%" PRId64 ",%" PRId64 "\n", kind, id, a, b);
  if (!strcmp(kind, "target_free"))
    ++destroyed;
  if (!strcmp(kind, "sdk_exchange")) {
    unsigned g = (unsigned)(id - 100);
    assert(g < 3);
    exchanged[g] = 1;
    assert(!pthread_cond_broadcast(&changed));
    while (a && !allowed[g])
      assert(!pthread_cond_wait(&changed, &log_gate));
  }
  if (!strcmp(kind, "detach_wait"))
    detach_waiting = 1;
  if (!strcmp(kind, "context_touch")) {
    context_entered = 1;
    assert(!pthread_cond_broadcast(&changed));
    while (!context_allowed)
      assert(!pthread_cond_wait(&changed, &log_gate));
  }
  assert(!pthread_cond_broadcast(&changed));
  assert(!pthread_mutex_unlock(&log_gate));
}
#define RINZU_NATIVE_LIFETIME_EVENT(kind, id, a, b) event(kind, id, a, b)
#include "rinzu/source_async.h"
typedef struct {
  unsigned magic;
} environment_t;
static void touch(void *raw) {
  environment_t *e = raw;
  assert(e->magic == 7183);
  event("context_touch", 0, 0, 0);
  assert(e->magic == 7183);
}
typedef struct {
  rinzu_native_completion_v3_t *carrier;
  uint64_t generation;
} publisher_t;
static void *publish(void *raw) {
  publisher_t *p = raw;
  producer = p->generation;
  event("publish_begin", 0, 0, 0);
  rinzu_native_completion_v2_publish(&p->carrier->completion);
  event("publish_end", 0, 0, 0);
  producer = 0;
  return NULL;
}
static void wait_exchange(unsigned g) {
  assert(!pthread_mutex_lock(&log_gate));
  while (!exchanged[g])
    assert(!pthread_cond_wait(&changed, &log_gate));
  assert(!pthread_mutex_unlock(&log_gate));
}
static void resume(unsigned g) {
  assert(!pthread_mutex_lock(&log_gate));
  allowed[g] = 1;
  assert(!pthread_cond_broadcast(&changed));
  assert(!pthread_mutex_unlock(&log_gate));
}
static rinzu_async_future_t *admit(rinzu_async_runtime_t *r, rinzu_native_completion_v3_t *c) {
  rinzu_async_future_t *f =
      rinzu_async_new(r, RINZU_ASYNC_COMPLETION, RINZU_SCALAR_I64, (rinzu_source_position_t){0});
  f->native_completion = &c->completion.base;
  rinzu_async_start(f);
  return f;
}
static void *release_context(void *unused) {
  (void)unused;
  assert(!pthread_mutex_lock(&log_gate));
  while (!detach_waiting)
    assert(!pthread_cond_wait(&changed, &log_gate));
  context_allowed = 1;
  assert(!pthread_cond_broadcast(&changed));
  assert(!pthread_mutex_unlock(&log_gate));
  return NULL;
}

static rinzu_native_completion_wake_t *capture(rinzu_native_completion_v2_t *carrier, uint64_t id) {
  producer = id;
  event("publish_begin", 0, 0, 0);
  return rinzu_native_completion_v2_publish_target(carrier);
}
static void release_publication(rinzu_native_completion_wake_t *target) {
  event("producer_release", 0, 0, 0);
  target->release(target);
  event("producer_released", 0, 0, 0);
  event("publish_end", 0, 0, 0);
  producer = 0;
}
static void coalesced(unsigned reverse, unsigned controlled) {
  printf("BEGIN 3 %u %u\n", reverse, controlled);
  allowed[0] = allowed[1] = context_allowed = 1;
  destroyed = 0;
  rinzu_async_runtime_t *runtime = calloc(1, sizeof(*runtime));
  environment_t *env = malloc(sizeof(*env));
  rinzu_native_completion_v3_t *scalar = calloc(1, sizeof(*scalar));
  rinzu_native_completion_v2_t *unit = calloc(1, sizeof(*unit));
  assert(runtime && env && scalar && unit);
  env->magic = 7183;
  rinzu_async_init(runtime);
  runtime->completion_wake = touch;
  runtime->wake_context = env;
  rinzu_native_completion_v2_init(&scalar->completion);
  scalar->completion.base.version = 3;
  scalar->completion.base.size = sizeof(*scalar);
  scalar->result = 7183;
  rinzu_native_completion_v2_init(unit);
  rinzu_native_completion_v2_t *carriers[] = {&scalar->completion, unit};
  rinzu_async_future_t *futures[2];
  for (unsigned g = 0; g < 2; ++g) {
    addresses[g] = (uintptr_t)carriers[g];
    generations[g] = 100 + g;
    event("generation", generations[g], g, 0);
    futures[g] =
        rinzu_async_new(runtime, RINZU_ASYNC_COMPLETION, g ? RINZU_TRANSFER_UNIT : RINZU_SCALAR_I64,
                        (rinzu_source_position_t){0});
    futures[g]->native_completion = &carriers[g]->base;
    rinzu_async_start(futures[g]);
  }
  if (controlled) {
    event("control_begin", futures[1]->identity, 1, 0);
    assert(rinzu_async_cancel(futures[1]));
    event("control_end", futures[1]->identity, 1, 0);
    assert(!rinzu_async_try_collect(futures[1]));
  }
  uint64_t epoch = runtime->completion_epoch;
  rinzu_native_completion_wake_t *leader = capture(carriers[0], 100);
  rinzu_native_completion_wake_t *duplicate = capture(carriers[1], 101);
  assert(leader && duplicate == leader);
  event("merge_wake", 101, 100, 0);
  release_publication(duplicate);
  assert(runtime->completion_epoch == epoch && !destroyed);
  producer = 100;
  leader->notify(leader);
  release_publication(leader);
  assert(runtime->completion_epoch == epoch + 1 && !destroyed);
  for (unsigned i = 0; i < 2; ++i) {
    unsigned g = reverse ? 1 - i : i;
    assert(rinzu_async_try_collect(futures[g]));
    assert(futures[g]->settled && (i || !futures[1 - g]->settled));
    assert((futures[g]->failure.primary != 0) == (controlled && g == 1));
    assert(futures[g]->result_slot == (g ? 0u : 7183u));
    event("generation_status", 100 + g,
          atomic_load_explicit(&carriers[g]->base.cancelled, memory_order_acquire) != 0,
          rinzu_native_completion_is_done(&carriers[g]->base));
    event("close_admission", 100 + g, 0, 0);
    free(g ? (void *)unit : (void *)scalar);
    event("carrier_free", 100 + g, 0, 0);
  }
  assert(futures[0]->result_slot == 7183);
  for (unsigned g = 0; g < 2; ++g)
    rinzu_async_release(futures[g]);
  event("shutdown", 0, 0, 0);
  rinzu_async_clear_completion_wake(runtime);
  rinzu_async_dispose(runtime);
  free(env);
  free(runtime);
  event("env_free", 0, 0, 0);
  assert(destroyed == 1);
  for (unsigned i = 0; i < RINZU_RESOURCE_COUNT; ++i)
    assert(!rinzu_resource_counters[i].live);
  puts("END");
}

int main(void) {
  const int64_t values[3] = {7183, 29, 65537};
  for (unsigned topology = 0; topology < 3; ++topology)
    for (unsigned scenario = 0; scenario < 6; ++scenario) {
      unsigned count = topology == 2 ? 3 : 2;
      printf("BEGIN %u %u\n", topology, scenario);
      memset(exchanged, 0, sizeof(exchanged));
      memset(allowed, 0, sizeof(allowed));
      destroyed = context_entered = detach_waiting = 0;
      context_allowed = scenario != 5;
      rinzu_async_runtime_t *r = calloc(1, sizeof(*r));
      environment_t *env = malloc(sizeof(*env));
      assert(r && env);
      env->magic = 7183;
      rinzu_async_init(r);
      r->completion_wake = touch;
      r->wake_context = env;
      rinzu_native_completion_v3_t *storage[2] = {calloc(1, sizeof(*storage[0])),
                                                  calloc(1, sizeof(*storage[1]))};
      assert(storage[0] && storage[1]);
      addresses[0] = (uintptr_t)storage[0];
      addresses[1] = (uintptr_t)storage[1];
      generations[0] = generations[1] = 0;
      rinzu_async_future_t *f[3] = {0};
      publisher_t p[3];
      pthread_t threads[3];
      for (unsigned g = 0; g < count; ++g) {
        unsigned replacement = (topology == 1 && g == 1) || (topology == 2 && g == 2);
        unsigned s = replacement || topology == 1 ? 0 : g;
        if (replacement) {
          assert(f[0]->settled);
          assert(atomic_load_explicit(&f[0]->physical_done, memory_order_acquire));
        }
        rinzu_native_completion_v3_t *c = storage[s];
        rinzu_native_completion_v2_init(&c->completion);
        c->completion.base.version = 3;
        c->completion.base.size = sizeof(*c);
        c->result = values[g];
        generations[s] = 100 + g;
        event(replacement ? "reuse" : "generation", 100 + g, replacement ? 100 : (int64_t)s,
              replacement ? 1 : 0);
        p[g] = (publisher_t){c, 100 + g};
        int early = scenario == 4 && g == count - 1;
        /* async_new admits before SDK publication; registration still loses to DONE. */
        if (early) {
          f[g] = rinzu_async_new(r, RINZU_ASYNC_COMPLETION, RINZU_SCALAR_I64,
                                 (rinzu_source_position_t){0});
          f[g]->native_completion = &c->completion.base;
          assert(!pthread_create(&threads[g], NULL, publish, &p[g]));
          assert(!pthread_join(threads[g], NULL));
          rinzu_async_start(f[g]);
        } else {
          f[g] = admit(r, c);
          if (scenario == 3 && g == count - 1) {
            event("control_begin", f[g]->identity, 1, 0);
            assert(rinzu_async_cancel(f[g]));
            event("control_end", f[g]->identity, 1, 0);
          }
          if (replacement) {
            assert(f[0]->result_slot == (uint64_t)values[0]);
            event("alias_result", f[0]->identity, values[0], 0);
            event("control_begin", f[0]->identity, 1, 0);
            assert(!rinzu_async_cancel(f[0]));
            event("control_end", f[0]->identity, 0, 0);
            assert(!atomic_load_explicit(&c->completion.base.cancelled, memory_order_acquire) ||
                   scenario == 3);
          }
          assert(!pthread_create(&threads[g], NULL, publish, &p[g]));
          wait_exchange(g);
        }
        if (replacement && early) {
          assert(f[0]->result_slot == (uint64_t)values[0]);
          event("alias_result", f[0]->identity, values[0], 0);
          event("control_begin", f[0]->identity, 1, 0);
          assert(!rinzu_async_cancel(f[0]));
          event("control_end", f[0]->identity, 0, 0);
        }
        rinzu_async_native_progress(r);
        assert(atomic_load_explicit(&f[g]->physical_done, memory_order_acquire));
        assert(f[g]->result_slot == (uint64_t)values[g]);
        assert(rinzu_async_try_collect(f[g]));
        event("generation_status", 100 + g,
              atomic_load_explicit(&c->completion.base.cancelled, memory_order_acquire) != 0,
              rinzu_native_completion_is_done(&c->completion.base));
        event("close_admission", 100 + g, 0, 0);
        event("carrier_free", 100 + g, 0, 0);
      }
      free(storage[0]);
      free(storage[1]);
      for (unsigned g = 0; g < count; ++g)
        rinzu_async_release(f[g]);
      pthread_t context_releaser;
      if (scenario == 5) {
        for (unsigned g = 0; g < count; ++g)
          resume(g);
        assert(!pthread_mutex_lock(&log_gate));
        while (!context_entered)
          assert(!pthread_cond_wait(&changed, &log_gate));
        assert(!pthread_mutex_unlock(&log_gate));
        assert(!pthread_create(&context_releaser, NULL, release_context, NULL));
      }
      event("shutdown", 0, 0, 0);
      rinzu_async_clear_completion_wake(r);
      rinzu_async_dispose(r);
      if (scenario == 5)
        assert(!pthread_join(context_releaser, NULL));
      free(env);
      free(r);
      event("env_free", 0, 0, 0);
      if (scenario != 5)
        assert(!destroyed);
      if (scenario == 2) {
        for (unsigned g = 0; g < count; ++g)
          resume(g);
      }
      for (unsigned i = 0; i < count; ++i) {
        unsigned g = scenario == 1 ? count - 1 - i : i;
        if (scenario == 4 && g == count - 1)
          continue;
        if (scenario != 2 && scenario != 5)
          resume(g);
        assert(!pthread_join(threads[g], NULL));
        if (scenario != 2 && scenario != 5 && i + 1 < count - (scenario == 4))
          assert(!destroyed);
      }
      assert(destroyed == 1);
      for (unsigned i = 0; i < RINZU_RESOURCE_COUNT; ++i)
        assert(!rinzu_resource_counters[i].live);
      puts("END");
    }
  for (unsigned reverse = 0; reverse < 2; ++reverse)
    for (unsigned controlled = 0; controlled < 2; ++controlled)
      coalesced(reverse, controlled);
}
