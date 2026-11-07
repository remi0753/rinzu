#include "rinzu/continuation.h"
#include <assert.h>
#include <signal.h>
#include <sys/wait.h>

typedef struct {
  rinzu_continuation_t continuation;
  uint64_t seed, result;
  int stage;
} task_t;
static void body(void *raw) {
  task_t *task = raw;
  volatile uint64_t local[128];
  for (size_t i = 0; i < 128; ++i)
    local[i] = task->seed + i;
  task->stage = 1;
  rinzu_continuation_yield(&task->continuation);
  for (size_t i = 0; i < 128; ++i)
    assert(local[i] == task->seed + i);
  task->result = local[127];
  task->stage = 2;
}
static void *wrong_owner(void *raw) {
  rinzu_continuation_cache_dispose(raw);
  return NULL;
}
int main(int argc, char **argv) {
  rinzu_continuation_t root;
  rinzu_continuation_accounting_t accounting = {0};
  rinzu_continuation_init_root(&root);
  if (argc > 1) {
    task_t task = {0};
    rinzu_continuation_init(&task.continuation, 256 * 1024, body, &task, &accounting);
    if (!strcmp(argv[1], "live"))
      rinzu_continuation_cache_dispose(&accounting);
    if (!strcmp(argv[1], "unfinished")) {
      rinzu_continuation_resume(&root, &task.continuation);
      rinzu_continuation_dispose(&task.continuation);
    }
    rinzu_continuation_dispose(&task.continuation); /* never started */
    if (!strcmp(argv[1], "foreign")) {
      pthread_t thread;
      assert(!pthread_create(&thread, NULL, wrong_owner, &accounting));
      assert(!pthread_join(thread, NULL));
    }
    if (!strcmp(argv[1], "double"))
      rinzu_continuation_dispose(&task.continuation);
    rinzu_continuation_init(&task.continuation, 256 * 1024, body, &task, &accounting);
    if (!strcmp(argv[1], "lower") || !strcmp(argv[1], "upper")) {
      rinzu_continuation_resume(&root, &task.continuation);
      assert(task.continuation.mapping && task.stage == 1);
    }
    if (!strcmp(argv[1], "lower"))
      *(volatile char *)task.continuation.mapping = 1;
    if (!strcmp(argv[1], "upper"))
      *((volatile char *)task.continuation.mapping + task.continuation.mapping_size - 1) = 1;
    return 99;
  }
  enum { COUNT = 16, BATCHES = 64 };
  task_t tasks[COUNT] = {0};
  for (size_t batch = 0; batch < BATCHES; ++batch) {
    for (size_t i = 0; i < COUNT; ++i) {
      tasks[i].seed = batch * 1000 + i;
      tasks[i].stage = 0;
      rinzu_continuation_init(&tasks[i].continuation, 256 * 1024, body, &tasks[i], &accounting);
      assert(!tasks[i].continuation.started && !tasks[i].continuation.finished);
      rinzu_continuation_resume(&root, &tasks[i].continuation);
      assert(tasks[i].stage == 1);
    }
    assert(accounting.live == COUNT);
    for (size_t i = COUNT; i--;) {
      rinzu_continuation_resume(&root, &tasks[i].continuation);
      assert(tasks[i].stage == 2 && tasks[i].result == tasks[i].seed + 127);
      rinzu_continuation_dispose(&tasks[i].continuation);
    }
    assert(!accounting.live && accounting.cached_bytes <= accounting.cache_limit);
  }
  if (!getenv("RINZU_CONTINUATION_CACHE_BYTES")) {
    assert(accounting.allocations == COUNT && accounting.reuses == (BATCHES - 1) * COUNT);
    assert(accounting.cached_bytes == accounting.mapped_bytes && accounting.cached_bytes);
  } else if (!accounting.cache_limit) {
    assert(accounting.allocations == BATCHES * COUNT && !accounting.reuses &&
           !accounting.mapped_bytes);
  }
  /* Live work is allowed to exceed the cache budget. */
  task_t many[40] = {0};
  for (size_t i = 0; i < 40; ++i) {
    rinzu_continuation_init(&many[i].continuation, 256 * 1024, body, &many[i], &accounting);
    rinzu_continuation_resume(&root, &many[i].continuation);
  }
  assert(accounting.live == 40);
  for (size_t i = 0; i < 40; ++i) {
    rinzu_continuation_resume(&root, &many[i].continuation);
    rinzu_continuation_dispose(&many[i].continuation);
  }
  rinzu_continuation_cache_dispose(&accounting);
  assert(!accounting.cached && !accounting.cached_bytes && !accounting.mapped_bytes &&
         !accounting.live);
}
