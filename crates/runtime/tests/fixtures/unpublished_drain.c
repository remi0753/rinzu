#include <assert.h>
#include <pthread.h>
#include <sched.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

static pthread_cond_t *owner_changed;
static pthread_t owner_thread;
static _Atomic int owner_waiting, helper_joined, detached, freed, provider_wait_confirmed;
static unsigned settled_decisions[3];

static int observe_wait(pthread_cond_t *condition, pthread_mutex_t *gate) {
  if (condition == owner_changed && pthread_equal(pthread_self(), owner_thread))
    atomic_store_explicit(&owner_waiting, 1, memory_order_release);
  return pthread_cond_wait(condition, gate);
}
static void event(const char *kind, uint64_t id, int64_t a, int64_t b) {
  (void)id;
  (void)a;
  (void)b;
  if (!strcmp(kind, "helper_joined"))
    atomic_store(&helper_joined, 1);
  if (!strcmp(kind, "detach"))
    atomic_store(&detached, 1);
  if (!strcmp(kind, "record_release"))
    atomic_fetch_add(&freed, 1);
}
static void observe_decision(uint64_t identity, unsigned winner) {
  assert(identity < 3);
  settled_decisions[identity] = winner;
}
#define pthread_cond_wait observe_wait
#define RINZU_NATIVE_LIFETIME_EVENT(kind, id, a, b) event(kind, id, a, b)
#define RINZU_NATIVE_DECISION_EVENT(future, winner) observe_decision((future)->identity, winner)
#include "rinzu/source_async.h"
#undef pthread_cond_wait

typedef struct {
  rinzu_async_future_t *future[3];
  rinzu_native_completion_v3_t notified;
  rinzu_native_completion_t polling;
  rinzu_native_completion_wake_t *captured;
} state_t;

static void *provider(void *raw) {
  state_t *state = raw;
  while (!atomic_load_explicit(&owner_waiting, memory_order_acquire))
    sched_yield();
  assert(!atomic_load(&helper_joined) && !atomic_load(&detached) && !atomic_load(&freed));
  for (unsigned i = 0; i < 3; ++i)
    assert(!atomic_load(&state->future[i]->physical_done));
  assert(atomic_load(&state->notified.completion.base.cancelled));
  assert(atomic_load(&state->polling.cancelled));
  assert(!rinzu_native_completion_is_done(&state->notified.completion.base));
  assert(!rinzu_native_completion_is_done(&state->polling));
  atomic_store(&provider_wait_confirmed, 1);

  state->captured = rinzu_native_completion_v2_publish_target(&state->notified.completion);
  assert(state->captured);
  state->captured->notify(state->captured);
  atomic_store_explicit(&state->polling.done, 1, memory_order_release);
  return NULL;
}

static rinzu_async_future_t *admit(rinzu_async_runtime_t *runtime,
                                   rinzu_native_completion_t *carrier, unsigned kind) {
  rinzu_async_future_t *future =
      rinzu_async_new(runtime, RINZU_ASYNC_COMPLETION, kind, (rinzu_source_position_t){0});
  future->native_completion = carrier;
  rinzu_async_start(future);
  return future;
}

int main(void) {
  rinzu_async_runtime_t runtime;
  rinzu_async_init(&runtime);
  owner_thread = pthread_self();
  owner_changed = &runtime.completion_changed;
  state_t state;
  rinzu_native_completion_v2_init(&state.notified.completion);
  state.notified.completion.base.version = 3;
  state.notified.completion.base.size = sizeof(state.notified);
  state.notified.result = 4456;
  rinzu_native_completion_init(&state.polling);
  for (unsigned i = 0; i < 2; ++i)
    state.future[i] = admit(&runtime, &state.notified.completion.base, RINZU_SCALAR_I64);
  state.future[2] = admit(&runtime, &state.polling, RINZU_TRANSFER_UNIT);
  assert(runtime.reactor_started);
  assert(rinzu_async_cancel(state.future[0]));
  assert(rinzu_async_timeout(state.future[1]));
  assert(rinzu_async_cancel(state.future[2]));
  pthread_t sdk;
  assert(!pthread_create(&sdk, NULL, provider, &state));

  /* The Owner's ordinary wait path drives only real physical collection. */
  for (unsigned i = 0; i < 3; ++i) {
    rinzu_async_future_t *future = state.future[i];
    while (!rinzu_async_try_collect(future)) {
      uint64_t observed = rinzu_async_completion_epoch(&runtime);
      if (rinzu_async_try_collect(future))
        break;
      rinzu_async_wait_progress(&runtime, observed);
    }
    assert(future->settled && future->failure.primary == 2);
    assert(atomic_load(&future->terminal) == RINZU_ASYNC_SETTLED);
    assert(settled_decisions[future->identity] ==
           (i == 1 ? RINZU_ASYNC_TIMED_OUT : RINZU_ASYNC_CANCELLED));
  }
  assert(!pthread_join(sdk, NULL));
  assert(atomic_load(&provider_wait_confirmed));
  for (unsigned i = 0; i < 3; ++i)
    rinzu_async_release(state.future[i]);
  assert(atomic_load(&freed) == 3);
  rinzu_async_dispose(&runtime);
  assert(atomic_load(&helper_joined) && atomic_load(&detached));
  state.captured->notify(state.captured);
  state.captured->release(state.captured);
  puts("UNPUBLISHED DRAIN C waits for SDK v1/v3 publication");
  return 0;
}
