#include <assert.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

static void observe(const char *, uint64_t, int64_t, int64_t);
static void target_free(void *);
#define RINZU_NATIVE_LIFETIME_EVENT(k, id, a, b) observe(k, id, a, b)
#define RINZU_NATIVE_WAKE_FREE_EVENT(target) target_free(target)
#include "rinzu/source_async.h"

#define BATCHES 3
#define MEMBERS 6
static _Thread_local unsigned publishing;
static rinzu_async_native_wake_t *target;
static rinzu_native_completion_wake_t *leaders[BATCHES];
static unsigned leader;
static int sdk_returns_before_runtime;
static pthread_mutex_t notification_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t notification_changed = PTHREAD_COND_INITIALIZER;
static int holding, started, may_return;

static size_t refs(void) {
  return atomic_load_explicit(&target->references, memory_order_acquire);
}

static void row(const char *action, unsigned batch, unsigned member, size_t references) {
  flockfile(stdout);
  printf("%s,%u,%u,%zu\n", action, batch, member, references);
  funlockfile(stdout);
}

static void *return_leaders(void *unused) {
  (void)unused;
  for (unsigned pos = BATCHES; pos > 0; --pos) {
    publishing = pos - 1;
    rinzu_native_completion_wake_t *lease = leaders[publishing];
    if (publishing)
      lease->notify(lease);
    row("return", publishing, leader, refs());
    leaders[publishing] = NULL;
    lease->release(lease);
  }
  return NULL;
}

static void observe(const char *action, uint64_t id, int64_t a, int64_t b) {
  (void)id;
  (void)b;
  if (!strcmp(action, "notify_enter")) {
    row("begin", publishing, (unsigned)a, refs());
    if (holding && publishing == 0) {
      assert(!pthread_mutex_lock(&notification_lock));
      started = 1;
      assert(!pthread_cond_broadcast(&notification_changed));
      while (!may_return)
        assert(!pthread_cond_wait(&notification_changed, &notification_lock));
      assert(!pthread_mutex_unlock(&notification_lock));
    }
  } else if (!strcmp(action, "notify_leave")) {
    row("end", publishing, 0, refs());
  } else if (!strcmp(action, "detach")) {
    row("detach", 0, 0, refs());
  } else if (!strcmp(action, "runtime_reference_release")) {
    /* The target's context gate is unlocked. The runtime still holds its reference. */
    if (sdk_returns_before_runtime) {
      pthread_t publisher;
      assert(!pthread_create(&publisher, NULL, return_leaders, NULL));
      assert(!pthread_join(publisher, NULL));
    }
    row("runtime_return", 0, 0, refs());
  }
}

static void target_free(void *freed) {
  assert(freed == &target->header);
  row("destroy", 0, 0, refs());
  target = NULL;
}

static void *notify_first(void *raw) {
  rinzu_native_completion_wake_t *ref = raw;
  publishing = 0;
  ref->notify(ref);
  return NULL;
}

static rinzu_native_completion_wake_t *capture(rinzu_native_completion_v3_t *carrier,
                                               unsigned index) {
  rinzu_native_completion_wake_t *ref =
      rinzu_native_completion_v2_publish_target(&carrier->completion);
  assert(ref == &target->header);
  row("capture", index / 2, index % 2, refs());
  return ref;
}

int main(int argc, char **argv) {
  assert(argc == 4 && strlen(argv[1]) == MEMBERS);
  leader = (unsigned)atoi(argv[2]);
  sdk_returns_before_runtime = atoi(argv[3]);
  assert(leader < 2 && (sdk_returns_before_runtime == 0 || sdk_returns_before_runtime == 1));
  rinzu_async_runtime_t runtime;
  rinzu_async_init(&runtime);
  rinzu_native_completion_v3_t carriers[MEMBERS];
  rinzu_async_future_t *receipts[MEMBERS];
  rinzu_native_completion_wake_t *leases[MEMBERS];
  for (unsigned i = 0; i < MEMBERS; ++i) {
    rinzu_native_completion_v3_t *carrier = &carriers[i];
    int transfer = !(i % 2);
    rinzu_native_completion_v2_init(&carrier->completion);
    if (transfer) {
      carrier->completion.base.version = 3;
      carrier->completion.base.size = sizeof(*carrier);
      carrier->result = i + 7183;
    }
    receipts[i] = rinzu_async_new(&runtime, RINZU_ASYNC_COMPLETION,
                                  transfer ? RINZU_SCALAR_I64 : RINZU_TRANSFER_UNIT,
                                  (rinzu_source_position_t){0});
    receipts[i]->native_completion = &carrier->completion.base;
    rinzu_async_start(receipts[i]);
    assert(atomic_load(&carrier->completion.notification) ==
           (uintptr_t)&runtime.native_wake->header);
  }
  target = runtime.native_wake;
  row("registered", 0, 0, refs());
  for (unsigned i = 0; i < MEMBERS; i += 2)
    leases[i] = capture(&carriers[i], i);
  pthread_t first;
  for (unsigned b = 0; b < BATCHES; ++b) {
    unsigned i = b * 2 + 1;
    leases[i] = capture(&carriers[i], i);
    unsigned duplicate = b * 2 + (1 - leader);
    leaders[b] = leases[b * 2 + leader];
    leases[duplicate]->release(leases[duplicate]);
    row("merge", b, 1 - leader, refs());
    if (b == 0) {
      holding = 1;
      assert(!pthread_create(&first, NULL, notify_first, leaders[b]));
      assert(!pthread_mutex_lock(&notification_lock));
      while (!started)
        assert(!pthread_cond_wait(&notification_changed, &notification_lock));
      assert(!pthread_mutex_unlock(&notification_lock));
    }
  }
  unsigned seen = 0;
  for (unsigned pos = 0; pos < MEMBERS; ++pos) {
    unsigned i = (unsigned)(argv[1][pos] - '0');
    assert(i < MEMBERS && !(seen & (1u << i)));
    seen |= 1u << i;
    assert(rinzu_async_try_collect(receipts[i]));
    assert(!receipts[i]->failure.primary && receipts[i]->result_slot == (i % 2 ? 0 : i + 7183));
    row("settle", i / 2, i % 2, refs());
    rinzu_async_release(receipts[i]);
  }
  assert(!pthread_mutex_lock(&notification_lock));
  may_return = 1;
  assert(!pthread_cond_broadcast(&notification_changed));
  assert(!pthread_mutex_unlock(&notification_lock));
  assert(!pthread_join(first, NULL));
  rinzu_async_dispose(&runtime);
  if (!sdk_returns_before_runtime)
    return_leaders(NULL);
  assert(!target);
  return 0;
}
