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
static unsigned auxiliary_next, scenario, exchanged, claim_seen, helper_touched, native_touched,
    join_wait, detach_wait;
static int allow_claim, allow_helper, allow_native, allow_notify;
static uint64_t auxiliary_id, first_id;
static _Thread_local int sdk_thread, helper_thread;
static void event(const char *kind, uint64_t id, int64_t a, int64_t b) {
  assert(!pthread_mutex_lock(&log_gate));
  if (!strcmp(kind, "admit")) {
    if (auxiliary_next) {
      auxiliary_id = id;
      auxiliary_next = 0;
    } else if (first_id == UINT64_MAX)
      first_id = id;
  }
  if (!strcmp(kind, "carrier_bind")) {
    if (id != auxiliary_id) {
      assert(!pthread_mutex_unlock(&log_gate));
      return;
    }
    kind = "aux_bind";
    id = 0;
    a = 1;
  } else if (id == auxiliary_id && auxiliary_id &&
             (!strcmp(kind, "admit") || !strcmp(kind, "copied") ||
              !strcmp(kind, "completion_claim") || !strcmp(kind, "physical") ||
              !strcmp(kind, "owner_settle") || !strcmp(kind, "record_release"))) {
    if (!strcmp(kind, "admit"))
      kind = "aux_admit";
    else if (!strcmp(kind, "copied"))
      kind = "aux_copied";
    else if (!strcmp(kind, "completion_claim"))
      kind = "aux_claim";
    else if (!strcmp(kind, "physical"))
      kind = "aux_physical";
    else if (!strcmp(kind, "owner_settle"))
      kind = "aux_settle";
    else
      kind = "aux_release";
    id = 0;
  }
  if (!strcmp(kind, "helper_wake_begin"))
    helper_thread = 1;
  printf("%s,%" PRIu64 ",%" PRId64 ",%" PRId64 "\n", kind, id, a, b);
  if (!strcmp(kind, "sdk_exchange")) {
    exchanged = 1;
    assert(!pthread_cond_broadcast(&changed));
    while (!allow_notify)
      assert(!pthread_cond_wait(&changed, &log_gate));
  }
  if (!strcmp(kind, "completion_claim") && id == first_id) {
    claim_seen = 1;
    assert(!pthread_cond_broadcast(&changed));
    while (!allow_claim)
      assert(!pthread_cond_wait(&changed, &log_gate));
  }
  if (!strcmp(kind, "helper_context")) {
    helper_touched = 1;
    assert(!pthread_cond_broadcast(&changed));
    while (!allow_helper)
      assert(!pthread_cond_wait(&changed, &log_gate));
  }
  if (!strcmp(kind, "native_context")) {
    native_touched = 1;
    assert(!pthread_cond_broadcast(&changed));
    while (!allow_native)
      assert(!pthread_cond_wait(&changed, &log_gate));
  }
  if (!strcmp(kind, "helper_join_wait"))
    join_wait = 1;
  if (!strcmp(kind, "detach_wait"))
    detach_wait = 1;
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
  assert(sdk_thread || helper_thread);
  event(sdk_thread ? "native_context" : "helper_context", 0, 0, 0);
  assert(e->magic == 7183);
}
typedef struct {
  rinzu_native_completion_v3_t *carrier;
} publisher_t;
static void *publish(void *raw) {
  publisher_t *p = raw;
  sdk_thread = 1;
  event("publish_begin", 0, 0, 0);
  rinzu_native_completion_v2_publish(&p->carrier->completion);
  event("publish_end", 0, 0, 0);
  return NULL;
}
static void wait_for(unsigned *value) {
  assert(!pthread_mutex_lock(&log_gate));
  while (!*value)
    assert(!pthread_cond_wait(&changed, &log_gate));
  assert(!pthread_mutex_unlock(&log_gate));
}
static void allow(int *value) {
  assert(!pthread_mutex_lock(&log_gate));
  *value = 1;
  assert(!pthread_cond_broadcast(&changed));
  assert(!pthread_mutex_unlock(&log_gate));
}
static void *release_readers(void *unused) {
  (void)unused;
  wait_for(&join_wait);
  allow(&allow_helper);
  if (scenario == 4) {
    wait_for(&detach_wait);
    allow(&allow_native);
  }
  return NULL;
}
static void control(rinzu_async_future_t *f, int kind, int expected) {
  event("control_begin", f->identity, kind, 0);
  int accepted = kind == 1 ? rinzu_async_cancel(f) : rinzu_async_timeout(f);
  event("control_end", f->identity, accepted, 0);
  assert(accepted == expected);
}
int main(void) {
  for (unsigned count = 1; count <= 2; ++count)
    for (scenario = 0; scenario < 5; ++scenario) {
      printf("BEGIN %u %u\n", count, scenario);
      auxiliary_next = exchanged = claim_seen = helper_touched = native_touched = join_wait =
          detach_wait = 0;
      auxiliary_id = first_id = UINT64_MAX;
      allow_claim = scenario != 1 && scenario != 2;
      allow_helper = 0;
      allow_native = scenario != 4;
      allow_notify = 0;
      rinzu_async_runtime_t *r = calloc(1, sizeof(*r));
      environment_t *e = malloc(sizeof(*e));
      assert(r && e);
      e->magic = 7183;
      rinzu_async_init(r);
      r->completion_wake = touch;
      r->wake_context = e;
      assert(!r->direct_native_completion && r->native_notifications);
      r->pending_operation_limit = 0;
      rinzu_native_completion_v3_t *carrier = calloc(1, sizeof(*carrier));
      rinzu_native_completion_t *aux = calloc(1, sizeof(*aux));
      assert(carrier && aux);
      rinzu_native_completion_v2_init(&carrier->completion);
      carrier->completion.base.version = 3;
      carrier->completion.base.size = sizeof(*carrier);
      carrier->result = 7183;
      rinzu_native_completion_init(aux);
      rinzu_async_future_t *f[2] = {0};
      for (unsigned i = 0; i < count; ++i) {
        f[i] = rinzu_async_new(r, RINZU_ASYNC_COMPLETION, RINZU_SCALAR_I64,
                               (rinzu_source_position_t){0});
        f[i]->native_completion = &carrier->completion.base;
        rinzu_async_start(f[i]);
        assert(!r->reactor_started);
      }
      auxiliary_next = 1;
      rinzu_async_future_t *poll = rinzu_async_new(r, RINZU_ASYNC_COMPLETION, RINZU_TRANSFER_UNIT,
                                                   (rinzu_source_position_t){0});
      poll->native_completion = aux;
      rinzu_async_start(poll);
      assert(r->reactor_started && !poll->native_notified);
      if (scenario == 3)
        control(f[0], 1, 1);
      /* Both publications happen while the real reactor cannot scan. Its single
         batch contains the v1 polling receipt and every v3 receipt. */
      rinzu_async_lock(r);
      publisher_t p = {carrier};
      pthread_t producer;
      assert(!pthread_create(&producer, NULL, publish, &p));
      wait_for(&exchanged);
      atomic_store_explicit(&aux->done, 1, memory_order_release);
      event("aux_publish", 0, 0, 0);
      pthread_cond_broadcast(&r->completion_changed);
      rinzu_async_unlock(r);
      if (scenario == 1 || scenario == 2) {
        wait_for(&claim_seen);
        assert(!atomic_load_explicit(&f[0]->physical_done, memory_order_acquire));
        control(f[0], scenario == 1 ? 1 : 2, 0);
        assert(!atomic_load_explicit(&carrier->completion.base.cancelled, memory_order_acquire));
        assert(e->magic == 7183);
        allow(&allow_claim);
      }
      wait_for(&helper_touched);
      if (scenario == 4) {
        allow(&allow_notify);
        wait_for(&native_touched);
      }
      event("close_admission", 0, 0, 0);
      for (unsigned i = 0; i < count; ++i) {
        assert(atomic_load_explicit(&f[i]->physical_done, memory_order_acquire));
        assert(f[i]->result_slot == 7183);
        assert(rinzu_async_try_collect(f[i]));
      }
      assert(rinzu_async_try_collect(poll));
      free(aux);
      event("aux_retire", 0, 0, 0);
      free(carrier);
      event("carrier_free", 0, 0, 0);
      for (unsigned i = 0; i < count; ++i)
        rinzu_async_release(f[i]);
      rinzu_async_release(poll);
      pthread_t releaser;
      assert(!pthread_create(&releaser, NULL, release_readers, NULL));
      event("shutdown", 0, 0, 0);
      rinzu_async_clear_completion_wake(r);
      rinzu_async_dispose(r);
      assert(!pthread_join(releaser, NULL));
      free(e);
      free(r);
      event("env_free", 0, 0, 0);
      if (scenario != 4)
        allow(&allow_notify);
      assert(!pthread_join(producer, NULL));
      for (unsigned i = 0; i < RINZU_RESOURCE_COUNT; ++i)
        assert(!rinzu_resource_counters[i].live);
      puts("END");
    }
}
