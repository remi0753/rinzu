#include <stdlib.h>

static size_t allocations;
static void *counted_calloc(size_t count, size_t size) {
  ++allocations;
  return calloc(count, size);
}
#define calloc counted_calloc
#include "rinzu/managed_data.h"
#undef calloc
#include <assert.h>
#include <sys/wait.h>

int main(void) {
  rinzu_managed_data_t data = {0};
  rinzu_managed_roots_t roots = {0};
  rinzu_managed_enter(&roots);
  uint64_t caller = rinzu_managed_string(&data, (const unsigned char *)"caller", 6);
  rinzu_managed_publish(&roots, &caller, 1);
  rinzu_managed_frame_t *outer = roots.current;
  /* Growing storage preserves the current publication until it is replaced. */
  (void)rinzu_managed_reserve_roots(&roots, 32);
  assert(outer->count == 1 && outer->roots[0] == caller);
  rinzu_managed_collect(&data, &roots);
  assert(data.objects == 1);

  rinzu_managed_enter(&roots);
  uint64_t callee = rinzu_managed_string(&data, (const unsigned char *)"callee", 6);
  uint64_t live[] = {callee, callee, callee};
  rinzu_managed_publish(&roots, live, 3);
  rinzu_managed_frame_t *inner = roots.current;
  uint64_t *buffer = inner->roots;
  rinzu_managed_collect(&data, &roots);
  assert(data.objects == 2);
  rinzu_managed_leave(&roots);
  size_t before = allocations;
  for (int i = 0; i < 10000; ++i) {
    rinzu_managed_enter(&roots);
    assert(roots.current == inner && inner->roots == buffer && !inner->count);
    rinzu_managed_publish(&roots, live, 3);
    rinzu_managed_publish(&roots, live, 1);
    rinzu_managed_publish(&roots, NULL, 0);
    rinzu_managed_leave(&roots);
    assert(roots.current == outer);
  }
  assert(allocations == before);
  for (int i = 0; i < 10000; ++i) {
    uint64_t *words = rinzu_managed_prepare_roots(&roots, 4);
    words[0] = caller;
    words[1] = words[2] = words[3] = 0;
    rinzu_managed_commit_roots(&roots, 4);
  }
  assert(allocations == before);
  for (int kind = 0; kind < 6; ++kind) {
    pid_t child = fork();
    assert(child >= 0);
    if (!child) {
      uint64_t *words = rinzu_managed_prepare_roots(&roots, 1);
      words[0] = caller;
      switch (kind) {
      case 0:
        rinzu_managed_collect(&data, &roots);
        break;
      case 1:
        rinzu_managed_enter(&roots);
        break;
      case 2:
        rinzu_managed_leave(&roots);
        break;
      case 3:
        rinzu_managed_commit_roots(&roots, 2);
        break;
      case 4: {
        rinzu_managed_data_t other = {0};
        words[0] = rinzu_managed_string(&other, (const unsigned char *)"foreign", 7);
        rinzu_managed_commit_roots(&roots, 1);
        rinzu_managed_safepoint(&data, &roots);
        break;
      }
      case 5: {
        rinzu_managed_commit_roots(&roots, 1);
        rinzu_managed_roots_t parked = {0};
        rinzu_managed_root_set_t attached = {0};
        rinzu_managed_enter(&parked);
        rinzu_managed_attach_roots(&data, &attached, &parked);
        (void)rinzu_managed_prepare_roots(&parked, 1);
        rinzu_managed_collect(&data, &roots);
        break;
      }
      }
      _Exit(1);
    }
    int status;
    assert(waitpid(child, &status, 0) == child && WIFEXITED(status) && WEXITSTATUS(status) == 70);
  }
  /* Neither pooled frames nor the tail of a reused buffer are roots. */
  rinzu_managed_collect(&data, &roots);
  assert(data.objects == 1);
  rinzu_managed_publish(&roots, NULL, 0);
  rinzu_managed_collect(&data, &roots);
  assert(!data.objects);
  rinzu_managed_leave(&roots);
  rinzu_managed_dispose(&roots);
  rinzu_managed_data_dispose(&data);
}
