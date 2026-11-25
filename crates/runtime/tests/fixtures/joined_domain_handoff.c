#include <assert.h>
#include <inttypes.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static _Thread_local unsigned domain;
static _Thread_local int helper;
static _Atomic uintptr_t runtime_identity[2];
static uint64_t auxiliary_id[2] = {UINT64_MAX, UINT64_MAX};
static unsigned expect_auxiliary[2];
static pthread_mutex_t gate = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t changed = PTHREAD_COND_INITIALIZER;
static unsigned allow_scan[2], in_wake[2], allow_return[2], join_wait[2];

static void trace(const char *kind, uint64_t id, int64_t a, int64_t b) {
  flockfile(stdout);
  printf("%u,%s,%" PRIu64 ",%" PRId64 ",%" PRId64 "\n", domain, kind, id, a, b);
  funlockfile(stdout);
}

static void signal_gate(unsigned *flag) {
  assert(!pthread_mutex_lock(&gate));
  *flag = 1;
  assert(!pthread_cond_broadcast(&changed));
  assert(!pthread_mutex_unlock(&gate));
}

static void wait_for(unsigned *flag) {
  assert(!pthread_mutex_lock(&gate));
  while (!*flag)
    assert(!pthread_cond_wait(&changed, &gate));
  assert(!pthread_mutex_unlock(&gate));
}

static void helper_entry(const void *runtime) {
  unsigned d = (uintptr_t)runtime != atomic_load(&runtime_identity[0]);
  assert((uintptr_t)runtime == atomic_load(&runtime_identity[d]));
  domain = d;
  helper = 1;
  trace("helper_identity", 0, (intptr_t)runtime, 0);
  wait_for(&allow_scan[d]);
}

static void event(const char *kind, uint64_t id, int64_t a, int64_t b) {
  if (!strcmp(kind, "admit") && expect_auxiliary[domain]) {
    auxiliary_id[domain] = id;
    expect_auxiliary[domain] = 0;
  }
  if (!strcmp(kind, "carrier_bind")) {
    if (id == auxiliary_id[domain]) {
      trace("joined_auxiliary_identity", id, a, 0);
      trace("aux_bind", 0, 1, 0);
    } else {
      trace("joined_carrier_identity", id, a, 0);
    }
    return;
  }
  if (id && id == auxiliary_id[domain]) {
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
    else if (!strcmp(kind, "record_release"))
      kind = "aux_release";
    else
      assert(0 && "unknown polling receipt event");
    id = 0;
  }
  trace(kind, id, a, b);
  if (!strcmp(kind, "helper_join_wait"))
    signal_gate(&join_wait[domain]);
}

#define RINZU_NATIVE_LIFETIME_EVENT(kind, id, a, b) event(kind, id, a, b)
#define RINZU_NATIVE_HELPER_ENTER_EVENT(runtime) helper_entry(runtime)
#define RINZU_NATIVE_WAKE_BIND_EVENT(f, target)                                                    \
  trace("wake_bind", (f)->identity, (intptr_t)(target), 0)
#define RINZU_NATIVE_WAKE_FREE_EVENT(target) trace("target_identity_free", 0, (intptr_t)(target), 0)
#include "rinzu/source_async.h"

typedef struct {
  unsigned owner;
  unsigned value;
} context_t;

typedef struct {
  rinzu_async_runtime_t *runtime;
  context_t *context;
  rinzu_native_completion_t *polling;
  rinzu_async_future_t *future[2];
  rinzu_native_completion_wake_t *captured;
} owner_t;

static unsigned payload(unsigned d) {
  return 7183 + 97 * d;
}

static void touch(void *raw) {
  const context_t *context = raw;
  assert(helper && context->owner == domain && context->value == payload(domain));
  trace("context_identity", 0, (intptr_t)raw, 0);
  trace("helper_context", 0, 0, 0);
  signal_gate(&in_wake[domain]);
  wait_for(&allow_return[domain]);
  assert(context->owner == domain && context->value == payload(domain));
}

static rinzu_native_completion_wake_t *publish(rinzu_native_completion_v3_t *scalar) {
  trace("publish_begin", 0, 0, 0);
  rinzu_native_completion_wake_t *target =
      rinzu_native_completion_v2_publish_target(&scalar->completion);
  if (target)
    trace("joined_capture", 0, (intptr_t)target, 0);
  else
    trace("publish_end", 0, 0, 0);
  return target;
}

static rinzu_async_future_t *admit(owner_t *owner, rinzu_native_completion_t *carrier,
                                   unsigned kind) {
  rinzu_async_future_t *future =
      rinzu_async_new(owner->runtime, RINZU_ASYNC_COMPLETION, kind, (rinzu_source_position_t){0});
  future->native_completion = carrier;
  rinzu_async_start(future);
  return future;
}

static void begin(owner_t *owner, unsigned d, rinzu_native_completion_v3_t *scalar, int early) {
  domain = d;
  if (d)
    trace("joined_handoff", 0, (intptr_t)scalar, 0);
  owner->runtime = calloc(1, sizeof(*owner->runtime));
  owner->context = malloc(sizeof(*owner->context));
  owner->polling = calloc(1, sizeof(*owner->polling));
  assert(owner->runtime && owner->context && owner->polling);
  atomic_store(&runtime_identity[d], (uintptr_t)owner->runtime);
  *owner->context = (context_t){d, payload(d)};
  trace("domain_runtime", 0, (intptr_t)owner->runtime, 0);
  trace("domain_context", 0, (intptr_t)owner->context, 0);
  trace("joined_carrier", 0, (intptr_t)scalar, 3);
  trace("joined_auxiliary", 0, (intptr_t)owner->polling, 1);
  rinzu_async_init(owner->runtime);
  assert(!owner->runtime->direct_native_completion && owner->runtime->native_notifications);
  rinzu_async_set_completion_wake(owner->runtime, touch, owner->context);
  rinzu_native_completion_v2_init(&scalar->completion);
  scalar->completion.base.version = 3;
  scalar->completion.base.size = sizeof(*scalar);
  scalar->result = payload(d);
  if (early)
    assert(!publish(scalar));
  owner->future[0] = admit(owner, &scalar->completion.base, RINZU_SCALAR_I64);
  assert(!owner->runtime->reactor_started);
  rinzu_native_completion_init(owner->polling);
  expect_auxiliary[d] = 1;
  owner->future[1] = admit(owner, owner->polling, RINZU_TRANSFER_UNIT);
  assert(owner->runtime->reactor_started && !owner->future[1]->native_notified);
  if (!early) {
    owner->captured = publish(scalar);
    assert(owner->captured);
  }
  atomic_store_explicit(&owner->polling->done, 1, memory_order_release);
  trace("aux_publish", 0, 0, 0);
  signal_gate(&allow_scan[d]);
  wait_for(&in_wake[d]);
}

static void collect(owner_t *owner, unsigned g, const rinzu_native_completion_v3_t *scalar) {
  assert(owner->future[g]->result_slot == (g ? 0 : payload(domain)));
  if (!g)
    trace("close_admission", 0, 0, 0);
  assert(rinzu_async_try_collect(owner->future[g]));
  if (g)
    trace("aux_retire", 0, 0, 0);
  else
    trace("joined_carrier_retire", 0, (intptr_t)scalar, 0);
}

static void *release_helper(void *raw) {
  unsigned d = (unsigned)(uintptr_t)raw;
  wait_for(&join_wait[d]);
  signal_gate(&allow_return[d]);
  return NULL;
}

static void finish(owner_t owners[2], unsigned d) {
  unsigned other = 1 - d;
  domain = other;
  uint64_t peer_epoch =
      owners[other].runtime ? rinzu_async_completion_epoch(owners[other].runtime) : 0;
  domain = d;
  owner_t *owner = &owners[d];
  assert(owner->future[0]->result_slot == payload(d));
  for (unsigned g = 0; g < 2; ++g)
    rinzu_async_release(owner->future[g]);
  rinzu_async_native_wake_t *target = owner->runtime->native_wake;
  size_t references = atomic_load_explicit(&target->references, memory_order_acquire);
  assert(references == (owner->captured ? 2u : 1u));
  trace("runtime_refs", 0, (intptr_t)&target->header, (int64_t)references);
  pthread_t releaser;
  assert(!pthread_create(&releaser, NULL, release_helper, (void *)(uintptr_t)d));
  trace("shutdown", 0, 0, 0);
  rinzu_async_dispose(owner->runtime);
  assert(!pthread_join(releaser, NULL));
  intptr_t context = (intptr_t)owner->context;
  free(owner->context);
  free(owner->runtime);
  owner->runtime = NULL;
  trace("environment_identity_free", 0, context, 0);
  trace("env_free", 0, 0, 0);
  if (owner->captured) {
    owner->captured->notify(owner->captured);
    assert(atomic_load_explicit(&target->references, memory_order_acquire) == 1);
    trace("joined_producer_refs", 0, (intptr_t)owner->captured, 1);
    trace("producer_release", 0, 0, 0);
    owner->captured->release(owner->captured);
    trace("producer_released", 0, 0, 0);
    trace("publish_end", 0, 0, 0);
  }
  domain = other;
  if (owners[other].runtime)
    assert(peer_epoch == rinzu_async_completion_epoch(owners[other].runtime));
}

int main(int argc, char **argv) {
  setvbuf(stdout, NULL, _IOLBF, 0);
  alarm(30);
  assert(argc == 4 && strlen(argv[1]) == 4);
  unsigned first = (unsigned)(argv[2][0] - '0');
  unsigned early = (unsigned)(argv[3][0] - '0');
  assert(first < 2 && early < 2);
  owner_t owners[2] = {0};
  rinzu_native_completion_v3_t *scalar = calloc(1, sizeof(*scalar));
  assert(scalar);
  begin(&owners[0], 0, scalar, 0);
  unsigned settled = 0;
  for (unsigned i = 0; i < 4; ++i) {
    unsigned receipt = (unsigned)(argv[1][i] - '0');
    assert(receipt < 4 && !(settled & (1u << receipt)));
    unsigned d = receipt / 2;
    domain = d;
    assert(owners[d].runtime);
    collect(&owners[d], receipt % 2, scalar);
    if (!receipt)
      begin(&owners[1], 1, scalar, early != 0);
    settled |= 1u << receipt;
  }
  finish(owners, first);
  finish(owners, 1 - first);
  domain = 1;
  uintptr_t slot = atomic_load(&scalar->completion.notification);
  unsigned cancelled = atomic_load(&scalar->completion.base.cancelled);
  assert(scalar->result == payload(1) && slot == RINZU_NATIVE_COMPLETION_DONE && !cancelled);
  trace("joined_final_slot", 0, (intptr_t)slot, cancelled);
  free(scalar);
  free(owners[0].polling);
  free(owners[1].polling);
  for (unsigned i = 0; i < RINZU_RESOURCE_COUNT; ++i)
    assert(!rinzu_resource_counters[i].live);
  return 0;
}
