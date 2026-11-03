#include <stdlib.h>
#include <string.h>
static int fail_allocation;
static void *index_allocate(size_t count, size_t bytes) {
  return fail_allocation ? NULL : calloc(count, bytes);
}
#define calloc index_allocate
#include "rinzu/source_async.h"
#undef calloc
#include <assert.h>

int main(int argc, char **argv) {
  rinzu_async_runtime_t runtime;
  rinzu_async_init(&runtime);
  rinzu_child_scope_t scope;
  rinzu_child_init(&scope);
  runtime.scope = &scope;
  if (argc > 1) {
    if (!strcmp(argv[1], "missing"))
      rinzu_async_find(&runtime, RINZU_ASYNC_HANDLE_BASE + 1);
    else if (!strcmp(argv[1], "overflow"))
      rinzu_async_index_resize(&runtime, SIZE_MAX);
    else if (!strcmp(argv[1], "future-oom")) {
      fail_allocation = 1;
      rinzu_async_new(&runtime, RINZU_ASYNC_READY, RINZU_SCALAR_I64, (rinzu_source_position_t){0});
    } else if (!strcmp(argv[1], "child-oom")) {
      fail_allocation = 1;
      rinzu_child_admit(&scope, 0);
    } else return 2;
    return 3;
  }
  enum { COUNT = 4096 };
  uint64_t last = 0;
  for (int batch = 0; batch < 3; ++batch) {
    rinzu_async_future_t *futures[COUNT];
    uint64_t handles[COUNT];
    for (unsigned i = 0; i < COUNT; ++i) {
      futures[i] = rinzu_async_new(&runtime, RINZU_ASYNC_READY, RINZU_SCALAR_I64,
                                  (rinzu_source_position_t){0});
      handles[i] = rinzu_async_handle(futures[i]);
      assert(handles[i] > last);
      last = handles[i];
      assert(rinzu_async_find(&runtime, handles[i]) == futures[i]);
      assert(rinzu_child_find(&scope, futures[i]->identity));
    }
    for (unsigned i = 0; i < COUNT; ++i) {
      unsigned slot = (i * 661) & (COUNT - 1);
      rinzu_async_future_t *future = rinzu_async_find(&runtime, handles[slot]);
      assert(future == futures[slot]);
      assert(!rinzu_child_withdraw_admission(&scope, future->identity));
      assert(!rinzu_child_find(&scope, future->identity));
      rinzu_async_release(future);
      assert(!rinzu_async_lookup(&runtime, handles[slot]));
    }
    assert(!runtime.futures && !runtime.records && runtime.future_capacity == 32);
    assert(!scope.children && !scope.children_tail && !scope.child_count);
    assert(!rinzu_async_lookup(&runtime, RINZU_ASYNC_HANDLE_BASE));
    assert(!rinzu_async_lookup(&runtime, UINT64_MAX));
  }
  rinzu_async_dispose(&runtime);
  assert(!runtime.future_index && !runtime.future_capacity);
  assert(!rinzu_child_begin_close(&scope, NULL));
  assert(!rinzu_child_finish_close(&scope));
  rinzu_child_dispose(&scope);
  return 0;
}
