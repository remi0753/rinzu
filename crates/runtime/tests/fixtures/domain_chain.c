#include "domain_trace.h"
#include <stdlib.h>
#include <unistd.h>

typedef struct {
  unsigned owner;
  unsigned notifications;
} context_t;

typedef struct {
  rinzu_async_runtime_t *runtime;
  context_t *context;
  rinzu_native_completion_v3_t *storage[3];
  rinzu_async_future_t *future[3];
  rinzu_native_completion_wake_t *target[2];
  unsigned members;
} chain_domain_t;

static unsigned value(unsigned d, unsigned g) {
  return 7183 + 97 * d + g;
}

static void touch(void *raw) {
  context_t *context = raw;
  assert(context->owner == domain && context->notifications < 2);
  ++context->notifications;
  trace("context_identity", 0, (intptr_t)raw, 0);
  trace("context_touch", producer, 0, 0);
}

static void init(chain_domain_t *owner, unsigned d, rinzu_native_completion_v3_t *scalar,
                 unsigned members) {
  domain = d;
  if (d)
    trace("chain_handoff", 100, (intptr_t)scalar, 0);
  owner->runtime = calloc(1, sizeof(*owner->runtime));
  owner->context = malloc(sizeof(*owner->context));
  owner->members = members;
  owner->storage[0] = scalar;
  assert(owner->runtime && owner->context);
  *owner->context = (context_t){d, 0};
  trace("domain_context", 0, (intptr_t)owner->context, 0);
  rinzu_async_init(owner->runtime);
  rinzu_async_set_completion_wake(owner->runtime, touch, owner->context);
  for (unsigned g = 0; g < members; ++g) {
    if (g)
      owner->storage[g] = calloc(1, sizeof(*scalar));
    rinzu_native_completion_v3_t *carrier = owner->storage[g];
    assert(carrier);
    rinzu_native_completion_v2_init(&carrier->completion);
    if (g != 2) {
      carrier->completion.base.version = 3;
      carrier->completion.base.size = sizeof(*carrier);
      carrier->result = value(d, g);
    }
    carriers[d][g] = (uintptr_t)carrier;
    trace("generation", 100 + g, g, 0);
    trace("carrier_address", 100 + g, (intptr_t)carrier, carrier->completion.base.version);
  }
}

static void admit(chain_domain_t *owner, unsigned g, int cancel) {
  rinzu_native_completion_v2_t *carrier = &owner->storage[g]->completion;
  owner->future[g] =
      start(owner->runtime, &carrier->base, g == 2 ? RINZU_TRANSFER_UNIT : RINZU_SCALAR_I64);
  if (cancel) {
    assert(rinzu_async_cancel(owner->future[g]));
    assert(!rinzu_async_try_collect(owner->future[g]));
  }
}

static void publish_first(chain_domain_t *owner) {
  admit(owner, 0, 0);
  owner->target[0] = capture_target(&owner->storage[0]->completion, 0);
  assert(refs(owner->target[0]) == 2);
  trace("chain_refs", 0, (intptr_t)owner->target[0], (int64_t)refs(owner->target[0]));
  rinzu_async_native_progress(owner->runtime);
}

static void publish_following(chain_domain_t *owner, int cancel) {
  admit(owner, 1, cancel && owner->members == 2);
  if (owner->members == 3)
    admit(owner, 2, cancel);
  owner->target[1] = capture_target(&owner->storage[1]->completion, 1);
  assert(owner->target[1] == owner->target[0]);
  if (owner->members == 3) {
    rinzu_native_completion_wake_t *duplicate = capture_target(&owner->storage[2]->completion, 2);
    assert(duplicate == owner->target[1] && refs(duplicate) == 4);
    trace("merge_wake", 102, 101, 0);
    return_reference(2, duplicate);
  }
  assert(refs(owner->target[1]) == 3);
  trace("chain_refs", 0, (intptr_t)owner->target[1], (int64_t)refs(owner->target[1]));
  rinzu_async_native_progress(owner->runtime);
}

static void settle(chain_domain_t *owner, unsigned g, int cancelled) {
  rinzu_async_future_t *future = owner->future[g];
  assert(rinzu_async_try_collect(future));
  assert(future->settled && (future->failure.primary != 0) == cancelled);
  if (!cancelled)
    assert(future->result_slot == (g == 2 ? 0 : value(domain, g)));
  const rinzu_native_completion_t *carrier = &owner->storage[g]->completion.base;
  trace("generation_status", 100 + g, atomic_load(&carrier->cancelled) != 0,
        rinzu_native_completion_is_done(carrier));
  trace("close_admission", 100 + g, 0, 0);
  trace("carrier_retire", 100 + g, (intptr_t)carrier, 0);
}

static void notify_both(const chain_domain_t *owner) {
  for (unsigned g = 0; g < 2; ++g) {
    producer = 100 + g;
    owner->target[g]->notify(owner->target[g]);
  }
  producer = 0;
}

static void finish(chain_domain_t owners[3], unsigned domain_count, unsigned d, unsigned seed) {
  uint64_t epochs[3] = {0};
  for (unsigned peer = 0; peer < domain_count; ++peer) {
    domain = peer;
    if (peer != d && owners[peer].runtime)
      epochs[peer] = rinzu_async_completion_epoch(owners[peer].runtime);
  }
  domain = d;
  chain_domain_t *owner = &owners[d];
  int detached = (seed + d) % 2 != 0;
  if (!detached)
    notify_both(owner);
  assert(owner->context->notifications == (detached ? 0u : 2u));
  assert(owner->future[0]->result_slot == value(d, 0));
  for (unsigned g = 0; g < owner->members; ++g)
    rinzu_async_release(owner->future[g]);
  assert(owner->target[0] == owner->target[1] && refs(owner->target[0]) == 3);
  trace("runtime_refs", 0, (intptr_t)owner->target[0], (int64_t)refs(owner->target[0]));
  trace("shutdown", 0, 0, 0);
  rinzu_async_dispose(owner->runtime);
  intptr_t context = (intptr_t)owner->context;
  free(owner->context);
  free(owner->runtime);
  owner->runtime = NULL;
  trace("environment_identity_free", 0, context, 0);
  trace("env_free", 0, 0, 0);
  assert(refs(owner->target[0]) == 2);
  if (detached)
    notify_both(owner);
  unsigned first = (seed / 2 + d) % 2;
  return_reference(first, owner->target[first]);
  assert(refs(owner->target[1 - first]) == 1);
  return_reference(1 - first, owner->target[1 - first]);
  for (unsigned peer = 0; peer < domain_count; ++peer) {
    domain = peer;
    if (peer != d && owners[peer].runtime)
      assert(epochs[peer] == rinzu_async_completion_epoch(owners[peer].runtime));
  }
}

int main(int argc, char **argv) {
  setvbuf(stdout, NULL, _IOLBF, 0);
  alarm(30);
  assert(argc == 5 && strlen(argv[1]) == 6);
  unsigned merged = (unsigned)(argv[4][0] - '0');
  assert(merged <= 1);
  unsigned domain_count = 3 - merged;
  unsigned members = 2 + merged;
  assert(strlen(argv[2]) == domain_count);
  unsigned seed = (unsigned)strtoul(argv[3], NULL, 10);
  unsigned settled = 0;
  unsigned disposed = 0;
  chain_domain_t owners[3] = {0};
  rinzu_native_completion_v3_t *scalar = calloc(1, sizeof(*scalar));
  assert(scalar);
  for (unsigned i = 0; i < 6; ++i) {
    unsigned choice = (unsigned)(argv[1][i] - '0');
    assert(choice < 6 && !(settled & (1u << choice)));
    unsigned d = choice / members;
    unsigned g = choice % members;
    domain = d;
    if (g == 0) {
      assert(d == 0 || (settled & (1u << (choice - members))));
      if (d)
        owners[d - 1].storage[0] = NULL;
      init(&owners[d], d, scalar, members);
      publish_first(&owners[d]);
      settle(&owners[d], 0, 0);
      publish_following(&owners[d], seed % 4 == d);
    } else {
      assert(settled & (1u << (choice - g)));
      settle(&owners[d], g, seed % 4 == d && g == members - 1);
    }
    settled |= 1u << choice;
  }
  for (unsigned i = 0; i < domain_count; ++i) {
    unsigned d = (unsigned)(argv[2][i] - '0');
    assert(d < domain_count && !(disposed & (1u << d)));
    finish(owners, domain_count, d, seed);
    disposed |= 1u << d;
  }
  domain = domain_count - 1;
  uintptr_t slot = atomic_load(&scalar->completion.notification);
  unsigned cancelled = atomic_load(&scalar->completion.base.cancelled);
  assert(scalar->result == value(domain, 0) && slot == RINZU_NATIVE_COMPLETION_DONE && !cancelled);
  trace("chain_final_slot", 100, (intptr_t)slot, cancelled);
  free(scalar);
  for (unsigned d = 0; d < domain_count; ++d)
    for (unsigned g = 1; g < members; ++g)
      free(owners[d].storage[g]);
  for (unsigned i = 0; i < RINZU_RESOURCE_COUNT; ++i)
    assert(!rinzu_resource_counters[i].live);
  return 0;
}
