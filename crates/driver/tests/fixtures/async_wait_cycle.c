/* Appended to the production generated runtime. Waits/yields/resumes use its
   actual private continuation functions, with native-only task dependencies. */
#include <assert.h>
static rinzu_async_future_t *model_wanted[3];
static pthread_mutex_t model_gate = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t model_changed = PTHREAD_COND_INITIALIZER;
static int model_entered, model_stopped, model_pump;
static void model_block(rinzu_async_future_t *future) {
  (void)future;
  assert(!pthread_mutex_lock(&model_gate));
  model_entered = 1;
  assert(!pthread_cond_signal(&model_changed));
  while (!model_stopped)
    assert(!pthread_cond_wait(&model_changed, &model_gate));
  assert(!pthread_mutex_unlock(&model_gate));
}
static void model_notify(rinzu_async_future_t *future) {
  (void)future;
}
static void model_wait_body(void *raw) {
  RinzuAsyncTask *task = raw;
  rinzu_aot_async_wait(&task->context, model_wanted[task->index]);
  if (model_pump && task->index == 1) {
    rinzu_async_future_t *blocked = model_wanted[0];
    assert(!atomic_load_explicit(&blocked->physical_done, memory_order_acquire));
    assert(!blocked->joined && blocked->admitted && blocked->runtime->domains == 1);
    assert(!pthread_mutex_lock(&model_gate));
    model_stopped = 1;
    assert(!pthread_cond_signal(&model_changed));
    assert(!pthread_mutex_unlock(&model_gate));
  }
  rinzu_async_materialize(task->future);
}
int main(int argc, char **argv) {
  alarm(20);
  assert(argc == 2);
  RinzuAsyncState state = {0};
  RinzuContext root = {0};
  state.root = &root;
  root.async = &state;
  state.root_source = (rinzu_source_position_t){17, 31, 49, 1};
  state.root_label = "model.main";
  rinzu_async_init(&state.runtime);
  rinzu_continuation_init_root(&state.root_continuation);
  enum { COUNT = 3 };
  RinzuAsyncTask *tasks[COUNT];
  const char *labels[] = {"first.await", "second.await", "closing.await"};
  for (size_t n = 0; n < COUNT; n++) {
    RinzuAsyncTask *task = rinzu_aot_async_allocate_task(&state.runtime, NULL, 0);
    tasks[n] = task;
    task->index = (int64_t)n;
    task->function_label = labels[n];
    task->function_source = state.root_source;
    task->future =
        rinzu_async_new(&state.runtime, RINZU_ASYNC_TASK, RINZU_TRANSFER_UNIT, state.root_source);
    task->context.async = &state;
    task->context.task = task;
    task->context.source = state.root_source;
    task->context.shared_data = &root.data;
    task->context.shared_transitions = &root.transitions;
    rinzu_managed_attach_roots(&root.data, &task->context.domain_roots, &task->context.roots);
    task->next = state.tasks;
    if (task->next)
      task->next->previous = task;
    state.tasks = task;
    task->future->task_context = task;
    rinzu_continuation_init(&task->continuation, 256 * 1024, model_wait_body, task,
                            &state.continuations);
  }
  rinzu_async_future_t *leaf = rinzu_async_find(
      &state.runtime, rinzu_async_ready(&state.runtime, RINZU_TRANSFER_UNIT, 0, state.root_source));
  for (size_t n = 0; n < COUNT; n++)
    model_wanted[n] = leaf;
  model_pump = !strcmp(argv[1], "pump");
  if (model_pump) {
    rinzu_async_future_t *blocked =
        rinzu_async_new(&state.runtime, RINZU_ASYNC_WORKER, RINZU_TRANSFER_UNIT, state.root_source);
    rinzu_async_future_t *notifier =
        rinzu_async_new(&state.runtime, RINZU_ASYNC_WORKER, RINZU_TRANSFER_UNIT, state.root_source);
    blocked->run = model_block;
    notifier->run = model_notify;
    assert(rinzu_async_admit(blocked) && rinzu_async_admit(notifier));
    rinzu_async_start(blocked);
    assert(!pthread_mutex_lock(&model_gate));
    while (!model_entered)
      assert(!pthread_cond_wait(&model_changed, &model_gate));
    assert(!pthread_mutex_unlock(&model_gate));
    rinzu_async_start(notifier);
    assert(rinzu_async_cancel(blocked));
    model_wanted[0] = blocked;
    model_wanted[1] = notifier;
  }
  if (!strcmp(argv[1], "self"))
    model_wanted[0] = tasks[0]->future;
  if (!strcmp(argv[1], "cycle"))
    for (size_t n = 0; n < COUNT; n++)
      model_wanted[n] = tasks[(n + 1) % COUNT]->future;
  for (size_t n = 0; n < COUNT; n++)
    rinzu_aot_async_resume(&root, tasks[n]);
  assert(rinzu_activity_stats(root.transitions.activity_board).wait_live == COUNT);
  if (model_pump) {
    rinzu_async_materialize(leaf);
    rinzu_aot_async_pump(&root, tasks[0]->future);
    assert(model_stopped && model_wanted[0]->joined && model_wanted[0]->settled);
    assert(!state.runtime.domains && !state.runtime.in_flight && !state.runtime.transport_bytes);
  } else {
    rinzu_async_materialize(leaf);
    for (size_t n = 0; n < COUNT; n++)
      rinzu_aot_async_resume(&root, tasks[n]);
  }
  rinzu_activity_stats_t stats = rinzu_activity_stats(root.transitions.activity_board);
  rinzu_continuation_cache_dispose(&state.continuations);
  assert(!state.tasks && !state.continuations.live && !state.continuations.mapped_bytes);
  assert(!stats.wait_live && !stats.wait_bytes && !stats.ready_live && !stats.ready_bytes);
  rinzu_async_dispose(&state.runtime);
  rinzu_owner_transition_finish(&root.transitions);
  rinzu_managed_data_dispose(&root.data);
  puts("ASYNC_WAIT_MODEL=baseline");
}
