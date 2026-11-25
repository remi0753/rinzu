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
static unsigned published, touched, detach_waiting;
static int notify_allowed, context_allowed;
static void event(const char *kind, uint64_t id, int64_t a, int64_t b) {
  if (!strcmp(kind, "carrier_bind"))
    return;
  assert(!pthread_mutex_lock(&log_gate));
  printf("%s,%" PRIu64 ",%" PRId64 ",%" PRId64 "\n", kind, id, a, b);
  if (!strcmp(kind, "publish_end"))
    ++published;
  if (!strcmp(kind, "context_touch"))
    ++touched;
  if (!strcmp(kind, "detach_wait"))
    ++detach_waiting;
  assert(!pthread_cond_broadcast(&changed));
  assert(!pthread_mutex_unlock(&log_gate));
}
#define RINZU_NATIVE_LIFETIME_EVENT(kind, id, a, b) event(kind, id, a, b)
#include "rinzu/source_async.h"

typedef struct {
  unsigned magic;
} environment_t;
static void wake_context(void *raw) {
  environment_t *context = raw;
  assert(context->magic == 7183);
  event("context_touch", 0, 0, 0);
  assert(!pthread_mutex_lock(&log_gate));
  while (!context_allowed)
    assert(!pthread_cond_wait(&changed, &log_gate));
  assert(!pthread_mutex_unlock(&log_gate));
  assert(context->magic == 7183);
}
typedef struct {
  rinzu_native_completion_v3_t *carrier;
} publisher_t;
static void *publish(void *raw) {
  publisher_t *p = raw;
  event("publish_begin", 0, 0, 0);
  rinzu_native_completion_wake_t *target =
      rinzu_native_completion_v2_publish_target(&p->carrier->completion);
  event("publish_end", 0, 0, 0);
  assert(target);
  assert(!pthread_mutex_lock(&log_gate));
  while (!notify_allowed)
    assert(!pthread_cond_wait(&changed, &log_gate));
  assert(!pthread_mutex_unlock(&log_gate));
  target->notify(target);
  event("producer_release", 0, 0, 0);
  target->release(target);
  event("producer_released", 0, 0, 0);
  return NULL;
}
static void *unblock_context(void *unused) {
  (void)unused;
  assert(!pthread_mutex_lock(&log_gate));
  while (!detach_waiting)
    assert(!pthread_cond_wait(&changed, &log_gate));
  context_allowed = 1;
  assert(!pthread_cond_broadcast(&changed));
  assert(!pthread_mutex_unlock(&log_gate));
  return NULL;
}
static void wait_for(unsigned *counter) {
  assert(!pthread_mutex_lock(&log_gate));
  while (!*counter)
    assert(!pthread_cond_wait(&changed, &log_gate));
  assert(!pthread_mutex_unlock(&log_gate));
}
static rinzu_async_future_t *admit(rinzu_async_runtime_t *r, rinzu_native_completion_v3_t *c) {
  rinzu_async_future_t *f =
      rinzu_async_new(r, RINZU_ASYNC_COMPLETION, RINZU_SCALAR_I64, (rinzu_source_position_t){0});
  f->native_completion = &c->completion.base;
  rinzu_async_start(f);
  return f;
}
static void control(rinzu_async_future_t *f, int kind, int expected) {
  event("control_begin", f->identity, kind, 0);
  int accepted = kind == 1 ? rinzu_async_cancel(f) : rinzu_async_timeout(f);
  event("control_end", f->identity, accepted, 0);
  assert(accepted == expected);
}
int main(void) {
  for (unsigned count = 1; count <= 2; ++count)
    for (unsigned scenario = 0; scenario < 3; ++scenario) {
      printf("BEGIN %u %u\n", count, scenario);
      published = touched = detach_waiting = 0;
      notify_allowed = scenario != 1;
      context_allowed = scenario != 2;
      rinzu_async_runtime_t *runtime = calloc(1, sizeof(*runtime));
      environment_t *context = malloc(sizeof(*context));
      rinzu_native_completion_v3_t *carrier = calloc(1, sizeof(*carrier));
      assert(runtime && context && carrier);
      context->magic = 7183;
      rinzu_async_init(runtime);
      runtime->completion_wake = wake_context;
      runtime->wake_context = context;
      rinzu_native_completion_v2_init(&carrier->completion);
      carrier->completion.base.version = 3;
      carrier->completion.base.size = sizeof(*carrier);
      carrier->result = 7183;
      rinzu_async_future_t *f[2] = {admit(runtime, carrier), NULL};
      if (scenario)
        control(f[0], scenario == 1 ? 1 : 2, 1);
      publisher_t publisher = {carrier};
      pthread_t producer;
      assert(!pthread_create(&producer, NULL, publish, &publisher));
      wait_for(&published);
      if (count == 2)
        f[1] = admit(runtime, carrier);
      event("close_admission", 0, 0, 0);
      rinzu_async_native_progress(runtime);
      for (unsigned i = 0; i < count; ++i)
        assert(rinzu_async_try_collect(f[i]));
      if (!scenario) {
        assert(!pthread_join(producer, NULL));
        control(f[0], 1, 0);
      }
      if (scenario == 2)
        wait_for(&touched);
      free(carrier);
      event("carrier_free", 0, 0, 0);
      for (unsigned i = 0; i < count; ++i)
        rinzu_async_release(f[i]);
      pthread_t controller;
      if (scenario == 2)
        assert(!pthread_create(&controller, NULL, unblock_context, NULL));
      event("shutdown", 0, 0, 0);
      rinzu_async_clear_completion_wake(runtime);
      rinzu_async_dispose(runtime);
      free(context);
      free(runtime);
      event("env_free", 0, 0, 0);
      if (scenario == 1) {
        assert(!pthread_mutex_lock(&log_gate));
        notify_allowed = 1;
        assert(!pthread_cond_broadcast(&changed));
        assert(!pthread_mutex_unlock(&log_gate));
      }
      if (scenario == 2)
        assert(!pthread_join(controller, NULL));
      if (scenario)
        assert(!pthread_join(producer, NULL));
      for (unsigned i = 0; i < RINZU_RESOURCE_COUNT; ++i)
        assert(!rinzu_resource_counters[i].live);
      puts("END");
    }
}
