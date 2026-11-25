#include <assert.h>
#include <stdlib.h>
#include <unistd.h>

static int reject_calloc;
static void *controlled_calloc(size_t count, size_t size) {
  return reject_calloc ? NULL : calloc(count, size);
}
#define calloc controlled_calloc
#include "rinzu/source_async.h"

static void cleanup(void) {
  (void)write(STDOUT_FILENO, "CLEANUP_RAN\n", 12);
}

static rinzu_async_future_t *admit(rinzu_async_runtime_t *runtime,
                                   rinzu_native_completion_v3_t *carrier) {
  rinzu_async_future_t *future = rinzu_async_new(runtime, RINZU_ASYNC_COMPLETION, RINZU_SCALAR_I64,
                                                 (rinzu_source_position_t){0});
  future->native_completion = &carrier->completion.base;
  rinzu_async_start(future);
  return future;
}

int main(void) {
  rinzu_async_runtime_t runtime;
  rinzu_async_init(&runtime);
  rinzu_native_completion_v3_t carriers[2];
  for (int i = 0; i < 2; ++i) {
    rinzu_native_completion_v2_init(&carriers[i].completion);
    carriers[i].completion.base.version = 3;
    carriers[i].completion.base.size = sizeof(carriers[i]);
    carriers[i].result = i + 7183;
  }
  rinzu_async_future_t *done = admit(&runtime, &carriers[0]);
  rinzu_async_future_t *pending = admit(&runtime, &carriers[1]);
  rinzu_native_completion_wake_t *sdk =
      rinzu_native_completion_v2_publish_target(&carriers[0].completion);
  assert(sdk == &runtime.native_wake->header);
  sdk->notify(sdk);
  rinzu_async_native_progress(&runtime);
  assert(rinzu_async_try_collect(done) && done->result_slot == 7183);
  /* Keep the settled record live so the next record needs a fresh allocation. */
  assert(!runtime.control_cached_bytes);
  assert(rinzu_async_cancel(pending) && !rinzu_async_try_collect(pending));
  assert(!rinzu_native_completion_is_done(&carriers[1].completion.base));
  assert(atomic_load(&carriers[1].completion.notification) == (uintptr_t)sdk);
  assert(atomic_load(&runtime.native_wake->references) == 3);
  assert(!atexit(cleanup));
  static const char marker[] = "NATIVE_DRAIN_AND_SDK_REFERENCE_LIVE\n";
  assert(write(STDOUT_FILENO, marker, sizeof(marker) - 1) == (ssize_t)(sizeof(marker) - 1));
  reject_calloc = 1;
  /* Fail the allocation while the Owner prepares its next receipt. */
  (void)rinzu_async_new(&runtime, RINZU_ASYNC_COMPLETION, RINZU_SCALAR_I64,
                        (rinzu_source_position_t){0});
  return 99;
}
