#include <stdlib.h>
#include <string.h>
static int fail_allocation;
static size_t allocation_calls, release_calls;
static void *observed_calloc(size_t count, size_t size) {
  ++allocation_calls;
  return fail_allocation ? NULL : calloc(count, size);
}
static void observed_free(void *memory) {
  if (memory)
    ++release_calls;
  free(memory);
}
#define calloc observed_calloc
#define free observed_free
#include "rinzu/source_async.h"
#undef calloc
#undef free
#include <assert.h>

static void zeroed(const void *memory, size_t bytes) {
  const unsigned char *value = memory;
  for (size_t index = 0; index < bytes; ++index)
    assert(!value[index]);
}
static void baseline(void) {
  for (unsigned index = 0; index < RINZU_RESOURCE_COUNT; ++index)
    assert(!rinzu_resource_counters[index].live);
}
static void sizes(void) {
  rinzu_async_runtime_t runtime;
  rinzu_async_init(&runtime);
  const size_t sizes[] = {8, 63, 64, 65, 127, 128, 616, 1688, 1704, 1736, 2047, 2048};
  for (size_t index = 0; index < sizeof(sizes) / sizeof(*sizes); ++index) {
    size_t size = sizes[index], rounded = (size + 63) / 64 * 64;
    void *memory = rinzu_async_control_allocate(&runtime, size);
    zeroed(memory, rounded);
    memset(memory, 0xa5, rounded);
    rinzu_async_control_release(&runtime, memory, size);
    assert(runtime.control_cached_bytes <= 65536);
    size_t before = allocation_calls;
    void *again = rinzu_async_control_allocate(&runtime, size);
    assert(again == memory && allocation_calls == before);
    zeroed(again, rounded);
    rinzu_async_control_release(&runtime, again, size);
  }
  size_t before = allocation_calls, freed = release_calls;
  void *large = rinzu_async_control_allocate(&runtime, 2049);
  assert(allocation_calls == before + 1);
  zeroed(large, 2049);
  rinzu_async_control_release(&runtime, large, 2049);
  assert(release_calls == freed + 1);
  rinzu_async_dispose(&runtime);
  assert(!runtime.control_cached_bytes && !runtime.control_live);
  baseline();
}
static void bounded(void) {
  rinzu_async_runtime_t runtime;
  rinzu_async_init(&runtime);
  void *blocks[64];
  for (size_t index = 0; index < 64; ++index)
    blocks[index] = rinzu_async_control_allocate(&runtime, 2048);
  assert(runtime.control_live == 64);
  assert(rinzu_resource_counters[RINZU_RESOURCE_RUNTIME_CONTROL].live == 131072);
  size_t before = release_calls;
  for (size_t index = 0; index < 64; ++index)
    rinzu_async_control_release(&runtime, blocks[index], 2048);
  assert(!runtime.control_live && runtime.control_cached_bytes == 65536);
  assert(release_calls == before + 32);
  assert(rinzu_resource_counters[RINZU_RESOURCE_RUNTIME_CONTROL].live == 65536);
  rinzu_async_dispose(&runtime);
  baseline();
}
static void aliases(void) {
  rinzu_async_runtime_t runtime;
  rinzu_async_init(&runtime);
  for (unsigned index = 0; index < 128; ++index) {
    uint64_t handle = rinzu_async_ready(&runtime, RINZU_SCALAR_I64, index,
                                        (rinzu_source_position_t){9, 17, 29, 1});
    rinzu_async_future_t *future = rinzu_async_find(&runtime, handle);
    assert(rinzu_async_materialize(future) == index);
    assert(rinzu_async_materialize(rinzu_async_find(&runtime, handle)) == index);
    assert(future->source.file == 9 && future->source.start_byte == 17);
    rinzu_async_release(future);
    assert(!rinzu_async_lookup(&runtime, handle));
    fail_allocation = 1;
    rinzu_async_future_t *again = rinzu_async_new(&runtime, RINZU_ASYNC_READY, RINZU_SCALAR_I64,
                                                  (rinzu_source_position_t){0});
    assert(again == future && rinzu_async_handle(again) != handle);
    assert(!again->settled && !again->result_slot && !again->failure.primary);
    assert(!again->task_context && !again->native_completion && !again->ready_waiters);
    assert(!atomic_load(&again->physical_done) && !atomic_load(&again->cancel_requested));
    assert(!rinzu_async_lookup(&runtime, handle));
    fail_allocation = 0;
    rinzu_async_release(again);
  }
  rinzu_async_dispose(&runtime);
  baseline();
}
static void *foreign(void *raw) {
  rinzu_async_new(raw, RINZU_ASYNC_READY, RINZU_SCALAR_I64, (rinzu_source_position_t){0});
  return NULL;
}
int main(int argc, char **argv) {
  atomic_store(&rinzu_resource_enabled, 1);
  if (argc > 1) {
    rinzu_async_runtime_t runtime;
    rinzu_async_init(&runtime);
    if (!strcmp(argv[1], "cold-oom")) {
      fail_allocation = 1;
      void *memory = rinzu_async_control_allocate(&runtime, 616);
      rinzu_async_control_release(&runtime, memory, 616);
    } else if (!strcmp(argv[1], "busy-dispose")) {
      void *memory = rinzu_async_control_allocate(&runtime, 616);
      rinzu_async_dispose(&runtime);
      free(memory);
      return 3;
    } else if (!strcmp(argv[1], "foreign-owner")) {
      pthread_t thread;
      assert(!pthread_create(&thread, NULL, foreign, &runtime));
      assert(!pthread_join(thread, NULL));
    } else if (!strcmp(argv[1], "isolated-oom")) {
      void *memory = rinzu_async_control_allocate(&runtime, 616);
      rinzu_async_control_release(&runtime, memory, 616);
      rinzu_async_runtime_t other;
      rinzu_async_init(&other);
      fail_allocation = 1;
      memory = rinzu_async_control_allocate(&other, 616);
      rinzu_async_control_release(&other, memory, 616);
      rinzu_async_dispose(&other);
    } else {
      rinzu_async_dispose(&runtime);
      return 2;
    }
    rinzu_async_dispose(&runtime);
    return 3;
  }
  sizes();
  bounded();
  aliases();
  puts("CONTROL_CACHE:classes:zero:bound:oversize:alias:reawait:reuse:accounting:dispose");
  return 0;
}
