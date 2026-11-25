#include "rinzu/source_async.h"
#include <assert.h>
#include <inttypes.h>
#include <stdio.h>
#include <string.h>

/* Deterministic public-boundary projections of the shared abstract traces.
 * This does not replace the concurrent notification/generation fixtures. */
static void acquire(rinzu_async_runtime_t *runtime, rinzu_async_future_t *future,
                    rinzu_native_completion_v3_t *carrier, int64_t payload) {
  carrier->result = payload;
  rinzu_native_completion_v2_publish(&carrier->completion);
  rinzu_async_native_progress(runtime);
  assert(atomic_load_explicit(&future->physical_done, memory_order_acquire));
  int64_t copied;
  memcpy(&copied, &future->result_slot, sizeof(copied));
  assert(copied == payload);
}

int main(int argc, char **argv) {
  assert(argc == 2);
  FILE *input = fopen(argv[1], "r");
  assert(input);
  char line[512];
  assert(fgets(line, sizeof(line), input));
  unsigned cases = 0;
  while (fgets(line, sizeof(line), input)) {
    char name[96], decision[32];
    unsigned control, collect_first, accepted;
    int64_t payload;
    assert(sscanf(line, "%95[^,],%u,%u,%u,%31[^,],%" SCNd64, name, &control, &collect_first,
                  &accepted, decision, &payload) == 6);
    rinzu_async_runtime_t runtime;
    rinzu_async_init(&runtime);
    rinzu_native_completion_v3_t *carrier = calloc(1, sizeof(*carrier));
    assert(carrier);
    rinzu_native_completion_v2_init(&carrier->completion);
    carrier->completion.base.version = 3;
    carrier->completion.base.size = sizeof(*carrier);
    rinzu_async_future_t *future = rinzu_async_new(&runtime, RINZU_ASYNC_COMPLETION,
                                                   RINZU_SCALAR_I64, (rinzu_source_position_t){0});
    future->native_completion = &carrier->completion.base;
    rinzu_async_start(future);
    if (collect_first) {
      acquire(&runtime, future, carrier, payload);
      assert(atomic_load(&future->terminal) == RINZU_ASYNC_COMPLETED);
      assert(rinzu_async_try_collect(future));
    }
    int won = control == 1 ? rinzu_async_cancel(future) : rinzu_async_timeout(future);
    assert(won == (int)accepted);
    if (!collect_first) {
      assert(!atomic_load(&future->physical_done));
      assert(!rinzu_async_try_collect(future));
      unsigned expected =
          !strcmp(decision, "cancelled") ? RINZU_ASYNC_CANCELLED : RINZU_ASYNC_TIMED_OUT;
      assert(atomic_load(&future->terminal) == expected);
      acquire(&runtime, future, carrier, payload);
      assert(atomic_load(&future->terminal) == expected);
      assert(rinzu_async_try_collect(future));
    }
    assert(future->settled && future->failure.primary == (accepted ? 2u : 0u));
    free(carrier);
    /* A rejected late request must not dereference the retired carrier. */
    assert(!rinzu_async_cancel(future));
    assert(!rinzu_async_timeout(future));
    if (!accepted)
      assert((int64_t)rinzu_async_materialize(future) == payload);
    rinzu_async_release(future);
    rinzu_async_dispose(&runtime);
    printf("%s:ok\n", name);
    ++cases;
  }
  assert(!ferror(input) && !fclose(input) && cases == 4);
  for (unsigned i = 0; i < RINZU_RESOURCE_COUNT; ++i)
    assert(!rinzu_resource_counters[i].live);
}
