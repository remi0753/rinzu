#include <assert.h>
/* The test build substitutes only the continuation resume in resume_ready.
   Future lookup, snapshot construction/order and candidate queue operations
   remain production code. Real continuation/GC/control paths are tested by
   async_ready_{order,alias}.rinzu and the CLI/lifecycle suites. */
static rinzu_async_future_t *guard_gate;
static RinzuAsyncTask *guard_old, *guard_victim;
static unsigned guard_mode;
static int64_t guard_seen[4096];
static size_t guard_count;
static uint64_t guard_checksum;
static RinzuAsyncTask *guard_created[8704];

static void guard_queue(RinzuAsyncTask *task) {
#if TEST_READY_INDEX
  rinzu_aot_async_queue_ready(task);
#else
  (void)task;
#endif
}
static void guard_unqueue(RinzuAsyncTask *task) {
#if TEST_READY_INDEX
  rinzu_aot_async_unqueue_ready(task);
#else
  (void)task;
#endif
}
static void guard_link(RinzuAsyncTask *task) {
  RinzuAsyncState *state = task->context.async;
  assert(!task->next && !task->previous && !task->future->task_context);
  assert(!task->future->main_admitted);
  ++state->runtime.live_main_tasks;
  task->future->main_admitted = 1;
  task->future->task_context = task;
  task->next = state->tasks;
  if (task->next) task->next->previous = task;
  state->tasks = task;
}
static void guard_unlink(RinzuAsyncTask *task) {
  RinzuAsyncState *state = task->context.async;
  assert(task->future->task_context == task);
  if (task->future->main_admitted) {
    assert(state->runtime.live_main_tasks);
    --state->runtime.live_main_tasks;
    task->future->main_admitted = 0;
  }
  if (task->previous) task->previous->next = task->next;
  else state->tasks = task->next;
  if (task->next) task->next->previous = task->previous;
  task->next = task->previous = NULL;
  task->future->task_context = NULL;
}
static RinzuAsyncTask *guard_task(RinzuContext *context, int64_t index, int ready) {
  RinzuAsyncState *state = context->async;
  RinzuAsyncTask *task = calloc(1, sizeof(*task));
  assert(task);
  task->context.async = state;
  task->context.task = task;
  task->index = index;
  task->future = rinzu_async_new(&state->runtime, RINZU_ASYNC_TASK, RINZU_SCALAR_I64,
                                 (rinzu_source_position_t){0});
  assert(index >= 0 && (uint64_t)index < sizeof(guard_created)/sizeof(*guard_created));
  assert(!guard_created[index]);
  guard_created[index] = task;
  task->continuation.started = !ready;
  task->waiting = ready ? NULL : guard_gate;
  guard_link(task);
  if (ready) guard_queue(task);
  return task;
}
static void guard_retire(RinzuAsyncTask *task) {
  guard_unqueue(task);
  if (task->future->task_context) guard_unlink(task);
  (void)rinzu_async_materialize(task->future);
  rinzu_async_release(task->future);
  guard_created[task->index] = NULL;
  free(task);
}
static __attribute__((noinline)) void ready_probe_resume(RinzuContext *context, RinzuAsyncTask *task) {
  guard_unqueue(task);
  task->continuation.started = 1;
  task->waiting = NULL;
  if (guard_mode == 4) {
    guard_checksum += (uint64_t)task->index + 1;
    guard_unlink(task);
    return;
  }
  assert(guard_count < sizeof(guard_seen)/sizeof(*guard_seen));
  guard_seen[guard_count++] = task->index;
  if ((guard_mode == 1 || guard_mode == 5) && task->index == 1) {
    (void)guard_task(context, 3, 1);
    (void)rinzu_async_materialize(guard_gate);
    guard_queue(guard_old);
    if (guard_mode == 5) rinzu_aot_async_resume_ready(context);
  }
  if ((guard_mode == 2 || guard_mode == 3) && task->index == 0) {
    if (guard_mode == 3) {
      guard_retire(guard_victim);
      guard_victim = NULL;
    }
    rinzu_aot_async_resume_ready(context);
  }
  (void)rinzu_async_materialize(task->future);
  guard_unlink(task);
}
/* Selection-only diagnostic: synthetic task bodies and Future records are
   reused; enqueue/unqueue, lifetime-list links, real snapshot/lookup/sort and
   scratch allocation are timed. Completed bodies leave the lifetime list,
   as real continuations do. This is not a request or Future-lifecycle bench. */
static size_t guard_parameter(const char *name, size_t fallback, size_t maximum) {
  const char *raw = getenv(name);
  if (!raw) return fallback;
  char *end;
  unsigned long long value = strtoull(raw, &end, 10);
  assert(raw[0] && !*end && value <= maximum);
  return (size_t)value;
}
static uint64_t guard_cpu(void) {
  struct timespec now;
  assert(!clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &now));
  return (uint64_t)now.tv_sec * UINT64_C(1000000000) + (uint64_t)now.tv_nsec;
}
static void guard_bench(RinzuContext *context) {
  size_t idle = guard_parameter("READY_IDLE", 16, 8192);
  size_t burst = guard_parameter("READY_BURST", 1, 512);
  size_t direction = guard_parameter("READY_DIRECTION", 0, 2);
  size_t count = guard_parameter("READY_ITERATIONS", 20000, 1000000);
  assert(burst && (burst & (burst - 1)) == 0 && count);
  for (size_t i = 0; i < idle; ++i) (void)guard_task(context, (int64_t)i, 0);
  RinzuAsyncTask *tasks[512];
  for (size_t i = 0; i < burst; ++i)
    tasks[i] = guard_task(context, (int64_t)(idle + i), 0);
  guard_mode = 4;
  uint64_t start = guard_cpu();
  for (size_t iteration = 0; iteration < count; ++iteration) {
    for (size_t i = 0; i < burst; ++i) {
      /* Admission order is always increasing identity. Readiness order below
         must not reorder the lifetime list as synthetic records are reused. */
      tasks[i]->continuation.started = 0;
      tasks[i]->waiting = NULL;
      if (!tasks[i]->future->task_context) guard_link(tasks[i]);
    }
    for (size_t i = 0; i < burst; ++i) {
      size_t index = direction == 1 ? burst - i - 1 : direction == 2 ? (i * 37) % burst : i;
      guard_queue(tasks[index]);
    }
    rinzu_aot_async_resume_ready(context);
  }
  uint64_t elapsed = guard_cpu() - start;
  assert(guard_checksum == count * (burst * (2 * idle + burst + 1) / 2));
  fprintf(stdout, "READY_BENCH:%zu:%zu:%zu:%zu:%llu:%llu\n", idle, burst, direction, count,
            (unsigned long long)elapsed, (unsigned long long)guard_checksum);
}
int main(int argc, char **argv) {
  assert(argc == 2);
  const char *mode = argv[1];
  RinzuAsyncState state = {0};
  RinzuContext context = {.async = &state};
  rinzu_async_init(&state.runtime);
  state.root = &context;
  guard_gate = rinzu_async_find(&state.runtime,
      rinzu_async_ready(&state.runtime, RINZU_SCALAR_I64, 42, (rinzu_source_position_t){0}));
  if (!strcmp(mode, "bench")) {
    guard_bench(&context);
  } else if (!strcmp(mode, "forward") || !strcmp(mode, "reverse") || !strcmp(mode, "mixed")) {
    RinzuAsyncTask *tasks[96];
    for (int64_t i = 0; i < 96; ++i) tasks[i] = guard_task(&context, i, 0);
    for (size_t i = 0; i < 96; ++i) {
      size_t index = !strcmp(mode, "reverse") ? 95 - i :
                     !strcmp(mode, "mixed") ? (i * 37) % 96 : i;
      tasks[index]->continuation.started = 0;
      tasks[index]->waiting = NULL;
      guard_queue(tasks[index]);
    }
    rinzu_aot_async_resume_ready(&context);
    assert(guard_count == 96);
    for (int64_t i = 0; i < 96; ++i) assert(guard_seen[i] == i);
  } else if (!strcmp(mode, "mixed-sparse")) {
    RinzuAsyncTask *tasks[20];
    for (int64_t i = 0; i < 20; ++i) tasks[i] = guard_task(&context, i, 0);
    const size_t order[] = {16, 19, 17, 18};
    for (size_t i = 0; i < 4; ++i) {
      tasks[order[i]]->continuation.started = 0;
      tasks[order[i]]->waiting = NULL;
      guard_queue(tasks[order[i]]);
    }
    rinzu_aot_async_resume_ready(&context);
    assert(guard_count == 4);
    for (int64_t i = 0; i < 4; ++i) assert(guard_seen[i] == i + 16);
  } else if (!strcmp(mode, "deferred") || !strcmp(mode, "deferred-reentrant")) {
    guard_mode = !strcmp(mode, "deferred") ? 1 : 5;
    guard_old = guard_task(&context, 0, 0);
    (void)guard_task(&context, 1, 1);
    (void)guard_task(&context, 2, 1);
    rinzu_aot_async_resume_ready(&context);
    const int64_t expected[] = {1, 2, 0, 3}, nested[] = {1, 0, 2, 3};
    assert(guard_count == 4 && !memcmp(guard_seen, guard_mode == 5 ? nested : expected, sizeof(expected)));
  } else if (!strcmp(mode, "reentrant") || !strcmp(mode, "retired") || !strcmp(mode, "control")) {
    guard_mode = !strcmp(mode, "reentrant") ? 2 : !strcmp(mode, "retired") ? 3 : 0;
    (void)guard_task(&context, 0, 1);
    guard_victim = guard_task(&context, 1, 1);
    (void)guard_task(&context, 2, 1);
    int removed = guard_mode == 3 || !strcmp(mode, "control");
    if (!strcmp(mode, "control")) {
      guard_retire(guard_victim);
      guard_victim = NULL;
    }
    rinzu_aot_async_resume_ready(&context);
    const int64_t expected[] = {0, 1, 2}, without[] = {0, 2};
    assert(guard_count == (removed ? 2u : 3u));
    assert(!memcmp(guard_seen, removed ? without : expected,
                   guard_count * sizeof(*guard_seen)));
  } else {
    assert(!strcmp(mode, "empty"));
    rinzu_aot_async_resume_ready(&context);
    assert(!guard_count);
  }
#if TEST_READY_INDEX
  assert(!state.ready_count && !state.ready_head && !state.ready_tail);
#endif
  for (size_t i = 0; i < sizeof(guard_created)/sizeof(*guard_created); ++i)
    if (guard_created[i]) guard_retire(guard_created[i]);
  assert(!state.tasks);
  (void)rinzu_async_materialize(guard_gate);
  rinzu_async_release(guard_gate);
  rinzu_async_dispose(&state.runtime);
  assert(!state.runtime.records);
  fprintf(stderr, "READY_SIZES:%zu:%zu:%zu\n", sizeof(RinzuAsyncTask), sizeof(RinzuAsyncState),
            sizeof(rinzu_async_ready_waiter_t));
  if (strcmp(mode, "bench")) fprintf(stdout, "READY_SNAPSHOT=%s\n", mode);
}
