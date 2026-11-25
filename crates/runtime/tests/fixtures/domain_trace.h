#ifndef RINZU_TEST_DOMAIN_TRACE_H
#define RINZU_TEST_DOMAIN_TRACE_H

#include <assert.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

static _Thread_local unsigned domain;
static _Thread_local uint64_t producer;
static uintptr_t carriers[3][3];

static void trace(const char *kind, uint64_t id, int64_t a, int64_t b) {
  flockfile(stdout);
  printf("%u,%s,%" PRIu64 ",%" PRId64 ",%" PRId64 "\n", domain, kind, id, a, b);
  funlockfile(stdout);
}

static void event(const char *kind, uint64_t id, int64_t a, int64_t b) {
  if (!strcmp(kind, "carrier_bind")) {
    unsigned member = 0;
    while (member < 3 && (uintptr_t)a != carriers[domain][member])
      ++member;
    assert(member < 3 && a);
    trace("receipt_carrier", id, a, 0);
    trace("receipt", id, 100 + member, 0);
    return;
  }
  if (producer && (!strcmp(kind, "sdk_prefix") || !strcmp(kind, "sdk_exchange") ||
                   !strcmp(kind, "notify_enter") || !strcmp(kind, "notify_ready") ||
                   !strcmp(kind, "notify_leave")))
    id = producer;
  trace(kind, id, a, b);
}

#define RINZU_NATIVE_LIFETIME_EVENT(kind, id, a, b) event(kind, id, a, b)
#define RINZU_NATIVE_CONTROL_EVENT(kind, future, value) trace(kind, (future)->identity, value, 0)
#define RINZU_NATIVE_WAKE_BIND_EVENT(f, target)                                                    \
  trace("wake_bind", (f)->identity, (intptr_t)(target), 0)
#define RINZU_NATIVE_WAKE_FREE_EVENT(target) trace("target_identity_free", 0, (intptr_t)(target), 0)
#include "rinzu/source_async.h"

static size_t refs(rinzu_native_completion_wake_t *target) {
  return atomic_load_explicit(&((rinzu_async_native_wake_t *)target)->references,
                              memory_order_acquire);
}

static rinzu_async_future_t *start(rinzu_async_runtime_t *runtime,
                                   rinzu_native_completion_t *carrier, unsigned kind) {
  rinzu_async_future_t *future =
      rinzu_async_new(runtime, RINZU_ASYNC_COMPLETION, kind, (rinzu_source_position_t){0});
  future->native_completion = carrier;
  rinzu_async_start(future);
  return future;
}

static rinzu_native_completion_wake_t *capture_target(rinzu_native_completion_v2_t *carrier,
                                                      unsigned g) {
  producer = 100 + g;
  trace("publish_begin", producer, 0, 0);
  rinzu_native_completion_wake_t *target = rinzu_native_completion_v2_publish_target(carrier);
  assert(target);
  trace("batch_target", producer, (intptr_t)target, 0);
  producer = 0;
  return target;
}

static void return_reference(unsigned g, rinzu_native_completion_wake_t *target) {
  trace("producer_refs", 100 + g, (intptr_t)target, (int64_t)refs(target));
  trace("producer_release", 100 + g, 0, 0);
  target->release(target);
  trace("producer_released", 100 + g, 0, 0);
  trace("publish_end", 100 + g, 0, 0);
}

#endif
