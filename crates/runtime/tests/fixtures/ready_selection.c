#include <assert.h>
#include <inttypes.h>
#include <sched.h>
#include <stdint.h>
#include <stdio.h>

static void observe_candidate(uint64_t, int);
#define RINZU_NATIVE_READY_SCAN_EVENT(future, observed)                                            \
  observe_candidate((future)->identity, observed)
#include "rinzu/source_async.h"

static rinzu_native_completion_v3_t *late_carrier;
static rinzu_async_future_t *late_receipt;
static uint64_t publish_after;

static void await_physical(const rinzu_async_future_t *future) {
  while (!atomic_load_explicit(&future->physical_done, memory_order_acquire))
    sched_yield();
}

static void observe_candidate(uint64_t receipt, int observed) {
  printf("read,%" PRIu64 ",%d\n", receipt, observed);
  if (receipt == publish_after) {
    rinzu_native_completion_v2_publish(&late_carrier->completion);
    await_physical(late_receipt);
    printf("physical,%" PRIu64 ",0\n", late_receipt->identity);
  }
}

static rinzu_async_future_t *admit(rinzu_async_runtime_t *runtime,
                                   rinzu_native_completion_t *carrier, unsigned kind) {
  rinzu_async_future_t *future =
      rinzu_async_new(runtime, RINZU_ASYNC_COMPLETION, kind, (rinzu_source_position_t){0});
  future->native_completion = carrier;
  rinzu_async_start(future);
  return future;
}

int main(int argc, char **argv) {
  assert(argc == 3);
  unsigned late = (unsigned)atoi(argv[1]);
  publish_after = (uint64_t)atoi(argv[2]);
  assert(late < 2 && publish_after < 3);

  rinzu_async_runtime_t runtime;
  rinzu_async_init(&runtime);
  rinzu_native_completion_v3_t carriers[2];
  rinzu_async_future_t *receipts[3];
  for (unsigned i = 0; i < 2; ++i) {
    rinzu_native_completion_v3_t *carrier = &carriers[i];
    rinzu_native_completion_v2_init(&carrier->completion);
    carrier->completion.base.version = 3;
    carrier->completion.base.size = sizeof(*carrier);
    carrier->result = 7199 + i;
    receipts[i] = admit(&runtime, &carrier->completion.base, RINZU_SCALAR_I64);
  }
  rinzu_native_completion_t polling;
  rinzu_native_completion_init(&polling);
  receipts[2] = admit(&runtime, &polling, RINZU_TRANSFER_UNIT);
  assert(runtime.reactor_started);
  for (unsigned i = 0; i < 3; ++i)
    assert(receipts[i]->identity == i);

  late_carrier = &carriers[late];
  late_receipt = receipts[late];
  atomic_store_explicit(&polling.done, 1, memory_order_release);
  await_physical(receipts[2]);
  puts("physical,2,0");

  rinzu_async_future_t *choice = rinzu_async_oldest_ready_before(&runtime, UINT64_MAX);
  assert(choice && rinzu_async_try_collect(choice));
  printf("settled,%" PRIu64 ",0\n", choice->identity);
  rinzu_native_completion_v2_publish(&carriers[1 - late].completion);
  for (unsigned i = 0; i < 3; ++i) {
    await_physical(receipts[i]);
    if (receipts[i] != choice)
      assert(rinzu_async_try_collect(receipts[i]));
    assert(receipts[i]->result_slot == (i == 2 ? 0 : i + 7199));
    assert(!receipts[i]->failure.primary);
    rinzu_async_release(receipts[i]);
  }
  rinzu_async_dispose(&runtime);
  return 0;
}
